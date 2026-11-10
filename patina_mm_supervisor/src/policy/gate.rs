//! Firmware policy gate.
//!
//! Wraps the validated firmware policy blob and answers whether an operation is permitted:
//! reading or writing an MSR or I/O port, running a privileged instruction, reading a
//! save-state field, or accessing a memory range.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::error::MmSupervisorResult;

use super::{
    ACCESS_ATTR_ALLOW, ACCESS_ATTR_DENY, AccessType, Instruction, IoWidth, MemDescriptorV1_0, PolicyRootV1,
    RESOURCE_ATTR_COND_READ, RESOURCE_ATTR_EXECUTE, RESOURCE_ATTR_READ, RESOURCE_ATTR_STRICT_WIDTH, SaveStateCondition,
    SaveStateField, SecurePolicyDataV1_0, TYPE_INSTRUCTION, TYPE_IO, TYPE_MEM, TYPE_MSR, TYPE_SAVE_STATE,
    validation::{IsInsideMmramFn, walk_page_table},
};
use spin::Once;

/// Errors that can occur during policy gate operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyGateError {
    /// The policy pointer is null.
    NullPointer,
    /// Invalid policy version.
    InvalidVersion,
    /// Invalid access mask specified.
    InvalidAccessMask,
    /// Invalid I/O address (out of 16-bit range).
    InvalidIoAddress,
    /// Invalid I/O address range (overflow).
    InvalidIoRange,
    /// Invalid instruction index.
    InvalidInstructionIndex,
    /// Policy root not found for the requested type.
    PolicyRootNotFound,
    /// The policy blob's internal offsets do not describe a structure inside its own buffer.
    MalformedPolicy,
    /// Access denied by policy.
    AccessDenied,
    /// The destination buffer is too small for the merged policy.
    BufferTooSmall,
    /// The memory policy buffer is null or holds no descriptors.
    InvalidMemoryPolicyBuffer,
    /// The memory policy buffer was already set, and may not be replaced.
    MemoryPolicyBufferAlreadySet,
    /// The memory policy snapshot was already taken, and may not be retaken.
    AlreadyLocked,
    /// Internal error during policy evaluation.
    InternalError,
}

impl core::error::Error for PolicyGateError {}

impl core::fmt::Display for PolicyGateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NullPointer => write!(f, "the policy pointer is null"),
            Self::InvalidVersion => write!(f, "the policy version is not supported"),
            Self::InvalidAccessMask => write!(f, "the requested access mask is invalid"),
            Self::InvalidIoAddress => write!(f, "the I/O address is outside the 16-bit port range"),
            Self::InvalidIoRange => write!(f, "the I/O address range overflows"),
            Self::InvalidInstructionIndex => write!(f, "the instruction index is not a known instruction"),
            Self::PolicyRootNotFound => write!(f, "the policy contains no root for the requested type"),
            Self::MalformedPolicy => {
                write!(f, "the policy blob's internal offsets do not describe a structure inside its own buffer")
            }
            Self::AccessDenied => write!(f, "the firmware policy denied the access"),
            Self::BufferTooSmall => write!(f, "the destination buffer is too small for the policy"),
            Self::InvalidMemoryPolicyBuffer => write!(f, "the memory policy buffer is null or empty"),
            Self::MemoryPolicyBufferAlreadySet => write!(f, "the memory policy buffer is already set"),
            Self::AlreadyLocked => write!(f, "the memory policy snapshot is already taken"),
            Self::InternalError => write!(f, "an internal error occurred while evaluating the policy"),
        }
    }
}

