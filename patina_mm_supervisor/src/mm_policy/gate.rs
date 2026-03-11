//! Policy Gate - Runtime access validation
//!
//! This module provides the `PolicyGate` struct that wraps a policy buffer
//! and provides methods to check if various operations are allowed.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use super::{
    ACCESS_ATTR_ALLOW, ACCESS_ATTR_DENY, AccessType, Instruction, IoWidth, MemDescriptorV1_0, PolicyRootV1,
    RESOURCE_ATTR_COND_READ, RESOURCE_ATTR_EXECUTE, RESOURCE_ATTR_READ, RESOURCE_ATTR_STRICT_WIDTH, SaveStateCondition,
    SaveStateField, SecurePolicyDataV1_0, TYPE_INSTRUCTION, TYPE_IO, TYPE_MEM, TYPE_MSR, TYPE_SAVE_STATE,
    helpers::{IsInsideMmramFn, walk_page_table},
};
use spin::Once;

/// Errors that can occur during policy gate operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyError {
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
    /// Access denied by policy.
    AccessDenied,
    /// Internal error during policy evaluation.
    InternalError,
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
    /// ## Safety
    ///
    /// The caller must ensure that `policy_ptr` points to a valid policy buffer
    /// that remains valid for the lifetime of this PolicyGate.
    pub unsafe fn new(policy_ptr: *const u8) -> Result<Self, PolicyError> {
        if policy_ptr.is_null() {
            return Err(PolicyError::NullPointer);
        }

        // SAFETY: `policy_ptr` is non-null (checked above) and, per this function's contract,
        // points to a valid policy buffer that outlives the gate, so reborrowing the header to
        // read its version is sound.
        let policy = unsafe { &*(policy_ptr as *const SecurePolicyDataV1_0) };
        if !policy.is_valid_version() {
            return Err(PolicyError::InvalidVersion);
        }

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
    /// the buffer address and size come from the PassDown HOB.
    ///
    /// ## Safety
    ///
    /// ## Safety Contract (deferred)
    ///
    /// The caller must ensure that `buffer` points to a valid memory region
    /// that can hold at least `max_count` `MemDescriptorV1_0` entries and that
    /// this memory remains valid for the lifetime of the `PolicyGate`.
    ///
    /// Storing the pointer is safe; the contract is enforced when the buffer
    /// is later dereferenced by [`take_snapshot`], [`verify_snapshot`], or
    /// [`fetch_n_update_policy`].
    pub fn set_memory_policy_buffer(&mut self, buffer: *mut MemDescriptorV1_0, max_count: usize) {
        self.memory_policy_buffer = buffer;
        self.memory_policy_max_count = max_count;
    }

    /// Gets a reference to the policy header.
    fn policy(&self) -> &SecurePolicyDataV1_0 {
        // SAFETY: Constructor validated the pointer
        unsafe { &*(self.policy_ptr as *const SecurePolicyDataV1_0) }
    }

    /// Finds a policy root by type.
    fn find_policy_root(&self, policy_type: u32) -> Option<&PolicyRootV1> {
        let policy = self.policy();
        // SAFETY: Constructor validated the policy
        let roots = unsafe { policy.get_policy_roots() };
        roots.iter().find(|r| r.policy_type == policy_type)
    }

    /// Checks if I/O access is allowed.
    ///
    /// `io_address` must be within the 16-bit I/O port space (`<= 0xFFFF`).
    pub fn is_io_allowed(&self, io_address: u32, width: IoWidth, access_type: AccessType) -> Result<(), PolicyError> {
        // Validate access type (must be read or write, not execute)
        if access_type == AccessType::Execute {
            return Err(PolicyError::InvalidAccessMask);
        }

        let io_size = width.size();

        // Validate I/O address range (16-bit port space)
        if io_address > u16::MAX as u32 {
            return Err(PolicyError::InvalidIoAddress);
        }

        // Check for overflow (MAX_UINT16 + 1 is valid for end address)
        if io_address.saturating_add(io_size) > (u16::MAX as u32) + 1 {
            return Err(PolicyError::InvalidIoRange);
        }

        let policy_root = match self.find_policy_root(TYPE_IO) {
            Some(root) => root,
            None => {
                log::warn!("Could not find IO policy root, denying access to be safe.");
                return Err(PolicyError::PolicyRootNotFound);
            }
        };

        // SAFETY: We validated the policy in the constructor
        let descriptors = unsafe { policy_root.get_io_descriptors(self.policy_ptr) };
        let access_mask = access_type.as_attr_mask();

        let mut found_match = false;

        for desc in descriptors {
            let desc_start = desc.io_address as u32;
            let desc_size = desc.length_or_width as u32;
            let is_strict_width = (desc.attributes as u32 & RESOURCE_ATTR_STRICT_WIDTH) != 0;

            if is_strict_width {
                // Strict width: address and size must match exactly
                if io_address == desc_start && io_size == desc_size {
                    // Check if the access type matches
                    if (desc.attributes as u32 & access_mask) != 0 {
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
                    if (desc.attributes as u32 & access_mask) != 0 {
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
            log::debug!("Rejecting IO access: port=0x{:x}, width={}, type={:?}", io_address, io_size, access_type);
            return Err(PolicyError::AccessDenied);
        }

        Ok(())
    }

    /// Checks if MSR access is allowed.
    pub fn is_msr_allowed(&self, msr_address: u32, access_type: AccessType) -> Result<(), PolicyError> {
        // Validate access type
        if access_type == AccessType::Execute {
            return Err(PolicyError::InvalidAccessMask);
        }

        let policy_root = match self.find_policy_root(TYPE_MSR) {
            Some(root) => root,
            None => {
                log::warn!("Could not find MSR policy root, denying access to be safe.");
                return Err(PolicyError::PolicyRootNotFound);
            }
        };

        // SAFETY: We validated the policy in the constructor
        let descriptors = unsafe { policy_root.get_msr_descriptors(self.policy_ptr) };
        let access_mask = access_type.as_attr_mask();

        let mut found_match = false;

        for desc in descriptors {
            let desc_start = desc.msr_address;
            let desc_end = desc_start.saturating_add(desc.length as u32);

            if msr_address >= desc_start && msr_address < desc_end && (desc.attributes as u32 & access_mask) != 0 {
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
            log::debug!("Rejecting MSR access: address=0x{:x}, type={:?}", msr_address, access_type);
            return Err(PolicyError::AccessDenied);
        }

        Ok(())
    }

    /// Checks if instruction execution is allowed.
    pub fn is_instruction_allowed(&self, instruction: Instruction) -> Result<(), PolicyError> {
        let instruction_index = instruction.as_index();

        if instruction_index >= Instruction::COUNT {
            return Err(PolicyError::InvalidInstructionIndex);
        }

        let policy_root = match self.find_policy_root(TYPE_INSTRUCTION) {
            Some(root) => root,
            None => {
                log::error!("Could not find Instruction policy root, denying access to be safe.");
                return Err(PolicyError::PolicyRootNotFound);
            }
        };

        // SAFETY: We validated the policy in the constructor
        let descriptors = unsafe { policy_root.get_instruction_descriptors(self.policy_ptr) };

        let mut found_match = false;

        for desc in descriptors {
            if instruction_index == desc.instruction_index && (desc.attributes as u32 & RESOURCE_ATTR_EXECUTE) != 0 {
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
            log::error!("Rejecting instruction execution: {:?}", instruction);
            return Err(PolicyError::AccessDenied);
        }

        Ok(())
    }

    /// Checks if save state read access is allowed.
    pub fn is_save_state_read_allowed(
        &self,
        field: SaveStateField,
        width: usize,
        current_condition: Option<SaveStateCondition>,
    ) -> Result<(), PolicyError> {
        let policy_root = match self.find_policy_root(TYPE_SAVE_STATE) {
            Some(root) => root,
            None => {
                // No save state policy = level 20, allow all reads
                log::error!("No save state policy root found, allowing read (level 20 policy).");
                return Ok(());
            }
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
            log::error!("Rejecting save state read: field={:?}, width={}", field, width);
            return Err(PolicyError::AccessDenied);
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
    /// * `cr3` must point to a valid, stable PML4 table.
    /// * The memory policy buffer (set via [`set_memory_policy_buffer`])
    ///   must still be valid and large enough.
    pub unsafe fn take_snapshot(&self, cr3: u64, is_inside_mmram: IsInsideMmramFn) -> Result<usize, PolicyError> {
        // Idempotent: if already locked, return the saved count.
        if let Some(&count) = self.snapshot_count.get() {
            return Ok(count);
        }

        if self.memory_policy_buffer.is_null() || self.memory_policy_max_count == 0 {
            log::error!("take_snapshot: memory policy buffer not configured");
            return Err(PolicyError::InternalError);
        }

        // SAFETY: The caller guarantees that `cr3` points to a valid PML4 and
        // that the memory policy buffer (set via `set_memory_policy_buffer`) is
        // valid and can hold `memory_policy_max_count` descriptors.
        let count =
            unsafe { walk_page_table(cr3, self.memory_policy_buffer, self.memory_policy_max_count, is_inside_mmram) }
                .map_err(|e| {
                log::error!("take_snapshot: walk_page_table failed: {:?}", e);
                PolicyError::InternalError
            })?;

        self.snapshot_count.call_once(|| count);
        log::info!("Policy snapshot taken: {} descriptors, ready-to-lock is now TRUE", count);
        Ok(count)
    }

    /// Verifies that the current page table still matches the saved snapshot.
    ///
    /// The caller must provide a scratch buffer (typically allocated from the
    /// page allocator) large enough to hold the walk results. This avoids
    /// overwriting the saved snapshot during comparison.
    ///
    /// Returns `Ok(())` if the tables match, or `Err(PolicyError::AccessDenied)`
    /// if they differ ("security violation").
    ///
    /// ## Safety
    ///
    /// * `cr3` must point to a valid, stable PML4 table.
    /// * `scratch` must point to a buffer of at least `scratch_max_count`
    ///   `MemDescriptorV1_0` entries.
    pub unsafe fn verify_snapshot(
        &self,
        cr3: u64,
        is_inside_mmram: IsInsideMmramFn,
        scratch: *mut MemDescriptorV1_0,
        scratch_max_count: usize,
    ) -> Result<(), PolicyError> {
        let saved_count = match self.snapshot_count.get() {
            Some(&c) => c,
            None => {
                log::warn!("verify_snapshot: no snapshot available, skipping verification");
                return Ok(());
            }
        };

        // SAFETY: The caller guarantees that `cr3` points to a valid PML4 and
        // that `scratch` can hold `scratch_max_count` descriptors.
        let fresh_count =
            unsafe { walk_page_table(cr3, scratch, scratch_max_count, is_inside_mmram) }.map_err(|e| {
                log::error!("verify_snapshot: walk_page_table failed: {:?}", e);
                PolicyError::InternalError
            })?;

        if fresh_count != saved_count {
            log::error!("verify_snapshot: descriptor count mismatch (saved={}, fresh={})", saved_count, fresh_count,);
            return Err(PolicyError::AccessDenied);
        }

        // View both buffers as slices so the comparison runs in safe code.
        let saved_ptr = self.memory_policy_buffer as *const MemDescriptorV1_0;
        // SAFETY: `walk_page_table` populated `scratch` with `fresh_count` descriptors; the saved
        // snapshot buffer was populated by a prior `take_snapshot` call with `saved_count`
        // (== `fresh_count`) entries.
        let (saved, fresh) = unsafe {
            (
                core::slice::from_raw_parts(saved_ptr, saved_count),
                core::slice::from_raw_parts(scratch as *const MemDescriptorV1_0, fresh_count),
            )
        };

        for (i, (saved, fresh)) in saved.iter().zip(fresh.iter()).enumerate() {
            if saved != fresh {
                log::error!(
                    "verify_snapshot: descriptor {} mismatch - \
                     saved=(base=0x{:x}, size=0x{:x}, attrs=0x{:x}) vs \
                     fresh=(base=0x{:x}, size=0x{:x}, attrs=0x{:x})",
                    i,
                    saved.base_address,
                    saved.size,
                    saved.mem_attributes,
                    fresh.base_address,
                    fresh.size,
                    fresh.mem_attributes,
                );
                return Err(PolicyError::AccessDenied);
            }
        }

        log::info!("verify_snapshot: page table matches saved snapshot ({} descriptors)", saved_count,);
        Ok(())
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
    /// * The `TYPE_MEM` policy root's `offset` → `fw_size` and `count` → snapshot count
    /// * The header's `size` → `fw_size + mem_policy_bytes`
    /// * The legacy `memory_policy_count` field is zeroed (unused with root-based layout)
    ///
    /// Note: the caller is responsible for writing/reserving any request header
    /// *before* the region pointed to by `dest`.
    ///
    /// ## Safety
    ///
    /// * `dest` must point to a writable buffer of at least `dest_size` bytes.
    pub unsafe fn fetch_n_update_policy(&self, dest: *mut u8, dest_size: usize) -> Result<usize, PolicyError> {
        let count = self.snapshot_count.get().copied().ok_or_else(|| {
            log::error!("fetch_n_update_policy: no snapshot taken");
            PolicyError::InternalError
        })?;

        let desc_size = core::mem::size_of::<MemDescriptorV1_0>();
        let mem_policy_bytes = count.checked_mul(desc_size).ok_or_else(|| {
            log::error!("fetch_n_update_policy: descriptor count overflow");
            PolicyError::InternalError
        })?;

        let fw_size = self.firmware_policy_size();
        if fw_size == 0 {
            log::error!("fetch_n_update_policy: firmware policy size is 0");
            return Err(PolicyError::InternalError);
        }

        let total_bytes = fw_size.checked_add(mem_policy_bytes).ok_or_else(|| {
            log::error!("fetch_n_update_policy: total size overflow");
            PolicyError::InternalError
        })?;

        if dest_size < total_bytes {
            log::error!("fetch_n_update_policy: buffer too small ({} bytes, need {})", dest_size, total_bytes,);
            return Err(PolicyError::InternalError);
        }

        // 1. Copy the firmware policy blob (header + payload), then append the
        //    memory policy descriptors after it.
        //
        // SAFETY: The caller guarantees that `dest` is writable for at least
        // `dest_size` bytes (verified >= `total_bytes` above). `self.policy_ptr`
        // points to a valid firmware policy blob of `fw_size` bytes (validated at
        // construction). The memory policy buffer holds `count` valid descriptors
        // from a prior `take_snapshot` call.
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
        // SAFETY: The header reports `root_count` `PolicyRootV1` entries at
        // `root_offset`, all within the `total_bytes` region copied above.
        let roots = unsafe { core::slice::from_raw_parts_mut(dest.add(root_offset) as *mut PolicyRootV1, root_count) };

        // Find the TYPE_MEM policy root and patch its offset/count.
        let Some(mem_root) = roots.iter_mut().find(|r| r.policy_type == TYPE_MEM) else {
            log::error!("fetch_n_update_policy: firmware policy has no TYPE_MEM policy root");
            return Err(PolicyError::PolicyRootNotFound);
        };
        mem_root.access_attr = ACCESS_ATTR_ALLOW;
        mem_root.offset = fw_size as u32;
        mem_root.count = count as u32;

        // 4. Update the total size and clear the legacy memory_policy_count.
        //
        // SAFETY: `dest` holds a valid `SecurePolicyDataV1_0` header (see above).
        let header = unsafe { &mut *(dest as *mut SecurePolicyDataV1_0) };
        header.size = total_bytes as u32;
        header.memory_policy_count = 0;

        log::info!(
            "fetch_n_update_policy: wrote {} bytes (fw_policy={}, mem_policy={} ({} descs))",
            total_bytes,
            fw_size,
            mem_policy_bytes,
            count,
        );
        Ok(total_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
