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
    /// The policy blob's internal offsets do not describe a structure inside its own buffer.
    MalformedPolicy,
    /// Access denied by policy.
    AccessDenied,
    /// Internal error during policy evaluation.
    InternalError,
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
unsafe fn validate_policy_layout(policy_ptr: *const u8, buffer_size: usize) -> Result<(), PolicyError> {
    let header_size = core::mem::size_of::<SecurePolicyDataV1_0>();
    if buffer_size < header_size {
        log::error!("policy blob buffer is {buffer_size} bytes, too small for a {header_size}-byte header");
        return Err(PolicyError::MalformedPolicy);
    }

    // SAFETY: the buffer was just verified to hold a complete header, and the caller guarantees
    // `policy_ptr` is readable for `buffer_size` bytes.
    let policy = unsafe { &*(policy_ptr as *const SecurePolicyDataV1_0) };

    if !policy.is_valid_version() {
        return Err(PolicyError::InvalidVersion);
    }

    // Every offset below is bounded against the blob's own declared size, so that has to fit the
    // allocation first. `fetch_n_update_policy` also copies exactly this many bytes out.
    let declared = policy.size as usize;
    if declared < header_size || declared > buffer_size {
        log::error!("policy blob declares {declared} bytes in a {buffer_size}-byte buffer");
        return Err(PolicyError::MalformedPolicy);
    }

    let roots_end = (policy.policy_root_count as usize)
        .checked_mul(core::mem::size_of::<PolicyRootV1>())
        .and_then(|bytes| bytes.checked_add(policy.policy_root_offset as usize))
        .ok_or(PolicyError::MalformedPolicy)?;
    if roots_end > declared {
        log::error!(
            "policy root array ends at {roots_end}, past the {declared}-byte blob ({} roots at offset {})",
            policy.policy_root_count,
            policy.policy_root_offset
        );
        return Err(PolicyError::MalformedPolicy);
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
            .ok_or(PolicyError::MalformedPolicy)?;
        if descriptors_end > declared {
            log::error!(
                "policy type {} descriptors end at {descriptors_end}, past the {declared}-byte blob",
                root.policy_type
            );
            return Err(PolicyError::MalformedPolicy);
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
    pub unsafe fn new(policy_ptr: *const u8, buffer_size: usize) -> Result<Self, PolicyError> {
        if policy_ptr.is_null() {
            return Err(PolicyError::NullPointer);
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
        if io_address > u32::from(u16::MAX) {
            return Err(PolicyError::InvalidIoAddress);
        }

        // Check for overflow (MAX_UINT16 + 1 is valid for end address)
        if io_address.saturating_add(io_size) > u32::from(u16::MAX) + 1 {
            return Err(PolicyError::InvalidIoRange);
        }

        let policy_root = if let Some(root) = self.find_policy_root(TYPE_IO) {
            root
        } else {
            log::warn!("Could not find IO policy root, denying access to be safe.");
            return Err(PolicyError::PolicyRootNotFound);
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

        let policy_root = if let Some(root) = self.find_policy_root(TYPE_MSR) {
            root
        } else {
            log::warn!("Could not find MSR policy root, denying access to be safe.");
            return Err(PolicyError::PolicyRootNotFound);
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

        let policy_root = if let Some(root) = self.find_policy_root(TYPE_INSTRUCTION) {
            root
        } else {
            log::error!("Could not find Instruction policy root, denying access to be safe.");
            return Err(PolicyError::PolicyRootNotFound);
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
                log::error!("take_snapshot: walk_page_table failed: {e:?}");
                PolicyError::InternalError
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
    /// Returns `Err(PolicyError::AccessDenied)` if they differ ("security violation"), and
    /// `Err(PolicyError::InternalError)` if there is no snapshot to verify against - this
    /// routine is what detects tampering, so "nothing to compare" must never read as a pass.
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
        let Some(&saved_count) = self.snapshot_count.get() else {
            log::error!("verify_snapshot: no snapshot to verify against");
            return Err(PolicyError::InternalError);
        };

        // SAFETY: The caller guarantees that `cr3` points to a valid PML4 and
        // that `scratch` can hold `scratch_max_count` descriptors.
        let fresh_count =
            unsafe { walk_page_table(cr3, scratch, scratch_max_count, is_inside_mmram) }.map_err(|e| {
                log::error!("verify_snapshot: walk_page_table failed: {e:?}");
                PolicyError::InternalError
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
            log::error!("fetch_n_update_policy: buffer too small ({dest_size} bytes, need {total_bytes})");
            return Err(PolicyError::InternalError);
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
            "fetch_n_update_policy: wrote {total_bytes} bytes (fw_policy={fw_size}, mem_policy={mem_policy_bytes} ({count} descs))",
        );
        Ok(total_bytes)
    }
}

/// Returns an error when a freshly walked page table no longer matches the recorded snapshot.
fn compare_snapshot(saved: &[MemDescriptorV1_0], fresh: &[MemDescriptorV1_0]) -> Result<(), PolicyError> {
    if saved.len() != fresh.len() {
        log::error!("verify_snapshot: descriptor count mismatch (saved={}, fresh={})", saved.len(), fresh.len());
        return Err(PolicyError::AccessDenied);
    }

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

    log::info!("verify_snapshot: page table matches saved snapshot ({} descriptors)", saved.len());
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::super::test_support::{Descriptors, PolicyBuilder, instruction, io, mem, msr, save_state};
    use super::super::{RESOURCE_ATTR_COND_WRITE, RESOURCE_ATTR_WRITE, TYPE_MSR};
    use super::*;
    use zerocopy::IntoBytes;

    const READ: u16 = RESOURCE_ATTR_READ as u16;
    const WRITE: u16 = RESOURCE_ATTR_WRITE as u16;
    const EXECUTE: u16 = RESOURCE_ATTR_EXECUTE as u16;
    const STRICT: u16 = RESOURCE_ATTR_STRICT_WIDTH as u16;

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
        // SAFETY: `new` checks for null before dereferencing.
        assert_eq!(unsafe { PolicyGate::new(core::ptr::null(), 0) }.err(), Some(PolicyError::NullPointer));

        let wrong_version = PolicyBuilder::new().version(2, 0).build();
        assert_eq!(
            // SAFETY: the builder produced an aligned, fully-initialized header that outlives the call.
            unsafe { PolicyGate::new(wrong_version.as_ptr(), wrong_version.len()) }.err(),
            Some(PolicyError::InvalidVersion)
        );
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
        assert_eq!(gate.is_io_allowed(0x60, IoWidth::Byte, AccessType::Execute), Err(PolicyError::InvalidAccessMask));
        // Ports live in the 16-bit space.
        assert_eq!(gate.is_io_allowed(0x1_0000, IoWidth::Byte, AccessType::Read), Err(PolicyError::InvalidIoAddress));
        // A width that runs off the end of the port space.
        assert_eq!(gate.is_io_allowed(0xFFFF, IoWidth::Dword, AccessType::Read), Err(PolicyError::InvalidIoRange));
    }

    #[test]
    fn test_is_io_allowed_requires_a_policy_root() {
        let policy = PolicyBuilder::new().build();
        assert_eq!(
            policy.gate().is_io_allowed(0x60, IoWidth::Byte, AccessType::Read),
            Err(PolicyError::PolicyRootNotFound)
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
        assert_eq!(gate.is_io_allowed(0xCFE, IoWidth::Dword, AccessType::Read), Err(PolicyError::AccessDenied));
        // Outside the descriptor entirely.
        assert_eq!(gate.is_io_allowed(0x60, IoWidth::Byte, AccessType::Read), Err(PolicyError::AccessDenied));
    }

    #[test]
    fn test_is_io_allowed_honours_strict_width_descriptors() {
        let policy =
            PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0x70, 2, READ | STRICT)])).build();
        let gate = policy.gate();

        assert_eq!(gate.is_io_allowed(0x70, IoWidth::Word, AccessType::Read), Ok(()));
        // Strict width requires an exact address and size match.
        assert_eq!(gate.is_io_allowed(0x70, IoWidth::Byte, AccessType::Read), Err(PolicyError::AccessDenied));
        assert_eq!(gate.is_io_allowed(0x71, IoWidth::Word, AccessType::Read), Err(PolicyError::AccessDenied));
        // The descriptor grants read only.
        assert_eq!(gate.is_io_allowed(0x70, IoWidth::Word, AccessType::Write), Err(PolicyError::AccessDenied));
    }

    #[test]
    fn test_is_io_allowed_inverts_on_a_deny_list() {
        let policy =
            PolicyBuilder::new().root(ACCESS_ATTR_DENY, Descriptors::Io(vec![io(0xB2, 1, READ | WRITE)])).build();
        let gate = policy.gate();

        assert_eq!(gate.is_io_allowed(0xB2, IoWidth::Byte, AccessType::Write), Err(PolicyError::AccessDenied));
        assert_eq!(gate.is_io_allowed(0x60, IoWidth::Byte, AccessType::Write), Ok(()));
    }

    #[test]
    fn test_is_io_allowed_fails_closed_on_an_unknown_access_attribute() {
        let policy = PolicyBuilder::new().root(0x7F, Descriptors::Io(vec![io(0x60, 1, READ)])).build();
        assert_eq!(policy.gate().is_io_allowed(0x60, IoWidth::Byte, AccessType::Read), Err(PolicyError::AccessDenied));
    }

    #[test]
    fn test_is_msr_allowed() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![msr(0x1B, 4, READ)])).build();
        let gate = policy.gate();

        assert_eq!(gate.is_msr_allowed(0x1B, AccessType::Read), Ok(()));
        assert_eq!(gate.is_msr_allowed(0x1E, AccessType::Read), Ok(()));
        // One past the descriptor's range.
        assert_eq!(gate.is_msr_allowed(0x1F, AccessType::Read), Err(PolicyError::AccessDenied));
        // The descriptor grants read only.
        assert_eq!(gate.is_msr_allowed(0x1B, AccessType::Write), Err(PolicyError::AccessDenied));
        assert_eq!(gate.is_msr_allowed(0x1B, AccessType::Execute), Err(PolicyError::InvalidAccessMask));
    }

    #[test]
    fn test_is_msr_allowed_without_a_policy_root_or_with_an_unknown_attribute() {
        let missing = PolicyBuilder::new().build();
        assert_eq!(missing.gate().is_msr_allowed(0x1B, AccessType::Read), Err(PolicyError::PolicyRootNotFound));

        let deny = PolicyBuilder::new().root(ACCESS_ATTR_DENY, Descriptors::Msr(vec![msr(0x1B, 1, READ)])).build();
        assert_eq!(deny.gate().is_msr_allowed(0x1B, AccessType::Read), Err(PolicyError::AccessDenied));
        assert_eq!(deny.gate().is_msr_allowed(0x20, AccessType::Read), Ok(()));

        let unknown = PolicyBuilder::new().root(0x7F, Descriptors::Msr(vec![msr(0x1B, 1, READ)])).build();
        assert_eq!(unknown.gate().is_msr_allowed(0x1B, AccessType::Read), Err(PolicyError::AccessDenied));
    }

    #[test]
    fn test_is_instruction_allowed() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Instruction(vec![instruction(Instruction::Hlt, EXECUTE)]))
            .build();
        let gate = policy.gate();

        assert_eq!(gate.is_instruction_allowed(Instruction::Hlt), Ok(()));
        assert_eq!(gate.is_instruction_allowed(Instruction::Cli), Err(PolicyError::AccessDenied));
    }

    #[test]
    fn test_is_instruction_allowed_without_a_policy_root_or_with_an_unknown_attribute() {
        let missing = PolicyBuilder::new().build();
        assert_eq!(missing.gate().is_instruction_allowed(Instruction::Cli), Err(PolicyError::PolicyRootNotFound));

        // A descriptor without the execute attribute never matches.
        let no_execute = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Instruction(vec![instruction(Instruction::Cli, READ)]))
            .build();
        assert_eq!(no_execute.gate().is_instruction_allowed(Instruction::Cli), Err(PolicyError::AccessDenied));

        let deny = PolicyBuilder::new()
            .root(ACCESS_ATTR_DENY, Descriptors::Instruction(vec![instruction(Instruction::Cli, EXECUTE)]))
            .build();
        assert_eq!(deny.gate().is_instruction_allowed(Instruction::Cli), Err(PolicyError::AccessDenied));
        assert_eq!(deny.gate().is_instruction_allowed(Instruction::Hlt), Ok(()));

        let unknown = PolicyBuilder::new()
            .root(0x7F, Descriptors::Instruction(vec![instruction(Instruction::Cli, EXECUTE)]))
            .build();
        assert_eq!(unknown.gate().is_instruction_allowed(Instruction::Cli), Err(PolicyError::AccessDenied));
    }

    #[test]
    fn test_is_save_state_read_allowed_without_a_policy_root_allows_everything() {
        // No save-state root means the platform ships a level-20 policy.
        let policy = PolicyBuilder::new().build();
        assert_eq!(policy.gate().is_save_state_read_allowed(SaveStateField::Rax, 8, None), Ok(()));
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

        assert_eq!(gate.is_save_state_read_allowed(SaveStateField::Rax, 8, None), Ok(()));
        assert_eq!(
            gate.is_save_state_read_allowed(SaveStateField::IoTrap, 4, Some(SaveStateCondition::IoRead)),
            Ok(())
        );
        // The conditional descriptor only covers I/O reads.
        assert_eq!(
            gate.is_save_state_read_allowed(SaveStateField::IoTrap, 4, Some(SaveStateCondition::IoWrite)),
            Err(PolicyError::AccessDenied)
        );
        // A conditional descriptor never matches when no condition is supplied.
        assert_eq!(gate.is_save_state_read_allowed(SaveStateField::IoTrap, 4, None), Err(PolicyError::AccessDenied));
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
            deny.gate().is_save_state_read_allowed(SaveStateField::Rax, 8, None),
            Err(PolicyError::AccessDenied)
        );
        assert_eq!(deny.gate().is_save_state_read_allowed(SaveStateField::IoTrap, 8, None), Ok(()));

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
            write_only.gate().is_save_state_read_allowed(SaveStateField::Rax, 8, None),
            Err(PolicyError::AccessDenied)
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
            unknown.gate().is_save_state_read_allowed(SaveStateField::Rax, 8, None),
            Err(PolicyError::AccessDenied)
        );
    }

    #[test]
    fn test_take_snapshot_requires_a_memory_policy_buffer() {
        let policy = PolicyBuilder::new().build();
        let gate = policy.gate();

        // SAFETY: the buffer check fails before `cr3` is ever dereferenced.
        assert_eq!(unsafe { gate.take_snapshot(0x1000, |_, _| false) }, Err(PolicyError::InternalError));
    }

    #[test]
    fn test_take_snapshot_is_idempotent_once_recorded() {
        let policy = PolicyBuilder::new().build();
        let gate = policy.gate();

        assert_eq!(gate.record_snapshot(3), 3);
        assert!(gate.is_locked());
        assert_eq!(gate.snapshot_count(), Some(3));
        // A second record does not overwrite the first.
        assert_eq!(gate.record_snapshot(7), 3);

        // SAFETY: the recorded count short-circuits before `cr3` is dereferenced.
        assert_eq!(unsafe { gate.take_snapshot(0x1000, |_, _| false) }, Ok(3));
    }

    #[test]
    fn test_verify_snapshot_fails_closed_before_a_snapshot_is_taken() {
        let policy = PolicyBuilder::new().build();
        let gate = policy.gate();

        // Having nothing to compare against is not a pass: this is the routine that detects
        // page-table tampering after lock.
        assert_eq!(
            // SAFETY: the missing snapshot short-circuits before `cr3`/`scratch` are dereferenced.
            unsafe { gate.verify_snapshot(0x1000, |_, _| false, core::ptr::null_mut(), 0) },
            Err(PolicyError::InternalError)
        );
    }

    #[test]
    fn test_compare_snapshot() {
        let saved = [mem(0x1000, 0x1000, RESOURCE_ATTR_READ), mem(0x8000, 0x1000, RESOURCE_ATTR_WRITE)];

        assert_eq!(compare_snapshot(&saved, &saved), Ok(()));
        // A page table that grew or shrank since the snapshot.
        assert_eq!(compare_snapshot(&saved, &saved[..1]), Err(PolicyError::AccessDenied));
        // Same shape, but an attribute changed under us.
        let tampered = [saved[0], mem(0x8000, 0x1000, RESOURCE_ATTR_EXECUTE)];
        assert_eq!(compare_snapshot(&saved, &tampered), Err(PolicyError::AccessDenied));
    }

    #[test]
    fn test_fetch_n_update_policy_requires_a_snapshot() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        let gate = policy.gate();
        let mut dest = vec![0u8; 256];

        assert_eq!(
            // SAFETY: `dest` is writable for `dest.len()` bytes.
            unsafe { gate.fetch_n_update_policy(dest.as_mut_ptr(), dest.len()) },
            Err(PolicyError::InternalError)
        );
    }

    #[test]
    fn test_fetch_n_update_policy_rejects_a_small_destination() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        let mut gate = policy.gate();
        let mut snapshot = [mem(0x1000, 0x1000, RESOURCE_ATTR_READ)];
        gate.set_memory_policy_buffer(snapshot.as_mut_ptr(), snapshot.len());
        gate.record_snapshot(snapshot.len());

        let mut dest = vec![0u8; 8];
        assert_eq!(
            // SAFETY: `dest` is writable for `dest.len()` bytes.
            unsafe { gate.fetch_n_update_policy(dest.as_mut_ptr(), dest.len()) },
            Err(PolicyError::InternalError)
        );
    }

    #[test]
    fn test_fetch_n_update_policy_requires_a_memory_policy_root() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![])).build();
        let mut gate = policy.gate();
        let mut snapshot: [MemDescriptorV1_0; 0] = [];
        gate.set_memory_policy_buffer(snapshot.as_mut_ptr(), 0);
        gate.record_snapshot(0);

        let mut dest = vec![0u8; 256];
        assert_eq!(
            // SAFETY: `dest` is writable for `dest.len()` bytes.
            unsafe { gate.fetch_n_update_policy(dest.as_mut_ptr(), dest.len()) },
            Err(PolicyError::PolicyRootNotFound)
        );
    }

    #[test]
    fn test_gate_rejects_a_policy_that_declares_no_size() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).declared_size(0).build();
        assert_eq!(policy.try_gate().err(), Some(PolicyError::MalformedPolicy));
    }

    #[test]
    fn test_gate_rejects_a_policy_larger_than_its_buffer() {
        // A blob that claims more bytes than it was handed would make `fetch_n_update_policy`
        // copy adjacent MMRAM out to the non-MM caller.
        let policy =
            PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).declared_size(u32::MAX).build();
        assert_eq!(policy.try_gate().err(), Some(PolicyError::MalformedPolicy));
    }

    #[test]
    fn test_gate_rejects_a_root_array_past_the_end_of_the_policy() {
        let policy =
            PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).declared_root_count(4096).build();
        assert_eq!(policy.try_gate().err(), Some(PolicyError::MalformedPolicy));
    }

    #[test]
    fn test_gate_rejects_descriptors_past_the_end_of_the_policy() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![mem(0x1000, 0x1000, RESOURCE_ATTR_READ)]))
            .declared_descriptor_count(4096)
            .build();
        assert_eq!(policy.try_gate().err(), Some(PolicyError::MalformedPolicy));
    }

    #[test]
    fn test_gate_rejects_a_descriptor_count_that_overflows_a_byte_count() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![mem(0x1000, 0x1000, RESOURCE_ATTR_READ)]))
            .declared_descriptor_count(u32::MAX)
            .build();
        assert_eq!(policy.try_gate().err(), Some(PolicyError::MalformedPolicy));
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
    fn test_gate_accepts_a_root_of_an_unrecognized_type() {
        // Types the supervisor does not look up are never dereferenced, so their descriptor
        // bounds are not the gate's problem.
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Unknown(0x4242)).build();
        assert!(policy.try_gate().is_ok());
    }

    #[test]
    fn test_fetch_n_update_policy_rejects_an_overflowing_descriptor_count() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        let gate = policy.gate();
        // A descriptor count this large cannot be converted to a byte count.
        gate.record_snapshot(usize::MAX);

        let mut dest = vec![0u8; 256];
        assert_eq!(
            // SAFETY: `dest` is writable for `dest.len()` bytes.
            unsafe { gate.fetch_n_update_policy(dest.as_mut_ptr(), dest.len()) },
            Err(PolicyError::InternalError)
        );
    }

    #[test]
    fn test_fetch_n_update_policy_rejects_an_overflowing_total_size() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Mem(vec![])).build();
        let gate = policy.gate();
        // The descriptor bytes alone fit, but appending them to the firmware policy does not.
        gate.record_snapshot(usize::MAX / size_of::<MemDescriptorV1_0>());

        let mut dest = vec![0u8; 256];
        assert_eq!(
            // SAFETY: `dest` is writable for `dest.len()` bytes.
            unsafe { gate.fetch_n_update_policy(dest.as_mut_ptr(), dest.len()) },
            Err(PolicyError::InternalError)
        );
    }

    #[test]
    fn test_fetch_n_update_policy_appends_the_snapshot_and_patches_the_header() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_DENY, Descriptors::Mem(vec![]))
            .root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0x60, 1, READ)]))
            .build();
        let firmware_size = policy.header().size as usize;

        let mut gate = policy.gate();
        let mut snapshot =
            [mem(0x1000, 0x1000, RESOURCE_ATTR_READ), mem(0x8000, 0x2000, RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE)];
        gate.set_memory_policy_buffer(snapshot.as_mut_ptr(), snapshot.len());
        gate.record_snapshot(snapshot.len());

        let descriptor_bytes = snapshot.len() * size_of::<MemDescriptorV1_0>();
        let mut dest = vec![0u64; (firmware_size + descriptor_bytes).div_ceil(8)];
        let dest_bytes = dest.as_mut_slice().as_mut_bytes();

        // SAFETY: `dest_bytes` is writable for its whole length and is 8-byte aligned.
        let written = unsafe { gate.fetch_n_update_policy(dest_bytes.as_mut_ptr(), dest_bytes.len()) }
            .expect("policy fits in the destination");
        assert_eq!(written, firmware_size + descriptor_bytes);

        // SAFETY: `fetch_n_update_policy` wrote a complete, aligned policy blob into `dest_bytes`.
        let (header, roots) = unsafe {
            let header = &*(dest_bytes.as_ptr() as *const SecurePolicyDataV1_0);
            (header, header.get_policy_roots())
        };
        assert_eq!(header.size as usize, written);
        assert_eq!(header.memory_policy_count, 0);

        let mem_root = roots.iter().find(|r| r.policy_type == TYPE_MEM).expect("memory policy root");
        assert_eq!(mem_root.access_attr, ACCESS_ATTR_ALLOW);
        assert_eq!(mem_root.offset as usize, firmware_size);
        assert_eq!(mem_root.count as usize, snapshot.len());

        // The unrelated roots are copied through untouched.
        assert!(!roots.iter().any(|r| r.policy_type == TYPE_MSR));
        // SAFETY: the patched memory root describes the descriptors appended above.
        let appended = unsafe { mem_root.get_mem_descriptors(dest_bytes.as_ptr()) };
        assert_eq!(appended, snapshot.as_slice());
    }
}