/// Checks that every offset and count inside the policy blob describes a structure lying wholly
/// within `buffer_size` bytes.
///
/// The blob comes from firmware, and the descriptor accessors, per-syscall lookups and the
/// `FETCH_POLICY` copy-out trust its fields. Validating the layout once here is what makes their
/// unchecked `slice::from_raw_parts` calls sound.
///
/// Every offset in the blob is a byte offset from the start of the blob, not an address. So the
/// end of each structure is also an offset, and it is compared with the size the blob declares.
///
/// ## Errors
///
/// Returns `NullPointer` for a null `policy_ptr`, `InvalidVersion` for a policy that is not
/// V1.0, and `MalformedPolicy` when an offset or count reaches past the buffer.
///
/// ## Safety
///
/// A non-null `policy_ptr` must point to `buffer_size` readable bytes.
unsafe fn validate_policy_layout(policy_ptr: *const u8, buffer_size: usize) -> MmSupervisorResult<()> {
    if policy_ptr.is_null() {
        log::error!("Policy gate creation failed: policy buffer pointer is null");
        return Err(PolicyGateError::NullPointer.into());
    }

    let header_size = core::mem::size_of::<SecurePolicyDataV1_0>();
    if buffer_size < header_size {
        log::error!("policy blob buffer is {buffer_size} bytes, too small for a {header_size}-byte header");
        return Err(PolicyGateError::MalformedPolicy.into());
    }

    // SAFETY: `policy_ptr` is non-null (checked above), the buffer holds a complete header
    // (checked above), and the caller guarantees `policy_ptr` is readable for `buffer_size` bytes.
    let policy = unsafe { &*policy_ptr.cast::<SecurePolicyDataV1_0>() };

    if !policy.is_valid_version() {
        log::error!(
            "Policy gate creation failed: unsupported policy version {}.{}",
            policy.version_major,
            policy.version_minor
        );
        return Err(PolicyGateError::InvalidVersion.into());
    }

    // Every offset below is checked against the declared size, so the declared size must first
    // fit the buffer. `fetch_and_update_policy` also copies exactly this many bytes out.
    let declared = policy.size as usize;
    if declared < header_size || declared > buffer_size {
        log::error!("policy blob declares {declared} bytes in a {buffer_size}-byte buffer");
        return Err(PolicyGateError::MalformedPolicy.into());
    }

    let roots_end_offset = (policy.policy_root_count as usize)
        .checked_mul(core::mem::size_of::<PolicyRootV1>())
        .and_then(|bytes| bytes.checked_add(policy.policy_root_offset as usize))
        .ok_or(PolicyGateError::MalformedPolicy)?;
    if roots_end_offset > declared {
        log::error!(
            "policy root array ends at offset {roots_end_offset}, past the {declared}-byte blob ({} roots at offset {})",
            policy.policy_root_count,
            policy.policy_root_offset
        );
        return Err(PolicyGateError::MalformedPolicy.into());
    }

    // SAFETY: the roots array lies wholly inside the blob (checked above).
    let roots = unsafe { policy.get_policy_roots() };
    for root in roots {
        // A type the supervisor does not recognize is never looked up, so its descriptors are
        // never dereferenced and need no bounds.
        let Some(desc_size) = super::descriptor_size(root.policy_type) else {
            continue;
        };

        let descriptors_end_offset = (root.count as usize)
            .checked_mul(desc_size)
            .and_then(|bytes| bytes.checked_add(root.offset as usize))
            .ok_or(PolicyGateError::MalformedPolicy)?;
        if descriptors_end_offset > declared {
            log::error!(
                "policy type {} descriptors end at offset {descriptors_end_offset}, past the {declared}-byte blob",
                root.policy_type
            );
            return Err(PolicyGateError::MalformedPolicy.into());
        }
    }

    Ok(())
}

/// Runtime access checks against the firmware policy.
///
/// Wraps a policy buffer and checks whether I/O, MSR, instruction and save state accesses are
/// allowed. It also holds the memory policy snapshot taken at ready-to-lock.
pub struct PolicyGate {
    /// Pointer to the firmware policy data (static, read-only).
    policy_ptr: *const u8,
    /// Memory policy buffer (written by `walk_page_table` during snapshot).
    memory_policy_buffer: *mut MemDescriptorV1_0,
    /// Maximum number of `MemDescriptorV1_0` entries the memory policy buffer can hold.
    memory_policy_max_count: usize,
    /// Number of descriptors in the snapshot buffer: `None` before ready-to-lock, `Some(count)`
    /// once the snapshot is taken.
    snapshot_count: Once<usize>,
}

// SAFETY: PolicyGate only holds pointers into stable policy/snapshot memory; the snapshot buffer
// is written only during the single ready-to-lock event, so moving it between MM cores is
// race-free.
unsafe impl Send for PolicyGate {}
// SAFETY: see the `Send` impl above.
unsafe impl Sync for PolicyGate {}

