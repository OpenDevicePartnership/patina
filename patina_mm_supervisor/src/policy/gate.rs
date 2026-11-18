//! Firmware policy gate.
//!
//! Wraps the validated firmware policy blob and answers whether an MSR or I/O access, a
//! privileged instruction or a save state read is permitted. Also holds the memory policy
//! snapshot taken at ready-to-lock.
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
    validation::{IsInsideMmramFn, PolicyValidationError, walk_page_table},
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

/// Checks that the policy blob's offsets and counts describe structures inside `buffer_size`
/// bytes. The descriptor accessors and the `FETCH_POLICY` copy-out rely on this.
///
/// Checks:
/// - `policy_ptr` is not null.
/// - The buffer holds a whole header, and the policy version is V1.0.
/// - The header, the root array and every descriptor array are aligned for their types.
/// - The declared size is at least the header size and at most `buffer_size`.
/// - Every root has a type the supervisor recognizes.
/// - The root array and every descriptor array end within the declared size.
///
/// ## Errors
///
/// Returns `NullPointer` for a null `policy_ptr`, `InvalidVersion` for a policy that is not
/// V1.0, `UnrecognizedPolicyType` for a root of a type the supervisor does not recognize, and
/// `MalformedPolicy` for any other failed check.
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

    // The header has the largest alignment, so once the blob is aligned each offset only has to
    // be a multiple of its own type's alignment.
    if !policy_ptr.cast::<SecurePolicyDataV1_0>().is_aligned() {
        log::error!("policy blob at {policy_ptr:p} is not aligned for its header");
        return Err(PolicyGateError::MalformedPolicy.into());
    }

    // SAFETY: `policy_ptr` is non-null, aligned and readable for a whole header (checked above
    // and by the caller).
    let policy = unsafe { &*policy_ptr.cast::<SecurePolicyDataV1_0>() };

    if !policy.is_valid_version() {
        log::error!(
            "Policy gate creation failed: unsupported policy version {}.{}",
            policy.version_major,
            policy.version_minor
        );
        return Err(PolicyGateError::InvalidVersion.into());
    }

    // Later offsets are checked against the declared size, which `fetch_and_update_policy` also
    // copies out, so it must fit the buffer.
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
    if !(policy.policy_root_offset as usize).is_multiple_of(core::mem::align_of::<PolicyRootV1>()) {
        log::error!("policy root array offset {} is not aligned for a policy root", policy.policy_root_offset);
        return Err(PolicyGateError::MalformedPolicy.into());
    }

    // SAFETY: the roots array lies wholly inside the blob and is aligned (checked above).
    let roots = unsafe { policy.get_policy_roots() };
    for root in roots {
        let (Some(desc_size), Some(desc_align)) =
            (super::descriptor_size(root.policy_type), super::descriptor_align(root.policy_type))
        else {
            log::error!("policy root has type {}, which the supervisor does not recognize", root.policy_type);
            return Err(PolicyValidationError::UnrecognizedPolicyType { policy_type: root.policy_type }.into());
        };

        if !(root.offset as usize).is_multiple_of(desc_align) {
            log::error!(
                "policy type {} descriptors at offset {} are not aligned for the descriptor type",
                root.policy_type,
                root.offset
            );
            return Err(PolicyGateError::MalformedPolicy.into());
        }

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

/// Runtime access checks against the firmware policy, plus the ready-to-lock memory policy
/// snapshot.
pub struct PolicyGate {
    /// Pointer to the firmware policy data (static, read-only).
    policy_ptr: *const u8,
    /// Memory policy buffer (written by `walk_page_table` during snapshot).
    memory_policy_buffer: *mut MemDescriptorV1_0,
    /// Maximum number of `MemDescriptorV1_0` entries the memory policy buffer can hold.
    memory_policy_max_count: usize,
    /// Number of descriptors in the snapshot buffer, set once the snapshot is taken.
    snapshot_count: Once<usize>,
}

// SAFETY: the gate only points into stable policy and snapshot memory, and the snapshot buffer is
// written only once, at ready-to-lock.
unsafe impl Send for PolicyGate {}
// SAFETY: see the `Send` impl above.
unsafe impl Sync for PolicyGate {}

impl PolicyGate {
    /// Creates a policy gate over the blob at `policy_ptr`, after validating its layout against
    /// `buffer_size`, the allocation size the `PassDown` HOB reports.
    ///
    /// ## Errors
    ///
    /// Returns `NullPointer` for a null `policy_ptr`, `InvalidVersion` for a policy that is not
    /// V1.0, `UnrecognizedPolicyType` for a root of a type the supervisor does not recognize, and
    /// `MalformedPolicy` when an offset or count reaches past the buffer or a structure is
    /// misaligned.
    ///
    /// ## Safety
    ///
    /// A non-null `policy_ptr` must point to `buffer_size` readable bytes that stay valid for the
    /// lifetime of this `PolicyGate`.
    pub unsafe fn new(policy_ptr: *const u8, buffer_size: usize) -> MmSupervisorResult<Self> {
        // SAFETY: this function's contract; the null case is rejected inside the call.
        unsafe { validate_policy_layout(policy_ptr, buffer_size) }?;

        Ok(Self {
            policy_ptr,
            memory_policy_buffer: core::ptr::null_mut(),
            memory_policy_max_count: 0,
            snapshot_count: Once::new(),
        })
    }

    /// Sets the buffer, of `max_count` descriptors, that receives the page table snapshot. Call
    /// once, before [`take_snapshot`](Self::take_snapshot).
    ///
    /// The buffer is only dereferenced by the unsafe snapshot functions, whose contracts require
    /// it to still be valid.
    ///
    /// ## Errors
    ///
    /// Returns `InvalidMemoryPolicyBuffer` for a null `buffer` or a zero `max_count`, and
    /// `MemoryPolicyBufferAlreadySet` if a buffer was already set.
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
    /// An access outside the 16-bit port space, or an `Execute` access, is rejected.
    pub fn is_io_allowed(&self, io_address: u32, width: IoWidth, access_type: AccessType) -> MmSupervisorResult<()> {
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
        let is_deny_list = policy_root.access_attr == ACCESS_ATTR_DENY;

        let mut found_match = false;

        for desc in descriptors {
            let desc_start = u32::from(desc.io_address);
            let desc_size = u32::from(desc.length_or_width);
            let is_strict_width = (u32::from(desc.attributes) & RESOURCE_ATTR_STRICT_WIDTH) != 0;

            let covers = if is_strict_width {
                // Strict width: address and size must match exactly.
                io_address == desc_start && io_size == desc_size
            } else {
                let desc_end = desc_start.saturating_add(desc_size);
                let our_end = io_address.saturating_add(io_size);
                if is_deny_list {
                    // Any overlap with a denied range is denied, so a wide access cannot reach a
                    // denied port by starting just below it.
                    io_address < desc_end && desc_start < our_end
                } else {
                    // An allowed range must cover the whole access.
                    io_address >= desc_start && our_end <= desc_end
                }
            };

            // The descriptor must also name the access type.
            if covers && (u32::from(desc.attributes) & access_mask) != 0 {
                found_match = true;
                break;
            }
        }

        let allowed = if policy_root.access_attr == ACCESS_ATTR_ALLOW {
            found_match
        } else if is_deny_list {
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
            // A u64 end keeps a range that reaches MSR 0xFFFF_FFFF from losing its last MSR.
            let desc_start = u64::from(desc.msr_address);
            let desc_end = desc_start + u64::from(desc.length);
            let msr = u64::from(msr_address);

            if msr >= desc_start && msr < desc_end && (u32::from(desc.attributes) & access_mask) != 0 {
                found_match = true;
                break;
            }
        }

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
    /// `field` is `None` for a register that has no policy field. No descriptor can name it, so
    /// an allow list denies the read and a deny list permits it.
    ///
    /// A `COND_READ` descriptor matches only when `current_condition` equals its condition, and a
    /// plain `READ` descriptor only when its condition is unconditional.
    pub fn is_save_state_read_allowed(
        &self,
        field: Option<SaveStateField>,
        width: usize,
        current_condition: Option<SaveStateCondition>,
    ) -> MmSupervisorResult<()> {
        let Some(policy_root) = self.find_policy_root(TYPE_SAVE_STATE) else {
            log::warn!("No save state policy root found, allowing read (level 20 policy).");
            return Ok(());
        };

        // SAFETY: `new` checked that this root's descriptor array lies inside the blob.
        let descriptors = unsafe { policy_root.get_save_state_descriptors(self.policy_ptr) };

        let mut found_match = false;

        for desc in descriptors {
            if field.is_some_and(|field| desc.map_field == field.as_index()) {
                let is_read = (desc.attributes & RESOURCE_ATTR_READ) != 0;
                let is_cond_read = (desc.attributes & RESOURCE_ATTR_COND_READ) != 0;

                if is_read || is_cond_read {
                    if is_cond_read {
                        if let Some(current) = current_condition
                            && desc.access_condition == current as u32
                        {
                            found_match = true;
                            break;
                        }
                    } else {
                        // A plain read needs an unconditional descriptor.
                        if desc.access_condition == SaveStateCondition::Unconditional as u32 {
                            found_match = true;
                            break;
                        }
                    }
                }
            }
        }

        let allowed = if policy_root.access_attr == ACCESS_ATTR_ALLOW {
            found_match
        } else if policy_root.access_attr == ACCESS_ATTR_DENY {
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
    /// Walks the page table at `cr3` into the memory policy buffer and returns the descriptor
    /// count.
    ///
    /// ## Errors
    ///
    /// Returns `AlreadyLocked` if the snapshot was already taken, since it is the baseline for
    /// [`verify_snapshot`](Self::verify_snapshot). Returns `InternalError` if no memory policy
    /// buffer is set or the page table walk fails.
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
    /// Checks:
    /// - A snapshot has been taken.
    /// - A fresh walk of the page table at `cr3` succeeds.
    /// - The fresh walk matches the snapshot.
    ///
    /// The fresh walk goes into `scratch`.
    ///
    /// ## Errors
    ///
    /// Returns `AccessDenied` if the page table differs from the snapshot, and `InternalError`
    /// if there is no snapshot or the walk fails.
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
    /// The snapshot must already be taken. Layout written to `dest`:
    ///
    /// ```text
    /// |--------------------------------------|
    /// | SecurePolicyDataV1_0 + payload       |  <- firmware policy blob (copied first)
    /// |--------------------------------------|
    /// | MemDescriptorV1_0[0..N]              |  <- memory policy snapshot (appended)
    /// |--------------------------------------|
    /// ```
    ///
    /// The copy is then patched: the `TYPE_MEM` root points at the snapshot as an allow list, the
    /// header `size` covers both parts, and the legacy `memory_policy_count` is zeroed.
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

        // SAFETY: `dest` is writable for at least `total_bytes` (checked above), `new` checked
        // that `fw_size` bytes fit the blob, and the snapshot buffer holds `count` descriptors.
        unsafe {
            crate::memory::copy_volatile(self.policy_ptr, dest, fw_size);
            if mem_policy_bytes > 0 {
                let src = self.memory_policy_buffer as *const u8;
                crate::memory::copy_volatile(src, dest.add(fw_size), mem_policy_bytes);
            }
        }

        // `dest` has no alignment guarantee, so the header and roots use unaligned accesses.
        let header_ptr = dest.cast::<SecurePolicyDataV1_0>();

        // SAFETY: `dest` now holds the header copied from the validated blob.
        let mut header = unsafe { header_ptr.read_unaligned() };
        let root_offset = header.policy_root_offset as usize;
        let root_count = header.policy_root_count as usize;

        // SAFETY: `new` checked that the root array ends within the blob's `fw_size` bytes, all of
        // which were copied.
        let roots_ptr = unsafe { dest.add(root_offset) }.cast::<PolicyRootV1>();
        let mem_root = (0..root_count)
            // SAFETY: `index < root_count`, so each root lies in the roots array (see above).
            .map(|index| unsafe { roots_ptr.add(index) })
            // SAFETY: each pointer is in bounds; the read is unaligned.
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

        mem_root.access_attr = ACCESS_ATTR_ALLOW;
        mem_root.offset = to_u32(fw_size, "firmware policy size")?;
        mem_root.count = to_u32(count, "descriptor count")?;

        header.size = to_u32(total_bytes, "total policy size")?;
        header.memory_policy_count = 0;

        // SAFETY: both pointers were read above and lie within the bytes written to `dest`.
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use crate::test_support;

    use super::super::test_support::{
        Descriptors, PolicyBuilder, TestPageTable, instruction, io, mem, msr, save_state,
    };
    use super::super::{RESOURCE_ATTR_COND_WRITE, RESOURCE_ATTR_WRITE, TYPE_MSR};
    use super::*;
    use patina_paging::MemoryAttributes;
    use zerocopy::IntoBytes;

    const READ: u16 = RESOURCE_ATTR_READ as u16;
    const WRITE: u16 = RESOURCE_ATTR_WRITE as u16;
    const EXECUTE: u16 = RESOURCE_ATTR_EXECUTE as u16;
    const STRICT: u16 = RESOURCE_ATTR_STRICT_WIDTH as u16;

    #[test]
    fn test_policy_gate_errors_render_each_variant_distinctly() {
        // Every denial reaches the syscall layer as a bare EFI status, so this message is the
        // only record of which check rejected the access. The table is exhaustive, so a variant
        // added without a Display arm fails here rather than rendering as another variant's text.
        let all = [
            (PolicyGateError::NullPointer, "the policy pointer is null"),
            (PolicyGateError::InvalidVersion, "the policy version is not supported"),
            (PolicyGateError::InvalidAccessMask, "the requested access mask is invalid"),
            (PolicyGateError::InvalidIoAddress, "the I/O address is outside the 16-bit port range"),
            (PolicyGateError::InvalidIoRange, "the I/O address range overflows"),
            (PolicyGateError::InvalidInstructionIndex, "the instruction index is not a known instruction"),
            (PolicyGateError::PolicyRootNotFound, "the policy contains no root for the requested type"),
            (
                PolicyGateError::MalformedPolicy,
                "the policy blob's internal offsets do not describe a structure inside its own buffer",
            ),
            (PolicyGateError::AccessDenied, "the firmware policy denied the access"),
            (PolicyGateError::BufferTooSmall, "the destination buffer is too small for the policy"),
            (PolicyGateError::InvalidMemoryPolicyBuffer, "the memory policy buffer is null or empty"),
            (PolicyGateError::MemoryPolicyBufferAlreadySet, "the memory policy buffer is already set"),
            (PolicyGateError::AlreadyLocked, "the memory policy snapshot is already taken"),
            (PolicyGateError::InternalError, "an internal error occurred while evaluating the policy"),
        ];

        for (error, expected) in all {
            assert_eq!(format!("{error}"), expected);
        }

        // A denial and a malformed blob are different findings, so they must not read alike.
        assert_ne!(format!("{}", PolicyGateError::AccessDenied), format!("{}", PolicyGateError::MalformedPolicy));
    }

    #[test]
    fn test_io_width() {
        assert_eq!(IoWidth::Byte.size(), 1);
        assert_eq!(IoWidth::Word.size(), 2);
        assert_eq!(IoWidth::Dword.size(), 4);
    }

    #[test]
    fn test_instruction_conversion() {
        assert_eq!(Instruction::Cli.as_index(), 0);
        assert_eq!(Instruction::Wbinvd.as_index(), 1);
        assert_eq!(Instruction::Hlt.as_index(), 2);
        assert_eq!(Instruction::COUNT, 3);
    }

    #[test]
    fn test_gate_rejects_an_invalid_policy_buffer() {
        assert_eq!(
            // SAFETY: `new` checks for null before dereferencing.
            unsafe { PolicyGate::new(core::ptr::null(), 0) }.err(),
            Some(PolicyGateError::NullPointer.into())
        );

        let wrong_version = PolicyBuilder::new().version(2, 0).build();
        assert_eq!(
            // SAFETY: the builder produced an aligned, fully-initialized header that outlives the call.
            unsafe { PolicyGate::new(wrong_version.as_ptr(), wrong_version.len()) }.err(),
            Some(PolicyGateError::InvalidVersion.into())
        );

        // A header that understates its own size is refused here, which is why
        // `security_policy_check` never has to repeat the check.
        let understated = PolicyBuilder::new().declared_size(4).build();
        assert_eq!(understated.try_gate().err(), Some(PolicyGateError::MalformedPolicy.into()));

        // So is one that declares more bytes than the buffer holds.
        let overstated = PolicyBuilder::new().declared_size(256).build();
        assert_eq!(overstated.try_gate().err(), Some(PolicyGateError::MalformedPolicy.into()));
    }

    #[test]
    fn test_gate_exposes_its_policy_buffer() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![])).build();
        let gate = policy.gate();

        assert_eq!(gate.as_ptr(), policy.as_ptr());
        assert_eq!(gate.firmware_policy_size(), 64);
        assert!(!gate.is_locked());
        assert_eq!(gate.snapshot_count(), None);
    }

    #[test]
    fn test_is_io_allowed_validates_its_arguments() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0x60, 1, READ)])).build();
        let gate = policy.gate();

        // Execute is not a meaningful I/O access type.
        assert_eq!(
            gate.is_io_allowed(0x60, IoWidth::Byte, AccessType::Execute),
            Err(PolicyGateError::InvalidAccessMask.into())
        );
        // Ports live in the 16-bit space.
        assert_eq!(
            gate.is_io_allowed(0x1_0000, IoWidth::Byte, AccessType::Read),
            Err(PolicyGateError::InvalidIoAddress.into())
        );
        // A width that runs off the end of the port space.
        assert_eq!(
            gate.is_io_allowed(0xFFFF, IoWidth::Dword, AccessType::Read),
            Err(PolicyGateError::InvalidIoRange.into())
        );
    }

    #[test]
    fn test_is_io_allowed_requires_a_policy_root() {
        let policy = PolicyBuilder::new().build();
        assert_eq!(
            policy.gate().is_io_allowed(0x60, IoWidth::Byte, AccessType::Read),
            Err(PolicyGateError::PolicyRootNotFound.into())
        );
    }

    #[test]
    fn test_is_io_allowed_matches_ranges_on_an_allow_list() {
        let policy =
            PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0xCF8, 8, READ | WRITE)])).build();
        let gate = policy.gate();

        assert_eq!(gate.is_io_allowed(0xCF8, IoWidth::Dword, AccessType::Read), Ok(()));
        assert_eq!(gate.is_io_allowed(0xCFC, IoWidth::Dword, AccessType::Write), Ok(()));
        // Straddles the end of the descriptor's range.
        assert_eq!(
            gate.is_io_allowed(0xCFE, IoWidth::Dword, AccessType::Read),
            Err(PolicyGateError::AccessDenied.into())
        );
        // Outside the descriptor entirely.
        assert_eq!(
            gate.is_io_allowed(0x60, IoWidth::Byte, AccessType::Read),
            Err(PolicyGateError::AccessDenied.into())
        );
    }

    #[test]
    fn test_is_io_allowed_honours_strict_width_descriptors() {
        let policy =
            PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0x70, 2, READ | STRICT)])).build();
        let gate = policy.gate();

        assert_eq!(gate.is_io_allowed(0x70, IoWidth::Word, AccessType::Read), Ok(()));
        // Strict width requires an exact address and size match.
        assert_eq!(
            gate.is_io_allowed(0x70, IoWidth::Byte, AccessType::Read),
            Err(PolicyGateError::AccessDenied.into())
        );
        assert_eq!(
            gate.is_io_allowed(0x71, IoWidth::Word, AccessType::Read),
            Err(PolicyGateError::AccessDenied.into())
        );
        // The descriptor grants read only.
        assert_eq!(
            gate.is_io_allowed(0x70, IoWidth::Word, AccessType::Write),
            Err(PolicyGateError::AccessDenied.into())
        );
    }

    #[test]
    fn test_is_io_allowed_inverts_on_a_deny_list() {
        let policy =
            PolicyBuilder::new().root(ACCESS_ATTR_DENY, Descriptors::Io(vec![io(0xB2, 1, READ | WRITE)])).build();
        let gate = policy.gate();

        assert_eq!(
            gate.is_io_allowed(0xB2, IoWidth::Byte, AccessType::Write),
            Err(PolicyGateError::AccessDenied.into())
        );
        assert_eq!(gate.is_io_allowed(0x60, IoWidth::Byte, AccessType::Write), Ok(()));
    }

    #[test]
    fn test_is_io_allowed_denies_any_overlap_with_a_denied_range() {
        let policy =
            PolicyBuilder::new().root(ACCESS_ATTR_DENY, Descriptors::Io(vec![io(0xD00, 2, READ | WRITE)])).build();
        let gate = policy.gate();

        // A DWORD at 0xCFE starts below the denied range but reaches 0xD00 and 0xD01.
        assert_eq!(
            gate.is_io_allowed(0xCFE, IoWidth::Dword, AccessType::Read),
            Err(PolicyGateError::AccessDenied.into())
        );
        // A DWORD at 0xD00 covers the whole denied range and more.
        assert_eq!(
            gate.is_io_allowed(0xD00, IoWidth::Dword, AccessType::Write),
            Err(PolicyGateError::AccessDenied.into())
        );
        // Accesses that end at, or start after, the range edges do not touch it.
        assert_eq!(gate.is_io_allowed(0xCFC, IoWidth::Dword, AccessType::Read), Ok(()));
        assert_eq!(gate.is_io_allowed(0xD02, IoWidth::Word, AccessType::Read), Ok(()));
    }

    #[test]
    fn test_is_io_allowed_fails_closed_on_an_unknown_access_attribute() {
        let policy = PolicyBuilder::new().root(0x7F, Descriptors::Io(vec![io(0x60, 1, READ)])).build();
        assert_eq!(
            policy.gate().is_io_allowed(0x60, IoWidth::Byte, AccessType::Read),
            Err(PolicyGateError::AccessDenied.into())
        );
    }

    #[test]
    fn test_is_msr_allowed() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![msr(0x1B, 4, READ)])).build();
        let gate = policy.gate();

        assert_eq!(gate.is_msr_allowed(0x1B, AccessType::Read), Ok(()));
        assert_eq!(gate.is_msr_allowed(0x1E, AccessType::Read), Ok(()));
        // One past the descriptor's range.
        assert_eq!(gate.is_msr_allowed(0x1F, AccessType::Read), Err(PolicyGateError::AccessDenied.into()));
        // The descriptor grants read only.
        assert_eq!(gate.is_msr_allowed(0x1B, AccessType::Write), Err(PolicyGateError::AccessDenied.into()));
        assert_eq!(gate.is_msr_allowed(0x1B, AccessType::Execute), Err(PolicyGateError::InvalidAccessMask.into()));
    }

    #[test]
    fn test_is_msr_allowed_without_a_policy_root_or_with_an_unknown_attribute() {
        let missing = PolicyBuilder::new().build();
        assert_eq!(
            missing.gate().is_msr_allowed(0x1B, AccessType::Read),
            Err(PolicyGateError::PolicyRootNotFound.into())
        );

        let deny = PolicyBuilder::new().root(ACCESS_ATTR_DENY, Descriptors::Msr(vec![msr(0x1B, 1, READ)])).build();
        assert_eq!(deny.gate().is_msr_allowed(0x1B, AccessType::Read), Err(PolicyGateError::AccessDenied.into()));
        assert_eq!(deny.gate().is_msr_allowed(0x20, AccessType::Read), Ok(()));

        let unknown = PolicyBuilder::new().root(0x7F, Descriptors::Msr(vec![msr(0x1B, 1, READ)])).build();
        assert_eq!(unknown.gate().is_msr_allowed(0x1B, AccessType::Read), Err(PolicyGateError::AccessDenied.into()));
    }

    #[test]
    fn test_is_msr_allowed_matches_the_last_msr_of_the_address_space() {
        // A range that ends exactly at the top of the MSR space covers its last MSR.
        let deny =
            PolicyBuilder::new().root(ACCESS_ATTR_DENY, Descriptors::Msr(vec![msr(0xFFFF_FF00, 0x100, READ)])).build();
        let gate = deny.gate();

        assert_eq!(gate.is_msr_allowed(0xFFFF_FFFE, AccessType::Read), Err(PolicyGateError::AccessDenied.into()));
        assert_eq!(gate.is_msr_allowed(0xFFFF_FFFF, AccessType::Read), Err(PolicyGateError::AccessDenied.into()));
        assert_eq!(gate.is_msr_allowed(0xFFFF_FEFF, AccessType::Read), Ok(()));
    }

    #[test]
    fn test_is_instruction_allowed() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Instruction(vec![instruction(Instruction::Hlt, EXECUTE)]))
            .build();
        let gate = policy.gate();

        assert_eq!(gate.is_instruction_allowed(Instruction::Hlt), Ok(()));
        assert_eq!(gate.is_instruction_allowed(Instruction::Cli), Err(PolicyGateError::AccessDenied.into()));
    }

    #[test]
    fn test_is_instruction_allowed_without_a_policy_root_or_with_an_unknown_attribute() {
        let missing = PolicyBuilder::new().build();
        assert_eq!(
            missing.gate().is_instruction_allowed(Instruction::Cli),
            Err(PolicyGateError::PolicyRootNotFound.into())
        );

        // A descriptor without the execute attribute never matches.
        let no_execute = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Instruction(vec![instruction(Instruction::Cli, READ)]))
            .build();
        assert_eq!(
            no_execute.gate().is_instruction_allowed(Instruction::Cli),
            Err(PolicyGateError::AccessDenied.into())
        );

        let deny = PolicyBuilder::new()
            .root(ACCESS_ATTR_DENY, Descriptors::Instruction(vec![instruction(Instruction::Cli, EXECUTE)]))
            .build();
        assert_eq!(deny.gate().is_instruction_allowed(Instruction::Cli), Err(PolicyGateError::AccessDenied.into()));
        assert_eq!(deny.gate().is_instruction_allowed(Instruction::Hlt), Ok(()));

        let unknown = PolicyBuilder::new()
            .root(0x7F, Descriptors::Instruction(vec![instruction(Instruction::Cli, EXECUTE)]))
            .build();
        assert_eq!(unknown.gate().is_instruction_allowed(Instruction::Cli), Err(PolicyGateError::AccessDenied.into()));
    }

    #[test]
    fn test_is_save_state_read_allowed_without_a_policy_root_allows_everything() {
        // No save-state root means the platform ships a level-20 policy.
        let policy = PolicyBuilder::new().build();
        assert_eq!(policy.gate().is_save_state_read_allowed(Some(SaveStateField::Rax), 8, None), Ok(()));
        assert_eq!(policy.gate().is_save_state_read_allowed(None, 8, None), Ok(()));
    }

    #[test]
    fn test_is_save_state_read_allowed_applies_the_root_default_without_a_field() {
        // A register with no policy field can never match a descriptor, even one that grants
        // every read, so only the root's access attribute decides the result.
        let descriptors = || {
            Descriptors::SaveState(vec![save_state(
                SaveStateField::Rax,
                RESOURCE_ATTR_READ,
                SaveStateCondition::Unconditional,
            )])
        };

        let allow = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, descriptors()).build();
        assert_eq!(
            allow.gate().is_save_state_read_allowed(None, 8, Some(SaveStateCondition::IoRead)),
            Err(PolicyGateError::AccessDenied.into())
        );

        let empty_allow = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::SaveState(vec![])).build();
        assert_eq!(
            empty_allow.gate().is_save_state_read_allowed(None, 8, None),
            Err(PolicyGateError::AccessDenied.into())
        );

        let deny = PolicyBuilder::new().root(ACCESS_ATTR_DENY, descriptors()).build();
        assert_eq!(deny.gate().is_save_state_read_allowed(None, 8, None), Ok(()));
    }

    #[test]
    fn test_is_save_state_read_allowed_matches_conditions() {
        let policy = PolicyBuilder::new()
            .root(
                ACCESS_ATTR_ALLOW,
                Descriptors::SaveState(vec![
                    save_state(SaveStateField::Rax, RESOURCE_ATTR_READ, SaveStateCondition::Unconditional),
                    save_state(SaveStateField::IoTrap, RESOURCE_ATTR_COND_READ, SaveStateCondition::IoRead),
                ]),
            )
            .build();
        let gate = policy.gate();

        assert_eq!(gate.is_save_state_read_allowed(Some(SaveStateField::Rax), 8, None), Ok(()));
        assert_eq!(
            gate.is_save_state_read_allowed(Some(SaveStateField::IoTrap), 4, Some(SaveStateCondition::IoRead)),
            Ok(())
        );
        // The conditional descriptor only covers I/O reads.
        assert_eq!(
            gate.is_save_state_read_allowed(Some(SaveStateField::IoTrap), 4, Some(SaveStateCondition::IoWrite)),
            Err(PolicyGateError::AccessDenied.into())
        );
        // A conditional descriptor never matches when no condition is supplied.
        assert_eq!(
            gate.is_save_state_read_allowed(Some(SaveStateField::IoTrap), 4, None),
            Err(PolicyGateError::AccessDenied.into())
        );
    }

    #[test]
    fn test_is_save_state_read_allowed_on_deny_and_unknown_lists() {
        let deny = PolicyBuilder::new()
            .root(
                ACCESS_ATTR_DENY,
                Descriptors::SaveState(vec![save_state(
                    SaveStateField::Rax,
                    RESOURCE_ATTR_READ,
                    SaveStateCondition::Unconditional,
                )]),
            )
            .build();
        assert_eq!(
            deny.gate().is_save_state_read_allowed(Some(SaveStateField::Rax), 8, None),
            Err(PolicyGateError::AccessDenied.into())
        );
        assert_eq!(deny.gate().is_save_state_read_allowed(Some(SaveStateField::IoTrap), 8, None), Ok(()));

        // A descriptor granting neither read nor conditional read never matches.
        let write_only = PolicyBuilder::new()
            .root(
                ACCESS_ATTR_ALLOW,
                Descriptors::SaveState(vec![save_state(
                    SaveStateField::Rax,
                    RESOURCE_ATTR_COND_WRITE,
                    SaveStateCondition::Unconditional,
                )]),
            )
            .build();
        assert_eq!(
            write_only.gate().is_save_state_read_allowed(Some(SaveStateField::Rax), 8, None),
            Err(PolicyGateError::AccessDenied.into())
        );

        let unknown = PolicyBuilder::new()
            .root(
                0x7F,
                Descriptors::SaveState(vec![save_state(
                    SaveStateField::Rax,
                    RESOURCE_ATTR_READ,
                    SaveStateCondition::Unconditional,
                )]),
            )
            .build();
        assert_eq!(
            unknown.gate().is_save_state_read_allowed(Some(SaveStateField::Rax), 8, None),
            Err(PolicyGateError::AccessDenied.into())
        );
    }

    #[test]
    fn test_is_io_allowed_accepts_an_access_that_ends_at_the_top_of_the_port_space() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0xFFFC, 4, READ), io(0, 1, READ)]))
            .build();
        let gate = policy.gate();

        // A WORD at 0xFFFE ends exactly at 0x10000, so it is still inside the port space.
        assert_eq!(gate.is_io_allowed(0xFFFE, IoWidth::Word, AccessType::Read), Ok(()));
        assert_eq!(gate.is_io_allowed(0xFFFF, IoWidth::Byte, AccessType::Read), Ok(()));
        assert_eq!(gate.is_io_allowed(0xFFFC, IoWidth::Dword, AccessType::Read), Ok(()));
        // One byte further runs past it.
        assert_eq!(
            gate.is_io_allowed(0xFFFF, IoWidth::Word, AccessType::Read),
            Err(PolicyGateError::InvalidIoRange.into())
        );
        // The first port is an ordinary port.
        assert_eq!(gate.is_io_allowed(0, IoWidth::Byte, AccessType::Read), Ok(()));
        assert_eq!(gate.is_io_allowed(0, IoWidth::Word, AccessType::Read), Err(PolicyGateError::AccessDenied.into()));
    }

    #[test]
    fn test_deny_list_descriptors_block_only_the_access_types_they_name() {
        // Deny lists commonly block writes to a resource while leaving it readable.
        let msr_policy =
            PolicyBuilder::new().root(ACCESS_ATTR_DENY, Descriptors::Msr(vec![msr(0x1F2, 2, WRITE)])).build();
        let msr_gate = msr_policy.gate();
        assert_eq!(msr_gate.is_msr_allowed(0x1F2, AccessType::Read), Ok(()));
        assert_eq!(msr_gate.is_msr_allowed(0x1F3, AccessType::Read), Ok(()));
        assert_eq!(msr_gate.is_msr_allowed(0x1F2, AccessType::Write), Err(PolicyGateError::AccessDenied.into()));
        assert_eq!(msr_gate.is_msr_allowed(0x1F3, AccessType::Write), Err(PolicyGateError::AccessDenied.into()));
        assert_eq!(msr_gate.is_msr_allowed(0x1F4, AccessType::Write), Ok(()));

        let io_policy = PolicyBuilder::new().root(ACCESS_ATTR_DENY, Descriptors::Io(vec![io(0xCF9, 1, WRITE)])).build();
        let io_gate = io_policy.gate();
        assert_eq!(io_gate.is_io_allowed(0xCF9, IoWidth::Byte, AccessType::Read), Ok(()));
        assert_eq!(
            io_gate.is_io_allowed(0xCF9, IoWidth::Byte, AccessType::Write),
            Err(PolicyGateError::AccessDenied.into())
        );
        // A DWORD write at 0xCF8 overlaps the denied port, so it is denied as well.
        assert_eq!(
            io_gate.is_io_allowed(0xCF8, IoWidth::Dword, AccessType::Write),
            Err(PolicyGateError::AccessDenied.into())
        );
        assert_eq!(io_gate.is_io_allowed(0xCF8, IoWidth::Dword, AccessType::Read), Ok(()));
    }

    #[test]
    fn test_is_save_state_read_allowed_denies_a_conditional_read_only_under_its_condition() {
        let policy = PolicyBuilder::new()
            .root(
                ACCESS_ATTR_DENY,
                Descriptors::SaveState(vec![save_state(
                    SaveStateField::Rax,
                    RESOURCE_ATTR_COND_READ,
                    SaveStateCondition::IoWrite,
                )]),
            )
            .build();
        let gate = policy.gate();

        assert_eq!(
            gate.is_save_state_read_allowed(Some(SaveStateField::Rax), 8, Some(SaveStateCondition::IoWrite)),
            Err(PolicyGateError::AccessDenied.into())
        );
        // Under another trap condition, or with none, the descriptor does not apply.
        assert_eq!(
            gate.is_save_state_read_allowed(Some(SaveStateField::Rax), 8, Some(SaveStateCondition::IoRead)),
            Ok(())
        );
        assert_eq!(gate.is_save_state_read_allowed(Some(SaveStateField::Rax), 8, None), Ok(()));
    }

    #[test]
    fn test_is_save_state_read_allowed_ignores_a_plain_read_that_names_a_condition() {
        // A plain read descriptor applies only with the unconditional marker. One that names a
        // trap condition is inconsistent, so it grants nothing, even under that condition.
        let policy = PolicyBuilder::new()
            .root(
                ACCESS_ATTR_ALLOW,
                Descriptors::SaveState(vec![save_state(
                    SaveStateField::Rax,
                    RESOURCE_ATTR_READ,
                    SaveStateCondition::IoRead,
                )]),
            )
            .build();
        let gate = policy.gate();

        assert_eq!(
            gate.is_save_state_read_allowed(Some(SaveStateField::Rax), 8, Some(SaveStateCondition::IoRead)),
            Err(PolicyGateError::AccessDenied.into())
        );
        assert_eq!(
            gate.is_save_state_read_allowed(Some(SaveStateField::Rax), 8, None),
            Err(PolicyGateError::AccessDenied.into())
        );
    }

    #[test]
    fn test_take_snapshot_requires_a_memory_policy_buffer() {
        let policy = PolicyBuilder::new().build();
        let gate = policy.gate();

        assert_eq!(
            // SAFETY: the buffer check fails before `cr3` is ever dereferenced.
            unsafe { gate.take_snapshot(0x1000, |_| false) },
            Err(PolicyGateError::InternalError.into())
        );
    }

    #[test]
    fn test_take_snapshot_rejects_a_second_snapshot() {
        let policy = PolicyBuilder::new().build();
        let gate = policy.gate();

        assert_eq!(gate.record_snapshot(3), 3);
        assert!(gate.is_locked());
        assert_eq!(gate.snapshot_count(), Some(3));
        // A second record does not overwrite the first.
        assert_eq!(gate.record_snapshot(7), 3);

        // SAFETY: the lock check fails before `cr3` is dereferenced.
        assert_eq!(unsafe { gate.take_snapshot(0x1000, |_| false) }, Err(PolicyGateError::AlreadyLocked.into()));
        assert_eq!(gate.snapshot_count(), Some(3));
    }

    #[test]
    fn test_set_memory_policy_buffer_rejects_an_unusable_buffer() {
        let policy = PolicyBuilder::new().build();
        let mut gate = policy.gate();
        let mut snapshot = [mem(0x1000, 0x1000, RESOURCE_ATTR_READ)];

        assert_eq!(
            gate.set_memory_policy_buffer(core::ptr::null_mut(), 1),
            Err(PolicyGateError::InvalidMemoryPolicyBuffer.into())
        );
        assert_eq!(
            gate.set_memory_policy_buffer(snapshot.as_mut_ptr(), 0),
            Err(PolicyGateError::InvalidMemoryPolicyBuffer.into())
        );

        // SAFETY: no buffer was stored, so the check fails before `cr3` is dereferenced.
        assert_eq!(unsafe { gate.take_snapshot(0x1000, |_| false) }, Err(PolicyGateError::InternalError.into()));
    }

    #[test]
    fn test_set_memory_policy_buffer_rejects_a_second_buffer() {
        let policy = PolicyBuilder::new().build();
        let mut gate = policy.gate();
        let mut first = [mem(0x1000, 0x1000, RESOURCE_ATTR_READ)];
        let mut second = [mem(0x2000, 0x1000, RESOURCE_ATTR_READ); 2];

        assert_eq!(gate.set_memory_policy_buffer(first.as_mut_ptr(), first.len()), Ok(()));
        assert_eq!(
            gate.set_memory_policy_buffer(second.as_mut_ptr(), second.len()),
            Err(PolicyGateError::MemoryPolicyBufferAlreadySet.into())
        );
        assert_eq!(gate.memory_policy_buffer, first.as_mut_ptr());
        assert_eq!(gate.memory_policy_max_count, first.len());
    }

    #[test]
    fn test_verify_snapshot_fails_closed_before_a_snapshot_is_taken() {
        let policy = PolicyBuilder::new().build();
        let gate = policy.gate();

        // Having nothing to compare against is not a pass: this is the routine that detects
        // page-table tampering after lock.
        assert_eq!(
            // SAFETY: the missing snapshot short-circuits before `cr3`/`scratch` are dereferenced.
            unsafe { gate.verify_snapshot(0x1000, |_| false, core::ptr::null_mut(), 0) },
            Err(PolicyGateError::InternalError.into())
        );
    }

    /// A page table with a writable data page and a read-only code page.
    fn snapshot_page_table() -> TestPageTable {
        TestPageTable::new(&[
            (0x10_0000, 0x1000, MemoryAttributes::ExecuteProtect),
            (0x20_0000, 0x1000, MemoryAttributes::ReadOnly),
        ])
    }

    #[test]
    fn test_take_snapshot_records_the_page_table_and_verify_detects_changes() {
        let mut table = snapshot_page_table();
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        let mut gate = policy.gate();
        let mut snapshot = [MemDescriptorV1_0::default(); 4];
        gate.set_memory_policy_buffer(snapshot.as_mut_ptr(), snapshot.len()).expect("buffer is usable");

        // SAFETY: `table` is a live 4-level table and the snapshot buffer holds 4 descriptors.
        assert_eq!(unsafe { gate.take_snapshot(table.cr3(), |_| false) }, Ok(2));
        assert!(gate.is_locked());
        assert_eq!(gate.snapshot_count(), Some(2));
        assert_eq!(
            snapshot[..2],
            [
                mem(0x10_0000, 0x1000, RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE),
                mem(0x20_0000, 0x1000, RESOURCE_ATTR_READ | RESOURCE_ATTR_EXECUTE),
            ]
        );

        let mut scratch = [MemDescriptorV1_0::default(); 4];
        // SAFETY: `table` is a live 4-level table and `scratch` holds 4 descriptors.
        let verify = |table: &TestPageTable, scratch: &mut [MemDescriptorV1_0]| unsafe {
            gate.verify_snapshot(table.cr3(), |_| false, scratch.as_mut_ptr(), scratch.len())
        };
        assert_eq!(verify(&table, &mut scratch), Ok(()));

        // Making the code page writable after lock keeps the descriptor count, so only the
        // per-descriptor comparison can catch it.
        table.map(0x20_0000, 0x1000, MemoryAttributes::empty());
        assert_eq!(verify(&table, &mut scratch), Err(PolicyGateError::AccessDenied.into()));
    }

    #[test]
    fn test_take_snapshot_leaves_the_gate_unlocked_when_the_walk_fails() {
        let table = snapshot_page_table();
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        let mut gate = policy.gate();
        let mut snapshot = [MemDescriptorV1_0::default(); 1];
        gate.set_memory_policy_buffer(snapshot.as_mut_ptr(), snapshot.len()).expect("buffer is usable");

        // The two mappings need two descriptors, one more than the buffer holds.
        // SAFETY: `table` is a live 4-level table and the snapshot buffer holds 1 descriptor.
        assert_eq!(unsafe { gate.take_snapshot(table.cr3(), |_| false) }, Err(PolicyGateError::InternalError.into()));
        assert!(!gate.is_locked());
        assert_eq!(gate.snapshot_count(), None);
    }

    #[test]
    fn test_verify_snapshot_fails_closed_when_the_walk_fails() {
        let table = snapshot_page_table();
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        let mut gate = policy.gate();
        let mut snapshot = [MemDescriptorV1_0::default(); 4];
        gate.set_memory_policy_buffer(snapshot.as_mut_ptr(), snapshot.len()).expect("buffer is usable");
        // SAFETY: `table` is a live 4-level table and the snapshot buffer holds 4 descriptors.
        assert_eq!(unsafe { gate.take_snapshot(table.cr3(), |_| false) }, Ok(2));

        // A scratch buffer too small for the unchanged table is an internal failure, not a pass.
        let mut scratch = [MemDescriptorV1_0::default(); 1];
        assert_eq!(
            // SAFETY: `table` is a live 4-level table and `scratch` holds 1 descriptor.
            unsafe { gate.verify_snapshot(table.cr3(), |_| false, scratch.as_mut_ptr(), scratch.len()) },
            Err(PolicyGateError::InternalError.into())
        );
        assert_eq!(
            // SAFETY: the null scratch buffer is rejected before the table is read.
            unsafe { gate.verify_snapshot(table.cr3(), |_| false, core::ptr::null_mut(), 4) },
            Err(PolicyGateError::InternalError.into())
        );
    }

    #[test]
    fn test_fetch_and_update_policy_rejects_a_policy_that_reports_no_size() {
        let mut policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        let mut gate = policy.gate();
        let mut snapshot = [mem(0x1000, 0x1000, RESOURCE_ATTR_READ)];
        gate.set_memory_policy_buffer(snapshot.as_mut_ptr(), snapshot.len()).expect("buffer is usable");
        gate.record_snapshot(snapshot.len());

        // The header size was valid when the gate was created. A size that reads as zero later
        // must not produce a policy holding only the memory descriptors.
        policy.as_mut_bytes()[4..8].copy_from_slice(&0u32.to_le_bytes());

        let mut dest = vec![0u8; 256];
        assert_eq!(
            // SAFETY: `dest` is writable for `dest.len()` bytes.
            unsafe { gate.fetch_and_update_policy(dest.as_mut_ptr(), dest.len()) },
            Err(PolicyGateError::InternalError.into())
        );
        assert!(dest.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn test_compare_snapshot() {
        let saved = [mem(0x1000, 0x1000, RESOURCE_ATTR_READ), mem(0x8000, 0x1000, RESOURCE_ATTR_WRITE)];

        assert_eq!(compare_snapshot(&saved, &saved), Ok(()));
        // A page table that grew or shrank since the snapshot.
        assert_eq!(compare_snapshot(&saved, &saved[..1]), Err(PolicyGateError::AccessDenied.into()));
        // Same shape, but an attribute changed under us.
        let tampered = [saved[0], mem(0x8000, 0x1000, RESOURCE_ATTR_EXECUTE)];
        assert_eq!(compare_snapshot(&saved, &tampered), Err(PolicyGateError::AccessDenied.into()));
    }

    #[test]
    fn test_fetch_and_update_policy_requires_a_snapshot() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        let gate = policy.gate();
        let mut dest = vec![0u8; 256];

        assert_eq!(
            // SAFETY: `dest` is writable for `dest.len()` bytes.
            unsafe { gate.fetch_and_update_policy(dest.as_mut_ptr(), dest.len()) },
            Err(PolicyGateError::InternalError.into())
        );
    }

    #[test]
    fn test_fetch_and_update_policy_rejects_a_small_destination() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        let mut gate = policy.gate();
        let mut snapshot = [mem(0x1000, 0x1000, RESOURCE_ATTR_READ)];
        gate.set_memory_policy_buffer(snapshot.as_mut_ptr(), snapshot.len()).expect("buffer is usable");
        gate.record_snapshot(snapshot.len());

        let mut dest = vec![0u8; 8];
        assert_eq!(
            // SAFETY: `dest` is writable for `dest.len()` bytes.
            unsafe { gate.fetch_and_update_policy(dest.as_mut_ptr(), dest.len()) },
            Err(PolicyGateError::BufferTooSmall.into())
        );
    }

    #[test]
    fn test_fetch_and_update_policy_requires_a_memory_policy_root() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![])).build();
        let mut gate = policy.gate();
        let mut snapshot = [mem(0x1000, 0x1000, RESOURCE_ATTR_READ)];
        gate.set_memory_policy_buffer(snapshot.as_mut_ptr(), snapshot.len()).expect("buffer is usable");
        gate.record_snapshot(0);

        let mut dest = vec![0u8; 256];
        assert_eq!(
            // SAFETY: `dest` is writable for `dest.len()` bytes.
            unsafe { gate.fetch_and_update_policy(dest.as_mut_ptr(), dest.len()) },
            Err(PolicyGateError::PolicyRootNotFound.into())
        );
    }

    #[test]
    fn test_gate_rejects_a_buffer_too_small_for_a_header() {
        test_support::init_test_logger();

        // The producer's reported size is what bounds every offset the blob carries, so a buffer
        // that cannot even hold the header is refused before the header is read.
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        // SAFETY: `policy` keeps an aligned, live buffer alive; the undersized length is rejected
        // before anything is dereferenced.
        let result = unsafe { PolicyGate::new(policy.as_ptr(), core::mem::size_of::<SecurePolicyDataV1_0>() - 1) };
        assert_eq!(result.err(), Some(PolicyGateError::MalformedPolicy.into()));
    }

    #[test]
    fn test_gate_rejects_a_policy_that_declares_no_size() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).declared_size(0).build();
        assert_eq!(policy.try_gate().err(), Some(PolicyGateError::MalformedPolicy.into()));
    }

    #[test]
    fn test_gate_rejects_a_policy_larger_than_its_buffer() {
        // A blob that claims more bytes than it was handed would make `fetch_and_update_policy`
        // copy adjacent MMRAM out to the non-MM caller.
        let policy =
            PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).declared_size(u32::MAX).build();
        assert_eq!(policy.try_gate().err(), Some(PolicyGateError::MalformedPolicy.into()));
    }

    #[test]
    fn test_gate_rejects_a_root_array_past_the_end_of_the_policy() {
        let policy =
            PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).declared_root_count(4096).build();
        assert_eq!(policy.try_gate().err(), Some(PolicyGateError::MalformedPolicy.into()));
    }

    #[test]
    fn test_gate_rejects_descriptors_past_the_end_of_the_policy() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![mem(0x1000, 0x1000, RESOURCE_ATTR_READ)]))
            .declared_descriptor_count(4096)
            .build();
        assert_eq!(policy.try_gate().err(), Some(PolicyGateError::MalformedPolicy.into()));
    }

    #[test]
    fn test_gate_rejects_a_descriptor_count_that_overflows_a_byte_count() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![mem(0x1000, 0x1000, RESOURCE_ATTR_READ)]))
            .declared_descriptor_count(u32::MAX)
            .build();
        assert_eq!(policy.try_gate().err(), Some(PolicyGateError::MalformedPolicy.into()));
    }

    #[test]
    fn test_gate_rejects_structures_that_are_not_aligned_for_their_type() {
        const ROOT_OFFSET_FIELD: usize = 32;
        const FIRST_ROOT: usize = core::mem::size_of::<SecurePolicyDataV1_0>();
        const DESCRIPTOR_OFFSET_FIELD: usize = FIRST_ROOT + 12;
        let write_u32 = |bytes: &mut [u8], at: usize, value: u32| {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        };

        // A buffer that does not start on the header's alignment.
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        // SAFETY: `policy` keeps its buffer alive, and the shifted view stays inside it.
        let shifted = unsafe { PolicyGate::new(policy.as_ptr().add(4), policy.len() - 4) };
        assert_eq!(shifted.err(), Some(PolicyGateError::MalformedPolicy.into()));

        // A root array offset that is in bounds but not aligned for a root.
        let mut policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![mem(0x1000, 0x1000, RESOURCE_ATTR_READ)]))
            .build();
        write_u32(policy.as_mut_bytes(), ROOT_OFFSET_FIELD, FIRST_ROOT as u32 + 1);
        assert_eq!(policy.try_gate().err(), Some(PolicyGateError::MalformedPolicy.into()));

        // Memory descriptors at an offset that is in bounds but not aligned for them.
        let mut policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        let descriptor_offset = u32::from_le_bytes(
            policy.as_mut_bytes()[DESCRIPTOR_OFFSET_FIELD..DESCRIPTOR_OFFSET_FIELD + 4].try_into().unwrap(),
        );
        write_u32(policy.as_mut_bytes(), DESCRIPTOR_OFFSET_FIELD, descriptor_offset - 4);
        assert_eq!(policy.try_gate().err(), Some(PolicyGateError::MalformedPolicy.into()));
    }

    #[test]
    fn test_gate_accepts_a_well_formed_policy() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![mem(0x1000, 0x1000, RESOURCE_ATTR_READ)]))
            .root(ACCESS_ATTR_DENY, Descriptors::Msr(vec![msr(0x1B, 1, RESOURCE_ATTR_READ as u16)]))
            .build();
        assert!(policy.try_gate().is_ok());
    }

    #[test]
    fn test_gate_rejects_a_root_of_an_unrecognized_type() {
        // With no descriptor size, the root's descriptors cannot be bounded, so no gate is built
        // over it. A type too large for `security_policy_check`'s duplicate mask is refused the
        // same way.
        for policy_type in [9, 64, 0x4242] {
            let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Unknown(policy_type)).build();
            assert_eq!(
                policy.try_gate().err(),
                Some(PolicyValidationError::UnrecognizedPolicyType { policy_type }.into()),
                "type {policy_type}"
            );
        }

        // Also when it follows a recognized root.
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![]))
            .root(ACCESS_ATTR_ALLOW, Descriptors::Unknown(9))
            .build();
        assert_eq!(
            policy.try_gate().err(),
            Some(PolicyValidationError::UnrecognizedPolicyType { policy_type: 9 }.into())
        );
    }

    #[test]
    fn test_fetch_and_update_policy_rejects_an_overflowing_descriptor_count() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        let gate = policy.gate();
        // A descriptor count this large cannot be converted to a byte count.
        gate.record_snapshot(usize::MAX);

        let mut dest = vec![0u8; 256];
        assert_eq!(
            // SAFETY: `dest` is writable for `dest.len()` bytes.
            unsafe { gate.fetch_and_update_policy(dest.as_mut_ptr(), dest.len()) },
            Err(PolicyGateError::InternalError.into())
        );
    }

    #[test]
    fn test_fetch_and_update_policy_rejects_an_overflowing_total_size() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        let gate = policy.gate();
        // The descriptor bytes alone fit, but appending them to the firmware policy does not.
        gate.record_snapshot(usize::MAX / size_of::<MemDescriptorV1_0>());

        let mut dest = vec![0u8; 256];
        assert_eq!(
            // SAFETY: `dest` is writable for `dest.len()` bytes.
            unsafe { gate.fetch_and_update_policy(dest.as_mut_ptr(), dest.len()) },
            Err(PolicyGateError::InternalError.into())
        );
    }

    #[test]
    fn test_fetch_and_update_policy_appends_the_snapshot_and_patches_the_header() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_DENY, Descriptors::Mem(vec![]))
            .root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0x60, 1, READ)]))
            .build();
        let firmware_size = policy.header().size as usize;

        let mut gate = policy.gate();
        let mut snapshot =
            [mem(0x1000, 0x1000, RESOURCE_ATTR_READ), mem(0x8000, 0x2000, RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE)];
        gate.set_memory_policy_buffer(snapshot.as_mut_ptr(), snapshot.len()).expect("buffer is usable");
        gate.record_snapshot(snapshot.len());

        let descriptor_bytes = snapshot.len() * size_of::<MemDescriptorV1_0>();
        let mut dest = vec![0u64; (firmware_size + descriptor_bytes).div_ceil(8)];
        let dest_bytes = dest.as_mut_slice().as_mut_bytes();

        // SAFETY: `dest_bytes` is writable for its whole length and is 8-byte aligned.
        let written = unsafe { gate.fetch_and_update_policy(dest_bytes.as_mut_ptr(), dest_bytes.len()) }
            .expect("policy fits in the destination");
        assert_eq!(written, firmware_size + descriptor_bytes);

        // SAFETY: `fetch_and_update_policy` wrote a complete, aligned policy blob into `dest_bytes`.
        let (header, roots) = unsafe {
            let header = &*dest_bytes.as_ptr().cast::<SecurePolicyDataV1_0>();
            (header, header.get_policy_roots())
        };
        assert_eq!(header.size as usize, written);
        assert_eq!(header.memory_policy_count, 0);

        let mem_root = roots.iter().find(|root| root.policy_type == TYPE_MEM).expect("memory policy root");
        assert_eq!(mem_root.access_attr, ACCESS_ATTR_ALLOW);
        assert_eq!(mem_root.offset as usize, firmware_size);
        assert_eq!(mem_root.count as usize, snapshot.len());

        // No MSR root appears, since the firmware policy declared none.
        assert!(!roots.iter().any(|root| root.policy_type == TYPE_MSR));
        // SAFETY: the patched memory root describes the descriptors appended above.
        let appended = unsafe { mem_root.get_mem_descriptors(dest_bytes.as_ptr()) };
        assert_eq!(appended, snapshot.as_slice());
    }

    #[test]
    fn test_fetch_and_update_policy_writes_to_an_unaligned_destination() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_DENY, Descriptors::Mem(vec![]))
            .root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0x60, 1, READ)]))
            .build();
        let firmware_size = policy.header().size as usize;

        let mut gate = policy.gate();
        let mut snapshot = [mem(0x1000, 0x1000, RESOURCE_ATTR_READ)];
        gate.set_memory_policy_buffer(snapshot.as_mut_ptr(), snapshot.len()).expect("buffer is usable");
        gate.record_snapshot(snapshot.len());

        let total = firmware_size + size_of::<MemDescriptorV1_0>();
        let mut storage = vec![0u64; total.div_ceil(8) + 1];
        // Start one byte past an 8-byte boundary, as a destination after a request header can.
        let dest_bytes = &mut storage.as_mut_slice().as_mut_bytes()[1..=total];

        // SAFETY: `dest_bytes` is writable for its whole length.
        let written = unsafe { gate.fetch_and_update_policy(dest_bytes.as_mut_ptr(), dest_bytes.len()) }
            .expect("policy fits in the destination");
        assert_eq!(written, total);

        // SAFETY: `fetch_and_update_policy` wrote a complete header at the start of `dest_bytes`.
        let header = unsafe { dest_bytes.as_ptr().cast::<SecurePolicyDataV1_0>().read_unaligned() };
        assert_eq!(header.size as usize, total);
        assert_eq!(header.memory_policy_count, 0);

        let root_size = size_of::<PolicyRootV1>();
        let mem_root = (0..header.policy_root_count as usize)
            .map(|index| {
                let offset = header.policy_root_offset as usize + index * root_size;
                // SAFETY: the roots array lies inside the policy copied into `dest_bytes`.
                unsafe { dest_bytes.as_ptr().add(offset).cast::<PolicyRootV1>().read_unaligned() }
            })
            .find(|root| root.policy_type == TYPE_MEM)
            .expect("memory policy root");
        assert_eq!(mem_root.access_attr, ACCESS_ATTR_ALLOW);
        assert_eq!(mem_root.offset as usize, firmware_size);
        assert_eq!(mem_root.count, 1);
    }
}
