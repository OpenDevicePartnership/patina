//! Firmware Policy Gate
//!
//! Wraps the validated firmware policy blob and answers whether a given
//! operation is permitted: reading or writing an MSR or I/O port, executing a
//! privileged instruction, reading a save-state field, or touching a memory
//! range.
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
            Self::InternalError => write!(f, "an internal error occurred while evaluating the policy"),
        }
    }
}

/// Checks that every offset and count inside the policy blob describes a structure lying wholly
/// within `buffer_size` bytes.
///
/// The blob is firmware-supplied data whose fields are otherwise taken at face value by the
/// descriptor accessors, the per-syscall lookups and the `FETCH_POLICY` copy-out. Validating the
/// layout once here is what makes those unchecked `slice::from_raw_parts` calls sound.
///
/// ## Safety
///
/// `policy_ptr` must be non-null and point to `buffer_size` readable bytes.
unsafe fn validate_policy_layout(policy_ptr: *const u8, buffer_size: usize) -> MmSupervisorResult<()> {
    let header_size = core::mem::size_of::<SecurePolicyDataV1_0>();
    if buffer_size < header_size {
        log::error!("policy blob buffer is {buffer_size} bytes, too small for a {header_size}-byte header");
        return Err(PolicyGateError::MalformedPolicy.into());
    }

    // SAFETY: the buffer was just verified to hold a complete header, and the caller guarantees
    // `policy_ptr` is readable for `buffer_size` bytes.
    let policy = unsafe { &*policy_ptr.cast::<SecurePolicyDataV1_0>() };

    if !policy.is_valid_version() {
        log::error!(
            "Policy gate creation failed: unsupported policy version {}.{}",
            policy.version_major,
            policy.version_minor
        );
        return Err(PolicyGateError::InvalidVersion.into());
    }

    // Every offset below is bounded against the blob's own declared size, so that has to fit the
    // allocation first. `fetch_and_update_policy` also copies exactly this many bytes out.
    let declared = policy.size as usize;
    if declared < header_size || declared > buffer_size {
        log::error!("policy blob declares {declared} bytes in a {buffer_size}-byte buffer");
        return Err(PolicyGateError::MalformedPolicy.into());
    }

    let roots_end = (policy.policy_root_count as usize)
        .checked_mul(core::mem::size_of::<PolicyRootV1>())
        .and_then(|bytes| bytes.checked_add(policy.policy_root_offset as usize))
        .ok_or(PolicyGateError::MalformedPolicy)?;
    if roots_end > declared {
        log::error!(
            "policy root array ends at {roots_end}, past the {declared}-byte blob ({} roots at offset {})",
            policy.policy_root_count,
            policy.policy_root_offset
        );
        return Err(PolicyGateError::MalformedPolicy.into());
    }

    // SAFETY: the roots array was just verified to lie wholly inside the blob.
    let roots = unsafe { policy.get_policy_roots() };
    for root in roots {
        // A type the supervisor does not recognize is never looked up, so its descriptors are
        // never dereferenced and need no bounds.
        let Some(desc_size) = super::descriptor_size(root.policy_type) else {
            continue;
        };

        let descriptors_end = (root.count as usize)
            .checked_mul(desc_size)
            .and_then(|bytes| bytes.checked_add(root.offset as usize))
            .ok_or(PolicyGateError::MalformedPolicy)?;
        if descriptors_end > declared {
            log::error!(
                "policy type {} descriptors end at {descriptors_end}, past the {declared}-byte blob",
                root.policy_type
            );
            return Err(PolicyGateError::MalformedPolicy.into());
        }
    }

    Ok(())
}