impl PolicyGate {
    /// Creates a new policy gate from a policy buffer pointer.
    ///
    /// `buffer_size` is the size of the allocation at `policy_ptr`, as reported by the `PassDown`
    /// HOB. The blob's internal offsets are validated against it, so the descriptor accessors and
    /// the `FETCH_POLICY` copy-out can trust them later.
    ///
    /// ## Errors
    ///
    /// Returns `NullPointer` for a null `policy_ptr`, `InvalidVersion` for a policy that is not
    /// V1.0, and `MalformedPolicy` when an offset or count reaches past the buffer.
    ///
    /// ## Safety
    ///
    /// A non-null `policy_ptr` must point to `buffer_size` readable bytes that stay valid for the
    /// lifetime of this `PolicyGate`.
    pub unsafe fn new(policy_ptr: *const u8, buffer_size: usize) -> MmSupervisorResult<Self> {
        // SAFETY: per this function's contract, a non-null `policy_ptr` points to `buffer_size`
        // readable bytes that outlive the gate. The null case is rejected inside the call.
        unsafe { validate_policy_layout(policy_ptr, buffer_size) }?;

        Ok(Self {
            policy_ptr,
            memory_policy_buffer: core::ptr::null_mut(),
            memory_policy_max_count: 0,
            snapshot_count: Once::new(),
        })
    }

    /// Sets the buffer that receives the page table snapshot. Call once, before
    /// [`take_snapshot`](Self::take_snapshot); the address and size usually come from the
    /// `PassDown` HOB.
    ///
    /// `buffer` must hold at least `max_count` `MemDescriptorV1_0` entries and stay valid for the
    /// lifetime of the `PolicyGate`. Storing the pointer is safe; that contract is part of the
    /// safety requirements of [`Self::take_snapshot`], [`Self::verify_snapshot`] and
    /// [`Self::fetch_and_update_policy`], which dereference it.
    ///
    /// ## Errors
    ///
    /// Returns `InvalidMemoryPolicyBuffer` for a null `buffer` or a zero `max_count`, and
    /// `MemoryPolicyBufferAlreadySet` if a buffer was already set. The snapshot is the baseline
    /// that later page table checks compare against, so it may not be moved once set.
    pub fn set_memory_policy_buffer(
        &mut self,
        buffer: *mut MemDescriptorV1_0,
        max_count: usize,
    ) -> MmSupervisorResult<()> {
        if !self.memory_policy_buffer.is_null() {
            log::error!("memory policy buffer is already set");
            return Err(PolicyGateError::MemoryPolicyBufferAlreadySet.into());
        }
        if buffer.is_null() || max_count == 0 {
            log::error!("memory policy buffer {buffer:p} with {max_count} entries is not usable");
            return Err(PolicyGateError::InvalidMemoryPolicyBuffer.into());
        }

        self.memory_policy_buffer = buffer;
        self.memory_policy_max_count = max_count;
        Ok(())
    }

    /// Returns the policy header.
    pub(crate) fn policy(&self) -> &SecurePolicyDataV1_0 {
        // SAFETY: `new` checked that the pointer is non-null and the buffer holds a full header,
        // and the buffer outlives the gate.
        unsafe { &*self.policy_ptr.cast::<SecurePolicyDataV1_0>() }
    }

    /// Returns the policy roots the blob declares.
    ///
    /// Safe because [`PolicyGate::new`] checked that `policy_root_offset` and
    /// `policy_root_count` describe an array wholly inside the blob, and the buffer outlives the
    /// gate.
    pub(crate) fn policy_roots(&self) -> &[PolicyRootV1] {
        // SAFETY: the layout check in `new` established that the root array is in bounds.
        unsafe { self.policy().get_policy_roots() }
    }

    /// Finds a policy root by type.
    fn find_policy_root(&self, policy_type: u32) -> Option<&PolicyRootV1> {
        self.policy_roots().iter().find(|root| root.policy_type == policy_type)
    }

    /// Returns `Ok` if the policy allows the I/O access.
    ///
    /// `io_address` must be in the 16-bit I/O port space (at most 0xFFFF).
    pub fn is_io_allowed(&self, io_address: u32, width: IoWidth, access_type: AccessType) -> MmSupervisorResult<()> {
        // I/O access is read or write, never execute.
        if access_type == AccessType::Execute {
            log::error!("Rejecting IO access: port=0x{io_address:x} requested Execute access");
            return Err(PolicyGateError::InvalidAccessMask.into());
        }

        let io_size = width.size();

        if io_address > u32::from(u16::MAX) {
            log::error!("Rejecting IO access: port 0x{io_address:x} is outside the 16-bit I/O space");
            return Err(PolicyGateError::InvalidIoAddress.into());
        }

        // The access may end exactly at 0x10000.
        if io_address.saturating_add(io_size) > u32::from(u16::MAX) + 1 {
            log::error!("Rejecting IO access: port 0x{io_address:x} with width {io_size} runs past the I/O space");
            return Err(PolicyGateError::InvalidIoRange.into());
        }

        let Some(policy_root) = self.find_policy_root(TYPE_IO) else {
            log::warn!("Could not find IO policy root, denying access to be safe.");
            return Err(PolicyGateError::PolicyRootNotFound.into());
        };

        // SAFETY: `new` checked that this root's descriptor array lies inside the blob.
        let descriptors = unsafe { policy_root.get_io_descriptors(self.policy_ptr) };
        let access_mask = access_type.as_attr_mask();

        let mut found_match = false;

        for desc in descriptors {
            let desc_start = u32::from(desc.io_address);
            let desc_size = u32::from(desc.length_or_width);
            let is_strict_width = (u32::from(desc.attributes) & RESOURCE_ATTR_STRICT_WIDTH) != 0;

            if is_strict_width {
                // Strict width: address and size must match exactly.
                if io_address == desc_start && io_size == desc_size {
                    // The descriptor must also grant the access type.
                    if (u32::from(desc.attributes) & access_mask) != 0 {
                        found_match = true;
                        break;
                    }
                }
            } else {
                // Otherwise the descriptor must cover the whole access.
                let desc_end = desc_start.saturating_add(desc_size);
                let our_end = io_address.saturating_add(io_size);

                if io_address >= desc_start && our_end <= desc_end {
                    // The descriptor must also grant the access type.
                    if (u32::from(desc.attributes) & access_mask) != 0 {
                        found_match = true;
                        break;
                    }
                }
            }
        }

        // Evaluate based on allow/deny list semantics
        let allowed = if policy_root.access_attr == ACCESS_ATTR_ALLOW {
            found_match
        } else if policy_root.access_attr == ACCESS_ATTR_DENY {
            !found_match
        } else {
            log::error!(
                "IO access: unrecognized policy access_attr 0x{:x}; denying (fail-closed).",
                policy_root.access_attr
            );
            false
        };

        if !allowed {
            log::debug!("Rejecting IO access: port=0x{io_address:x}, width={io_size}, type={access_type:?}");
            return Err(PolicyGateError::AccessDenied.into());
        }

        Ok(())
    }

    /// Returns `Ok` if the policy allows the MSR access.
    pub fn is_msr_allowed(&self, msr_address: u32, access_type: AccessType) -> MmSupervisorResult<()> {
        // MSR access is read or write, never execute.
        if access_type == AccessType::Execute {
            log::error!("Rejecting MSR access: address=0x{msr_address:x} requested Execute access");
            return Err(PolicyGateError::InvalidAccessMask.into());
        }

        let Some(policy_root) = self.find_policy_root(TYPE_MSR) else {
            log::warn!("Could not find MSR policy root, denying access to be safe.");
            return Err(PolicyGateError::PolicyRootNotFound.into());
        };

        // SAFETY: `new` checked that this root's descriptor array lies inside the blob.
        let descriptors = unsafe { policy_root.get_msr_descriptors(self.policy_ptr) };
        let access_mask = access_type.as_attr_mask();

        let mut found_match = false;

        for desc in descriptors {
            let desc_start = desc.msr_address;
            let desc_end = desc_start.saturating_add(u32::from(desc.length));

            if msr_address >= desc_start && msr_address < desc_end && (u32::from(desc.attributes) & access_mask) != 0 {
                found_match = true;
                break;
            }
        }

        // Evaluate based on allow/deny list semantics
        let allowed = if policy_root.access_attr == ACCESS_ATTR_ALLOW {
            found_match
        } else if policy_root.access_attr == ACCESS_ATTR_DENY {
            !found_match
        } else {
            log::error!(
                "MSR access: unrecognized policy access_attr 0x{:x}; denying (fail-closed).",
                policy_root.access_attr
            );
            false
        };

        if !allowed {
            log::debug!("Rejecting MSR access: address=0x{msr_address:x}, type={access_type:?}");
            return Err(PolicyGateError::AccessDenied.into());
        }

        Ok(())
    }