/// Policy gate for runtime access validation.
///
/// This struct wraps a policy buffer and provides methods to check if
/// various operations (I/O, MSR, instruction, save state) are allowed.
pub struct PolicyGate {
    /// Pointer to the firmware policy data (static, read-only).
    policy_ptr: *const u8,
    /// Memory policy buffer (written by `walk_page_table` during snapshot).
    memory_policy_buffer: *mut MemDescriptorV1_0,
    /// Maximum number of `MemDescriptorV1_0` entries the memory policy buffer can hold.
    memory_policy_max_count: usize,
    /// Number of descriptors stored in the snapshot buffer.
    ///
    /// `None` means the ready-to-lock event has **not** occurred.
    /// `Some(count)` means a snapshot was taken with `count` entries.
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
    /// `buffer_size` is the size of the allocation `policy_ptr` names, as reported by the
    /// `PassDown` HOB. The blob's internal offsets are validated against it, which is what lets
    /// the descriptor accessors and the `FETCH_POLICY` copy-out trust those offsets later.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `policy_ptr` points to `buffer_size` readable bytes that
    /// remain valid for the lifetime of this `PolicyGate`.
    pub unsafe fn new(policy_ptr: *const u8, buffer_size: usize) -> MmSupervisorResult<Self> {
        if policy_ptr.is_null() {
            log::error!("Policy gate creation failed: policy buffer pointer is null");
            return Err(PolicyGateError::NullPointer.into());
        }

        // SAFETY: `policy_ptr` is non-null (checked above) and, per this function's contract,
        // points to `buffer_size` readable bytes that outlive the gate.
        unsafe { validate_policy_layout(policy_ptr, buffer_size) }?;

        Ok(Self {
            policy_ptr,
            memory_policy_buffer: core::ptr::null_mut(),
            memory_policy_max_count: 0,
            snapshot_count: Once::new(),
        })
    }

    /// Sets the memory policy buffer for page-table-derived snapshots.
    ///
    /// Must be called before [`take_snapshot`](Self::take_snapshot). Typically
    /// the buffer address and size come from the `PassDown` HOB.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `buffer` points to a valid memory region
    /// that can hold at least `max_count` `MemDescriptorV1_0` entries and that
    /// this memory remains valid for the lifetime of the `PolicyGate`.
    ///
    /// Storing the pointer is safe; the contract is enforced when the buffer
    /// is later dereferenced by [`take_snapshot`], [`verify_snapshot`], or
    /// [`fetch_and_update_policy`].
    pub fn set_memory_policy_buffer(&mut self, buffer: *mut MemDescriptorV1_0, max_count: usize) {
        self.memory_policy_buffer = buffer;
        self.memory_policy_max_count = max_count;
    }

    /// Gets a reference to the policy header.
    pub(crate) fn policy(&self) -> &SecurePolicyDataV1_0 {
        // SAFETY: Constructor validated the pointer
        unsafe { &*self.policy_ptr.cast::<SecurePolicyDataV1_0>() }
    }

    /// Returns the policy roots the blob declares.
    ///
    /// This is safe because [`PolicyGate::new`] proved that `policy_root_offset` and
    /// `policy_root_count` describe an array lying wholly inside the blob, and the buffer
    /// outlives the gate.
    pub(crate) fn policy_roots(&self) -> &[PolicyRootV1] {
        // SAFETY: the layout check in `new` established that the root array is in bounds.
        unsafe { self.policy().get_policy_roots() }
    }

    /// Finds a policy root by type.
    fn find_policy_root(&self, policy_type: u32) -> Option<&PolicyRootV1> {
        self.policy_roots().iter().find(|root| root.policy_type == policy_type)
    }