    /// Returns `Ok` if the policy allows running the instruction.
    pub fn is_instruction_allowed(&self, instruction: Instruction) -> MmSupervisorResult<()> {
        let instruction_index = instruction.as_index();

        if instruction_index >= Instruction::COUNT {
            log::error!(
                "Rejecting instruction execution: index {instruction_index} is outside 0..{}",
                Instruction::COUNT
            );
            return Err(PolicyGateError::InvalidInstructionIndex.into());
        }

        let Some(policy_root) = self.find_policy_root(TYPE_INSTRUCTION) else {
            log::warn!("Could not find Instruction policy root, denying access to be safe.");
            return Err(PolicyGateError::PolicyRootNotFound.into());
        };

        // SAFETY: `new` checked that this root's descriptor array lies inside the blob.
        let descriptors = unsafe { policy_root.get_instruction_descriptors(self.policy_ptr) };

        let mut found_match = false;

        for desc in descriptors {
            if instruction_index == desc.instruction_index && (u32::from(desc.attributes) & RESOURCE_ATTR_EXECUTE) != 0
            {
                found_match = true;
                break;
            }
        }

        // Evaluate based on allow/deny list semantics
        let allowed = if policy_root.access_attr == ACCESS_ATTR_ALLOW {
            found_match
        } else if policy_root.access_attr == ACCESS_ATTR_DENY {
            !found_match
        } else {
            log::error!(
                "Instruction execution: unrecognized policy access_attr 0x{:x}; denying (fail-closed).",
                policy_root.access_attr
            );
            false
        };

        if !allowed {
            log::debug!("Rejecting instruction execution: {instruction:?}");
            return Err(PolicyGateError::AccessDenied.into());
        }

        Ok(())
    }

    /// Returns `Ok` if the policy allows the save state read.
    ///
    /// With no save state policy root (a level 20 policy) every read is allowed.
    ///
    /// `field` is `None` for a register that has no policy field. No descriptor can name
    /// such a register, so the policy root's access attribute alone decides the result:
    ///
    /// - An `ACCESS_ATTR_ALLOW` root permits only the reads its descriptors name, so it
    ///   denies the read.
    /// - An `ACCESS_ATTR_DENY` root blocks only the reads its descriptors name, so it
    ///   permits the read.
    pub fn is_save_state_read_allowed(
        &self,
        field: Option<SaveStateField>,
        width: usize,
        current_condition: Option<SaveStateCondition>,
    ) -> MmSupervisorResult<()> {
        let Some(policy_root) = self.find_policy_root(TYPE_SAVE_STATE) else {
            // No save state root means a level 20 policy, which allows all reads.
            log::warn!("No save state policy root found, allowing read (level 20 policy).");
            return Ok(());
        };

        // SAFETY: `new` checked that this root's descriptor array lies inside the blob.
        let descriptors = unsafe { policy_root.get_save_state_descriptors(self.policy_ptr) };

        let mut found_match = false;

        // Only RAX / IO_TRAP have a policy field; every other register has
        // `field == None` and therefore matches no descriptor.
        for desc in descriptors {
            if field.is_some_and(|field| desc.map_field == field.as_index()) {
                let is_read = (desc.attributes & RESOURCE_ATTR_READ) != 0;
                let is_cond_read = (desc.attributes & RESOURCE_ATTR_COND_READ) != 0;

                if is_read || is_cond_read {
                    // A conditional read matches only under the same trap condition.
                    if is_cond_read {
                        if let Some(current) = current_condition
                            && desc.access_condition == current as u32
                        {
                            found_match = true;
                            break;
                        }
                        // Condition does not match; keep looking.
                    } else {
                        // Unconditional read
                        if desc.access_condition == SaveStateCondition::Unconditional as u32 {
                            found_match = true;
                            break;
                        }
                    }
                }
            }
        }

        // Evaluate based on allow/deny list semantics
        let allowed = if policy_root.access_attr == ACCESS_ATTR_ALLOW {
            // Allow-list: access is granted only if a matching descriptor was found.
            found_match
        } else if policy_root.access_attr == ACCESS_ATTR_DENY {
            // Deny-list: access is granted only if no matching descriptor was found.
            !found_match
        } else {
            log::error!(
                "Save state read: unrecognized policy access_attr 0x{:x}; denying (fail-closed).",
                policy_root.access_attr
            );
            false
        };

        if !allowed {
            log::debug!("Rejecting save state read: field={field:?}, width={width}");
            return Err(PolicyGateError::AccessDenied.into());
        }

        Ok(())
    }

    /// Returns the raw policy pointer.
    pub fn as_ptr(&self) -> *const u8 {
        self.policy_ptr
    }

    /// Returns `true` if the ready-to-lock snapshot has been taken.
    pub fn is_locked(&self) -> bool {
        self.snapshot_count.get().is_some()
    }

    /// Returns the snapshot descriptor count, or `None` if not yet locked.
    pub fn snapshot_count(&self) -> Option<usize> {
        self.snapshot_count.get().copied()
    }

    /// Returns the firmware policy blob size (from `SecurePolicyDataV1_0::size`).
    pub fn firmware_policy_size(&self) -> usize {
        self.policy().size as usize
    }

    /// Takes the memory policy snapshot from the page table and locks the gate.
    ///
    /// Walks the active page table, writes the descriptors into the memory policy buffer, and
    /// records the descriptor count. Afterwards [`is_locked`](Self::is_locked) returns `true`.
    ///
    /// ## Errors
    ///
    /// Returns `AlreadyLocked` if the snapshot was already taken. The snapshot is the baseline
    /// for [`verify_snapshot`](Self::verify_snapshot), so a second walk is refused rather than
    /// allowed to overwrite it. Returns `InternalError` if no memory policy buffer is set or the
    /// page table walk fails.
    ///
    /// ## Safety
    ///
    /// - `cr3` must point to a valid, stable PML4 table.
    /// - The memory policy buffer (set with [`Self::set_memory_policy_buffer`]) must still be
    ///   valid and large enough.
    pub unsafe fn take_snapshot(&self, cr3: u64, is_inside_mmram: IsInsideMmramFn) -> MmSupervisorResult<usize> {
        if self.is_locked() {
            log::error!("take_snapshot: the memory policy snapshot is already taken");
            return Err(PolicyGateError::AlreadyLocked.into());
        }

        if self.memory_policy_buffer.is_null() || self.memory_policy_max_count == 0 {
            log::error!("take_snapshot: memory policy buffer not configured");
            return Err(PolicyGateError::InternalError.into());
        }

        // SAFETY: the caller guarantees `cr3` points to a valid PML4 and the memory policy buffer
        // can hold `memory_policy_max_count` descriptors.
        let count =
            unsafe { walk_page_table(cr3, self.memory_policy_buffer, self.memory_policy_max_count, is_inside_mmram) }
                .map_err(|err| {
                log::error!("take_snapshot: walk_page_table failed: {err:?}");
                PolicyGateError::InternalError
            })?;

        Ok(self.record_snapshot(count))
    }

    /// Records that the memory policy buffer holds `count` descriptors and locks the gate.
    fn record_snapshot(&self, count: usize) -> usize {
        let count = *self.snapshot_count.call_once(|| count);
        log::info!("Policy snapshot taken: {count} descriptors, ready-to-lock is now TRUE");
        count
    }

    /// Verifies that the current page table still matches the saved snapshot.
    ///
    /// The fresh walk goes into `scratch` so the saved snapshot is not overwritten.
    ///
    /// ## Errors
    ///
    /// Returns `AccessDenied` if the page table differs from the snapshot, and `InternalError`
    /// if there is no snapshot or the walk fails. This routine detects tampering, so "nothing to
    /// compare" must never pass.
    ///
    /// ## Safety
    ///
    /// - `cr3` must point to a valid, stable PML4 table.
    /// - `scratch` must point to a buffer of at least `scratch_max_count`
    ///   `MemDescriptorV1_0` entries.
    pub unsafe fn verify_snapshot(
        &self,
        cr3: u64,
        is_inside_mmram: IsInsideMmramFn,
        scratch: *mut MemDescriptorV1_0,
        scratch_max_count: usize,
    ) -> MmSupervisorResult<()> {
        let Some(&saved_count) = self.snapshot_count.get() else {
            log::error!("verify_snapshot: no snapshot to verify against");
            return Err(PolicyGateError::InternalError.into());
        };

        // SAFETY: the caller guarantees `cr3` points to a valid PML4 and `scratch` can hold
        // `scratch_max_count` descriptors.
        let fresh_count =
            unsafe { walk_page_table(cr3, scratch, scratch_max_count, is_inside_mmram) }.map_err(|err| {
                log::error!("verify_snapshot: walk_page_table failed: {err:?}");
                PolicyGateError::InternalError
            })?;

        // View both buffers as slices so the comparison runs in safe code.
        //
        // SAFETY: `walk_page_table` filled `scratch` with `fresh_count` descriptors, and the earlier
        // `take_snapshot` filled the snapshot buffer with `saved_count` descriptors.
        let (saved, fresh) = unsafe {
            (
                core::slice::from_raw_parts(self.memory_policy_buffer.cast_const(), saved_count),
                core::slice::from_raw_parts(scratch.cast_const(), fresh_count),
            )
        };

        compare_snapshot(saved, fresh)
    }