    /// Checks if I/O access is allowed.
    ///
    /// `io_address` must be within the 16-bit I/O port space (`<= 0xFFFF`).
    pub fn is_io_allowed(&self, io_address: u32, width: IoWidth, access_type: AccessType) -> MmSupervisorResult<()> {
        // Validate access type (must be read or write, not execute)
        if access_type == AccessType::Execute {
            log::error!("Rejecting IO access: port=0x{io_address:x} requested Execute access");
            return Err(PolicyGateError::InvalidAccessMask.into());
        }

        let io_size = width.size();

        // Validate I/O address range (16-bit port space)
        if io_address > u32::from(u16::MAX) {
            log::error!("Rejecting IO access: port 0x{io_address:x} is outside the 16-bit I/O space");
            return Err(PolicyGateError::InvalidIoAddress.into());
        }

        // Check for overflow (MAX_UINT16 + 1 is valid for end address)
        if io_address.saturating_add(io_size) > u32::from(u16::MAX) + 1 {
            log::error!("Rejecting IO access: port 0x{io_address:x} with width {io_size} runs past the I/O space");
            return Err(PolicyGateError::InvalidIoRange.into());
        }

        let policy_root = if let Some(root) = self.find_policy_root(TYPE_IO) {
            root
        } else {
            log::warn!("Could not find IO policy root, denying access to be safe.");
            return Err(PolicyGateError::PolicyRootNotFound.into());
        };

        // SAFETY: We validated the policy in the constructor
        let descriptors = unsafe { policy_root.get_io_descriptors(self.policy_ptr) };
        let access_mask = access_type.as_attr_mask();

        let mut found_match = false;

        for desc in descriptors {
            let desc_start = u32::from(desc.io_address);
            let desc_size = u32::from(desc.length_or_width);
            let is_strict_width = (u32::from(desc.attributes) & RESOURCE_ATTR_STRICT_WIDTH) != 0;

            if is_strict_width {
                // Strict width: address and size must match exactly
                if io_address == desc_start && io_size == desc_size {
                    // Check if the access type matches
                    if (u32::from(desc.attributes) & access_mask) != 0 {
                        found_match = true;
                        break;
                    }
                }
            } else {
                // Non-strict: check if our range is covered by this descriptor
                let desc_end = desc_start.saturating_add(desc_size);
                let our_end = io_address.saturating_add(io_size);

                if io_address >= desc_start && our_end <= desc_end {
                    // Check if the access type matches
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

    /// Checks if MSR access is allowed.
    pub fn is_msr_allowed(&self, msr_address: u32, access_type: AccessType) -> MmSupervisorResult<()> {
        // Validate access type
        if access_type == AccessType::Execute {
            log::error!("Rejecting MSR access: address=0x{msr_address:x} requested Execute access");
            return Err(PolicyGateError::InvalidAccessMask.into());
        }

        let policy_root = if let Some(root) = self.find_policy_root(TYPE_MSR) {
            root
        } else {
            log::warn!("Could not find MSR policy root, denying access to be safe.");
            return Err(PolicyGateError::PolicyRootNotFound.into());
        };

        // SAFETY: We validated the policy in the constructor
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

    /// Checks if instruction execution is allowed.
    pub fn is_instruction_allowed(&self, instruction: Instruction) -> MmSupervisorResult<()> {
        let instruction_index = instruction.as_index();

        if instruction_index >= Instruction::COUNT {
            log::error!(
                "Rejecting instruction execution: index {instruction_index} is outside 0..{}",
                Instruction::COUNT
            );
            return Err(PolicyGateError::InvalidInstructionIndex.into());
        }

        let policy_root = if let Some(root) = self.find_policy_root(TYPE_INSTRUCTION) {
            root
        } else {
            log::error!("Could not find Instruction policy root, denying access to be safe.");
            return Err(PolicyGateError::PolicyRootNotFound.into());
        };

        // SAFETY: We validated the policy in the constructor
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
            log::error!("Rejecting instruction execution: {instruction:?}");
            return Err(PolicyGateError::AccessDenied.into());
        }

        Ok(())
    }

    /// Checks if save state read access is allowed.
    pub fn is_save_state_read_allowed(
        &self,
        field: SaveStateField,
        width: usize,
        current_condition: Option<SaveStateCondition>,
    ) -> MmSupervisorResult<()> {
        let policy_root = if let Some(root) = self.find_policy_root(TYPE_SAVE_STATE) {
            root
        } else {
            // No save state policy = level 20, allow all reads
            log::error!("No save state policy root found, allowing read (level 20 policy).");
            return Ok(());
        };

        // SAFETY: We validated the policy in the constructor
        let descriptors = unsafe { policy_root.get_save_state_descriptors(self.policy_ptr) };

        let mut found_match = false;

        // Only RAX / IO_TRAP have a policy field; every other register has
        // `field == None` and therefore matches no descriptor.
        for desc in descriptors {
            if desc.map_field == field.as_index() {
                // Check if this is a read-allowed policy
                let is_read = (desc.attributes & RESOURCE_ATTR_READ) != 0;
                let is_cond_read = (desc.attributes & RESOURCE_ATTR_COND_READ) != 0;

                if is_read || is_cond_read {
                    // Check condition if this is conditional read
                    if is_cond_read {
                        if let Some(current) = current_condition
                            && desc.access_condition == current as u32
                        {
                            found_match = true;
                            break;
                        }
                        // Condition doesn't match, continue looking
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
            log::error!("Rejecting save state read: field={field:?}, width={width}");
            return Err(PolicyGateError::AccessDenied.into());
        }

        Ok(())
    }

    /// Gets the raw policy pointer.
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
    ///
    /// Returns `0` if the policy pointer is null (should not happen after construction).
    pub fn firmware_policy_size(&self) -> usize {
        self.policy().size as usize
    }

    /// Takes a page-table memory policy snapshot and transitions to the locked
    /// state.
    ///
    /// Walks the active page table, writes the resulting descriptors into the
    /// memory policy buffer, and atomically saves the descriptor count. After
    /// this call, [`is_locked`](Self::is_locked) returns `true`.
    ///
    /// If the gate is already locked, the snapshot is **not** re-taken and the
    /// existing descriptor count is returned.
    ///
    /// ## Safety
    ///
    /// - `cr3` must point to a valid, stable PML4 table.
    /// - The memory policy buffer (set via [`set_memory_policy_buffer`])
    ///   must still be valid and large enough.
    pub unsafe fn take_snapshot(&self, cr3: u64, is_inside_mmram: IsInsideMmramFn) -> MmSupervisorResult<usize> {
        // Idempotent: if already locked, return the saved count.
        if let Some(&count) = self.snapshot_count.get() {
            return Ok(count);
        }

        if self.memory_policy_buffer.is_null() || self.memory_policy_max_count == 0 {
            log::error!("take_snapshot: memory policy buffer not configured");
            return Err(PolicyGateError::InternalError.into());
        }

        // SAFETY: The caller guarantees that `cr3` points to a valid PML4 and
        // that the memory policy buffer (set via `set_memory_policy_buffer`) is
        // valid and can hold `memory_policy_max_count` descriptors.
        let count =
            unsafe { walk_page_table(cr3, self.memory_policy_buffer, self.memory_policy_max_count, is_inside_mmram) }
                .map_err(|err| {
                log::error!("take_snapshot: walk_page_table failed: {err:?}");
                PolicyGateError::InternalError
            })?;

        Ok(self.record_snapshot(count))
    }

    /// Records that the memory policy buffer now holds `count` descriptors and
    /// transitions the gate to the locked state.
    fn record_snapshot(&self, count: usize) -> usize {
        let count = *self.snapshot_count.call_once(|| count);
        log::info!("Policy snapshot taken: {count} descriptors, ready-to-lock is now TRUE");
        count
    }

    /// Verifies that the current page table still matches the saved snapshot.
    ///
    /// The caller must provide a scratch buffer (typically allocated from the
    /// page allocator) large enough to hold the walk results. This avoids
    /// overwriting the saved snapshot during comparison.
    ///
    /// Returns `Ok(())` only when a snapshot exists and the tables match it.
    /// Returns `Err(PolicyGateError::AccessDenied.into())` if they differ ("security violation"), and
    /// `Err(PolicyGateError::InternalError.into())` if there is no snapshot to verify against - this
    /// routine is what detects tampering, so "nothing to compare" must never read as a pass.
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

        // SAFETY: The caller guarantees that `cr3` points to a valid PML4 and
        // that `scratch` can hold `scratch_max_count` descriptors.
        let fresh_count =
            unsafe { walk_page_table(cr3, scratch, scratch_max_count, is_inside_mmram) }.map_err(|err| {
                log::error!("verify_snapshot: walk_page_table failed: {err:?}");
                PolicyGateError::InternalError
            })?;

        // View both buffers as slices so the comparison runs in safe code.
        //
        // SAFETY: `walk_page_table` populated `scratch` with `fresh_count` descriptors; the saved
        // snapshot buffer was populated by a prior `take_snapshot` call with `saved_count` entries.
        let (saved, fresh) = unsafe {
            (
                core::slice::from_raw_parts(self.memory_policy_buffer.cast_const(), saved_count),
                core::slice::from_raw_parts(scratch.cast_const(), fresh_count),
            )
        };

        compare_snapshot(saved, fresh)
    }

    /// Writes the merged firmware + memory policy into `dest` and returns the total
    /// number of bytes written.
    ///
    /// Mirrors the C `FetchNUpdateSecurityPolicy` function. The caller is
    /// responsible for ensuring the snapshot has been taken first (via
    /// [`take_snapshot`](Self::take_snapshot)).
    ///
    /// ## Layout written to `dest`
    ///
    /// ```text
    /// |--------------------------------------|
    /// | SecurePolicyDataV1_0 + payload       |  <- firmware policy blob (copied first)
    /// |--------------------------------------|
    /// | MemDescriptorV1_0[0..N]              |  <- memory policy snapshot (appended)
    /// |--------------------------------------|
    /// ```
    ///
    /// After the copy the function patches the header in-place:
    ///
    /// * The `TYPE_MEM` policy root's `offset` -> `fw_size` and `count` -> snapshot count
    /// * The header's `size` -> `fw_size + mem_policy_bytes`
    /// * The legacy `memory_policy_count` field is zeroed (unused with root-based layout)
    ///
    /// Note: the caller is responsible for writing/reserving any request header
    /// *before* the region pointed to by `dest`.
    ///
    /// ## Safety
    ///
    /// * `dest` must point to a writable buffer of at least `dest_size` bytes.
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
            return Err(PolicyGateError::InternalError.into());
        }

        // 1. Copy the firmware policy blob (header + payload), then append the
        //    memory policy descriptors after it.
        //
        // SAFETY: The caller guarantees that `dest` is writable for at least
        // `dest_size` bytes (verified >= `total_bytes` above). `self.policy_ptr` names a blob
        // whose declared `size` was validated at construction to fit its own buffer, so reading
        // `fw_size` bytes from it stays inside that buffer. The memory policy buffer holds
        // `count` valid descriptors from a prior `take_snapshot` call.
        unsafe {
            core::ptr::copy_nonoverlapping(self.policy_ptr, dest, fw_size);
            if mem_policy_bytes > 0 {
                let src = self.memory_policy_buffer as *const u8;
                core::ptr::copy_nonoverlapping(src, dest.add(fw_size), mem_policy_bytes);
            }
        }

        // 2. Read the root table location from the freshly-copied header.
        let (root_offset, root_count) = {
            // SAFETY: `dest` now holds a valid `SecurePolicyDataV1_0` header copied from the
            // validated firmware policy blob above.
            let header = unsafe { &*(dest as *const SecurePolicyDataV1_0) };
            (header.policy_root_offset as usize, header.policy_root_count as usize)
        };

        // 3. View the policy roots as a slice so the lookup/patch can be done in
        //    safe code.
        //
        // SAFETY: `root_offset` and `root_count` came from the header just copied from the blob,
        // and construction validated that the roots array they describe ends within the blob's
        // declared size - which is `fw_size`, itself within the `total_bytes` copied above.
        let roots =
            unsafe { core::slice::from_raw_parts_mut(dest.add(root_offset).cast::<PolicyRootV1>(), root_count) };

        // Find the TYPE_MEM policy root and patch its offset/count.
        let Some(mem_root) = roots.iter_mut().find(|root| root.policy_type == TYPE_MEM) else {
            log::error!("fetch_and_update_policy: firmware policy has no TYPE_MEM policy root");
            return Err(PolicyGateError::PolicyRootNotFound.into());
        };
        mem_root.access_attr = ACCESS_ATTR_ALLOW;
        mem_root.offset = fw_size as u32;
        mem_root.count = count as u32;

        // 4. Update the total size and clear the legacy memory_policy_count.
        //
        // SAFETY: `dest` holds a valid `SecurePolicyDataV1_0` header (see above).
        let header = unsafe { &mut *dest.cast::<SecurePolicyDataV1_0>() };
        header.size = total_bytes as u32;
        header.memory_policy_count = 0;

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