    /// Writes the merged firmware and memory policy into `dest` and returns the number of bytes
    /// written.
    ///
    /// Matches the C `FetchNUpdateSecurityPolicy`. The snapshot must already be taken with
    /// [`take_snapshot`](Self::take_snapshot). Layout written to `dest`:
    ///
    /// ```text
    /// |--------------------------------------|
    /// | SecurePolicyDataV1_0 + payload       |  <- firmware policy blob (copied first)
    /// |--------------------------------------|
    /// | MemDescriptorV1_0[0..N]              |  <- memory policy snapshot (appended)
    /// |--------------------------------------|
    /// ```
    ///
    /// After the copy the header is patched in place:
    ///
    /// * The `TYPE_MEM` root gets `offset` set to `fw_size`, `count` set to the snapshot count,
    ///   and `access_attr` set to `ACCESS_ATTR_ALLOW`.
    /// * The header `size` becomes `fw_size + mem_policy_bytes`.
    /// * The legacy `memory_policy_count` is zeroed (unused with the root-based layout).
    ///
    /// Any request header must be written or reserved by the caller before `dest`.
    ///
    /// ## Errors
    ///
    /// Returns `BufferTooSmall` when `dest_size` cannot hold the merged policy, `InternalError`
    /// when there is no snapshot or a size overflows, and `PolicyRootNotFound` when the firmware
    /// policy has no `TYPE_MEM` root.
    ///
    /// ## Safety
    ///
    /// `dest` must point to a writable buffer of at least `dest_size` bytes.
    pub unsafe fn fetch_and_update_policy(&self, dest: *mut u8, dest_size: usize) -> MmSupervisorResult<usize> {
        let count = self.snapshot_count.get().copied().ok_or_else(|| {
            log::error!("fetch_and_update_policy: no snapshot taken");
            PolicyGateError::InternalError
        })?;

        let desc_size = core::mem::size_of::<MemDescriptorV1_0>();
        let mem_policy_bytes = count.checked_mul(desc_size).ok_or_else(|| {
            log::error!("fetch_and_update_policy: descriptor count overflow");
            PolicyGateError::InternalError
        })?;

        let fw_size = self.firmware_policy_size();
        if fw_size == 0 {
            log::error!("fetch_and_update_policy: firmware policy size is 0");
            return Err(PolicyGateError::InternalError.into());
        }

        let total_bytes = fw_size.checked_add(mem_policy_bytes).ok_or_else(|| {
            log::error!("fetch_and_update_policy: total size overflow");
            PolicyGateError::InternalError
        })?;

        if dest_size < total_bytes {
            log::error!("fetch_and_update_policy: buffer too small ({dest_size} bytes, need {total_bytes})");
            return Err(PolicyGateError::BufferTooSmall.into());
        }

        // 1. Copy the firmware policy blob (header + payload), then append the
        //    memory policy descriptors after it.
        //
        // SAFETY: the caller guarantees `dest` is writable for `dest_size` bytes, which is at
        // least `total_bytes` (checked above). `new` checked that the blob's declared `size` fits
        // its buffer, so reading `fw_size` bytes stays inside it. The memory policy buffer holds
        // `count` valid descriptors from the earlier `take_snapshot`.
        unsafe {
            core::ptr::copy_nonoverlapping(self.policy_ptr, dest, fw_size);
            if mem_policy_bytes > 0 {
                let src = self.memory_policy_buffer as *const u8;
                core::ptr::copy_nonoverlapping(src, dest.add(fw_size), mem_policy_bytes);
            }
        }

        // `dest` follows the request header in the communication buffer, so it has no alignment
        // guarantee. The header and roots are copied out and back with unaligned accesses rather
        // than viewed through references.
        let header_ptr = dest.cast::<SecurePolicyDataV1_0>();

        // 2. Read the root table location from the copied header.
        //
        // SAFETY: `dest` now holds a valid `SecurePolicyDataV1_0` header copied from the
        // validated firmware policy blob above. `read_unaligned` has no alignment requirement.
        let mut header = unsafe { header_ptr.read_unaligned() };
        let root_offset = header.policy_root_offset as usize;
        let root_count = header.policy_root_count as usize;

        // 3. Find the TYPE_MEM root.
        //
        // SAFETY: `root_offset` and `root_count` come from the header copied from the blob, and
        // `new` checked that the roots array they describe ends within the blob's declared size,
        // which is `fw_size` and so within the `total_bytes` copied above.
        let roots_ptr = unsafe { dest.add(root_offset) }.cast::<PolicyRootV1>();
        let mem_root = (0..root_count)
            // SAFETY: `index < root_count`, so each root lies in the roots array (see above).
            .map(|index| unsafe { roots_ptr.add(index) })
            // SAFETY: Each pointer is in bounds, and `read_unaligned` has no alignment requirement.
            .map(|root_ptr| (root_ptr, unsafe { root_ptr.read_unaligned() }))
            .find(|(_, root)| root.policy_type == TYPE_MEM);
        let Some((mem_root_ptr, mut mem_root)) = mem_root else {
            log::error!("fetch_and_update_policy: firmware policy has no TYPE_MEM policy root");
            return Err(PolicyGateError::PolicyRootNotFound.into());
        };

        let to_u32 = |value: usize, what: &str| {
            u32::try_from(value).map_err(|_| {
                log::error!("fetch_and_update_policy: {what} {value} does not fit in u32");
                PolicyGateError::InternalError
            })
        };

        // Point the TYPE_MEM root at the appended snapshot.
        mem_root.access_attr = ACCESS_ATTR_ALLOW;
        mem_root.offset = to_u32(fw_size, "firmware policy size")?;
        mem_root.count = to_u32(count, "descriptor count")?;

        // 4. Update the total size and clear the legacy `memory_policy_count`.
        header.size = to_u32(total_bytes, "total policy size")?;
        header.memory_policy_count = 0;

        // SAFETY: Both pointers were read above and lie within the `total_bytes` written to
        // `dest`. `write_unaligned` has no alignment requirement.
        unsafe {
            mem_root_ptr.write_unaligned(mem_root);
            header_ptr.write_unaligned(header);
        }

        log::debug!(
            "fetch_and_update_policy: wrote {total_bytes} bytes (fw_policy={fw_size}, mem_policy={mem_policy_bytes} ({count} descs))",
        );
        Ok(total_bytes)
    }
}

/// Returns an error when a freshly walked page table no longer matches the recorded snapshot.
fn compare_snapshot(saved: &[MemDescriptorV1_0], fresh: &[MemDescriptorV1_0]) -> MmSupervisorResult<()> {
    if saved.len() != fresh.len() {
        log::error!("verify_snapshot: descriptor count mismatch (saved={}, fresh={})", saved.len(), fresh.len());
        return Err(PolicyGateError::AccessDenied.into());
    }

    for (index, (saved, fresh)) in saved.iter().zip(fresh.iter()).enumerate() {
        if saved != fresh {
            log::error!(
                "verify_snapshot: descriptor {} mismatch - \
                 saved=(base=0x{:x}, size=0x{:x}, attrs=0x{:x}) vs \
                 fresh=(base=0x{:x}, size=0x{:x}, attrs=0x{:x})",
                index,
                saved.base_address,
                saved.size,
                saved.mem_attributes,
                fresh.base_address,
                fresh.size,
                fresh.mem_attributes,
            );
            return Err(PolicyGateError::AccessDenied.into());
        }
    }

    log::debug!("verify_snapshot: page table matches saved snapshot ({} descriptors)", saved.len());
    Ok(())
}
