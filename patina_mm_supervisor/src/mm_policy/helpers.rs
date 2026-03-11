//! Policy Helper Functions
//!
//! This module provides utility functions for policy manipulation:
//! - Dump/print policy for debugging
//! - Compare two policies (order-independent)
//! - Page table walking to generate memory policy
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use super::{
    ACCESS_ATTR_ALLOW, InstructionDescriptorV1_0, IoDescriptorV1_0, MemDescriptorV1_0, MsrDescriptorV1_0, PolicyRootV1,
    RESOURCE_ATTR_COND_READ, RESOURCE_ATTR_COND_WRITE, RESOURCE_ATTR_EXECUTE, RESOURCE_ATTR_READ, RESOURCE_ATTR_WRITE,
    SaveStateCondition, SaveStateDescriptorV1_0, SecurePolicyDataV1_0, TYPE_INSTRUCTION, TYPE_IO, TYPE_MEM, TYPE_MSR,
    TYPE_SAVE_STATE,
};
use core::mem::size_of;

use patina_paging::{MemoryAttributes, PagingType, x64::X64PageTable};

use crate::{SharedPagingAllocator, state::security_state};

/// Dumps a single memory policy entry for debugging.
pub fn dump_mem_policy_entry(desc: &MemDescriptorV1_0) {
    let r = if (desc.mem_attributes & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
    let w = if (desc.mem_attributes & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
    let x = if (desc.mem_attributes & RESOURCE_ATTR_EXECUTE) != 0 { "X" } else { "." };

    log::info!(
        "  MEM: [0x{:016x}-0x{:016x}] {}{}{}",
        desc.base_address,
        desc.base_address.saturating_add(desc.size).saturating_sub(1),
        r,
        w,
        x
    );
}

/// Dumps policy data for debugging (like `DumpSmmPolicyData`).
///
/// ## Safety
///
/// The caller must ensure that `policy_ptr` points to a valid policy buffer.
pub unsafe fn dump_policy(policy_ptr: *const u8) {
    if policy_ptr.is_null() {
        log::error!("dump_policy: null pointer");
        return;
    }

    // SAFETY: `policy_ptr` is non-null (checked above) and, per this function's contract, points
    // to a valid policy buffer, so the header can be reborrowed for reading.
    let policy = unsafe { &*(policy_ptr as *const SecurePolicyDataV1_0) };

    let len = policy.size as usize;
    if len < size_of::<SecurePolicyDataV1_0>() {
        log::error!("dump_policy: invalid policy size: 0x{:x}", len);
        return;
    }

    log::info!("SMM_SUPV_SECURE_POLICY_DATA_V1_0:");
    log::info!("  Version: {}.{}", policy.version_major, policy.version_minor);
    log::info!("  Size: 0x{:x}", policy.size);
    log::info!("  MemoryPolicyOffset: 0x{:x}", policy.memory_policy_offset);
    log::info!("  MemoryPolicyCount: 0x{:x}", policy.memory_policy_count);
    log::info!("  Flags: 0x{:x}", policy.flags);
    log::info!("  Capabilities: 0x{:x}", policy.capabilities);
    log::info!("  PolicyRootOffset: 0x{:x}", policy.policy_root_offset);
    log::info!("  PolicyRootCount: 0x{:x}", policy.policy_root_count);

    // SAFETY: `policy` is the validated header of a valid policy buffer, so its policy-root array
    // (root pointer + count) is in-bounds.
    let policy_roots = unsafe { policy.get_policy_roots() };

    for (i, root) in policy_roots.iter().enumerate() {
        log::info!("Policy Root {}:", i);
        log::info!("  Version: {}", root.version);
        log::info!("  PolicyRootSize: {}", root.policy_root_size);
        log::info!("  Type: {}", root.policy_type);
        log::info!("  Offset: 0x{:x}", root.offset);
        log::info!("  Count: {}", root.count);
        log::info!("  AccessAttr: {}", if root.access_attr == ACCESS_ATTR_ALLOW { "ALLOW" } else { "DENY" });

        match root.policy_type {
            TYPE_MEM => {
                if root.offset as usize + root.count as usize * size_of::<MemDescriptorV1_0>() > len {
                    panic!(
                        "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                        i, root.offset, root.count, len
                    );
                }
                // SAFETY: `root` came from the validated policy buffer and `policy_ptr` points to
                // that same buffer, so the descriptor array is in-bounds.
                let descriptors = unsafe { root.get_mem_descriptors(policy_ptr) };
                for desc in descriptors {
                    dump_mem_policy_entry(desc);
                }
            }
            TYPE_IO => {
                if root.offset as usize + root.count as usize * size_of::<IoDescriptorV1_0>() > len {
                    panic!(
                        "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                        i, root.offset, root.count, len
                    );
                }
                // SAFETY: `root` came from the validated policy buffer and `policy_ptr` points to
                // that same buffer, so the descriptor array is in-bounds.
                let descriptors = unsafe { root.get_io_descriptors(policy_ptr) };
                for desc in descriptors {
                    let r = if (desc.attributes as u32 & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
                    let w = if (desc.attributes as u32 & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
                    log::info!(
                        "  IO: [0x{:04x}-0x{:04x}] {}{}",
                        desc.io_address,
                        (desc.io_address as u32).saturating_add(desc.length_or_width as u32).saturating_sub(1),
                        r,
                        w
                    );
                }
            }
            TYPE_MSR => {
                if root.offset as usize + root.count as usize * size_of::<MsrDescriptorV1_0>() > len {
                    panic!(
                        "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                        i, root.offset, root.count, len
                    );
                }
                // SAFETY: `root` came from the validated policy buffer and `policy_ptr` points to
                // that same buffer, so the descriptor array is in-bounds.
                let descriptors = unsafe { root.get_msr_descriptors(policy_ptr) };
                for desc in descriptors {
                    let r = if (desc.attributes as u32 & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
                    let w = if (desc.attributes as u32 & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
                    log::info!(
                        "  MSR: [0x{:08x}-0x{:08x}] {}{}",
                        desc.msr_address,
                        desc.msr_address.saturating_add(desc.length as u32).saturating_sub(1),
                        r,
                        w
                    );
                }
            }
            TYPE_INSTRUCTION => {
                if root.offset as usize + root.count as usize * size_of::<InstructionDescriptorV1_0>() > len {
                    panic!(
                        "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                        i, root.offset, root.count, len
                    );
                }
                // SAFETY: `root` came from the validated policy buffer and `policy_ptr` points to
                // that same buffer, so the descriptor array is in-bounds.
                let descriptors = unsafe { root.get_instruction_descriptors(policy_ptr) };
                for desc in descriptors {
                    let name = match desc.instruction_index {
                        0 => "CLI",
                        1 => "WBINVD",
                        2 => "HLT",
                        _ => "UNKNOWN",
                    };
                    let x = if (desc.attributes as u32 & RESOURCE_ATTR_EXECUTE) != 0 { "X" } else { "." };
                    log::info!("  INSTRUCTION: {} {}", name, x);
                }
            }
            TYPE_SAVE_STATE => {
                if root.offset as usize + root.count as usize * size_of::<SaveStateDescriptorV1_0>() > len {
                    panic!(
                        "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                        i, root.offset, root.count, len
                    );
                }
                // SAFETY: `root` came from the validated policy buffer and `policy_ptr` points to
                // that same buffer, so the descriptor array is in-bounds.
                let descriptors = unsafe { root.get_save_state_descriptors(policy_ptr) };
                for desc in descriptors {
                    let field = match desc.map_field {
                        0 => "RAX",
                        1 => "IO_TRAP",
                        _ => "UNKNOWN",
                    };
                    let condition = match desc.access_condition {
                        0 => "Unconditional",
                        1 => "IoRead",
                        2 => "IoWrite",
                        _ => "Unknown",
                    };
                    log::info!("  SAVESTATE: {} attr=0x{:x} cond={}", field, desc.attributes, condition);
                }
            }
            _ => {
                log::error!("  Unknown policy type: {}", root.policy_type);
            }
        }
    }
}

/// Errors that can occur during policy validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyCheckError {
    /// The policy pointer is null.
    NullPointer,
    /// Invalid policy version.
    InvalidVersion { major: u16, minor: u16 },
    /// A reserved field contains non-zero data.
    InvalidReservedField { policy_type: u32, entry_index: usize },
    /// The same policy type appears multiple times.
    DuplicatePolicyType { policy_type: u32 },
    /// Size mismatch.
    SizeMismatch { expected: usize, declared: usize },
    /// Unrecognized policy type.
    UnrecognizedPolicyType { policy_type: u32 },
    /// Unrecognized header bits.
    UnrecognizedHeaderBits,
    /// Unsupported attribute.
    UnsupportedAttribute { policy_type: u32, entry_index: usize, attributes: u32 },
    /// Conflicting condition.
    ConflictingCondition { entry_index: usize },
    /// Legacy memory policy detected.
    LegacyMemoryPolicyDetected,
}

/// Performs comprehensive security policy validation.
///
/// ## Safety
///
/// The caller must ensure that `policy_ptr` points to a valid policy buffer.
pub unsafe fn security_policy_check(policy_ptr: *const u8) -> Result<(), PolicyCheckError> {
    if policy_ptr.is_null() {
        return Err(PolicyCheckError::NullPointer);
    }

    // SAFETY: `policy_ptr` is non-null (checked above) and, per this function's contract, points
    // to a valid policy buffer, so the header can be reborrowed for reading.
    let policy = unsafe { &*(policy_ptr as *const SecurePolicyDataV1_0) };

    let len = policy.size as usize;
    if len < size_of::<SecurePolicyDataV1_0>() {
        log::error!("security_policy_check: invalid policy size: 0x{:x}", len);
        return Err(PolicyCheckError::SizeMismatch { expected: size_of::<SecurePolicyDataV1_0>(), declared: len });
    }

    log::info!("Security policy check entry...");

    // Version check
    if !policy.is_valid_version() {
        return Err(PolicyCheckError::InvalidVersion { major: policy.version_major, minor: policy.version_minor });
    }

    // Check for unrecognized header bits
    if policy.reserved != 0 || policy.flags != 0 || policy.capabilities != 0 {
        return Err(PolicyCheckError::UnrecognizedHeaderBits);
    }

    let mut total_scanned_size = size_of::<SecurePolicyDataV1_0>();
    let mut type_flags: u64 = 0;

    // SAFETY: `policy` is the validated header of a valid policy buffer, so its policy-root array
    // (root pointer + count) is in-bounds.
    let policy_roots = unsafe { policy.get_policy_roots() };

    for root in policy_roots.iter() {
        let type_bit = 1u64 << root.policy_type;

        if (type_flags & type_bit) != 0 {
            return Err(PolicyCheckError::DuplicatePolicyType { policy_type: root.policy_type });
        }
        type_flags |= type_bit;

        if !root.has_valid_reserved() {
            return Err(PolicyCheckError::InvalidReservedField { policy_type: root.policy_type, entry_index: 0 });
        }

        match root.policy_type {
            TYPE_IO => {
                if root.offset as usize + root.count as usize * size_of::<IoDescriptorV1_0>() > len {
                    panic!(
                        "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                        root.policy_type, root.offset, root.count, len
                    );
                }
                // SAFETY: `policy_ptr` is a valid policy buffer (contract) and `root` was read from
                // it, so the descriptors it references are in-bounds.
                unsafe { validate_io_policy(policy_ptr, root)? };
                total_scanned_size += (root.count as usize) * size_of::<IoDescriptorV1_0>();
            }
            TYPE_MEM => {
                if root.offset as usize + root.count as usize * size_of::<MemDescriptorV1_0>() > len {
                    panic!(
                        "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                        root.policy_type, root.offset, root.count, len
                    );
                }
                // SAFETY: as above; `policy_ptr`/`root` describe an in-bounds descriptor array.
                unsafe { validate_mem_policy(policy_ptr, root)? };
                total_scanned_size += (root.count as usize) * size_of::<MemDescriptorV1_0>();
            }
            TYPE_MSR => {
                if root.offset as usize + root.count as usize * size_of::<MsrDescriptorV1_0>() > len {
                    panic!(
                        "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                        root.policy_type, root.offset, root.count, len
                    );
                }
                // SAFETY: as above; `policy_ptr`/`root` describe an in-bounds descriptor array.
                unsafe { validate_msr_policy(policy_ptr, root)? };
                total_scanned_size += (root.count as usize) * size_of::<MsrDescriptorV1_0>();
            }
            TYPE_INSTRUCTION => {
                if root.offset as usize + root.count as usize * size_of::<InstructionDescriptorV1_0>() > len {
                    panic!(
                        "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                        root.policy_type, root.offset, root.count, len
                    );
                }
                // SAFETY: as above; `policy_ptr`/`root` describe an in-bounds descriptor array.
                unsafe { validate_instruction_policy(policy_ptr, root)? };
                total_scanned_size += (root.count as usize) * size_of::<InstructionDescriptorV1_0>();
            }
            TYPE_SAVE_STATE => {
                if root.offset as usize + root.count as usize * size_of::<SaveStateDescriptorV1_0>() > len {
                    panic!(
                        "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                        root.policy_type, root.offset, root.count, len
                    );
                }
                // SAFETY: as above; `policy_ptr`/`root` describe an in-bounds descriptor array.
                unsafe { validate_save_state_policy(policy_ptr, root)? };
                total_scanned_size += (root.count as usize) * size_of::<SaveStateDescriptorV1_0>();
            }
            _ => {
                return Err(PolicyCheckError::UnrecognizedPolicyType { policy_type: root.policy_type });
            }
        }

        total_scanned_size += size_of::<PolicyRootV1>();
    }

    if policy.memory_policy_count != 0 {
        return Err(PolicyCheckError::LegacyMemoryPolicyDetected);
    }

    if total_scanned_size != policy.size as usize {
        return Err(PolicyCheckError::SizeMismatch { expected: total_scanned_size, declared: policy.size as usize });
    }

    log::info!("Security policy check passed.");
    Ok(())
}

// Validation helper functions

/// Validates the I/O policy descriptors referenced by `root`.
///
/// ## Safety
///
/// The caller must ensure that `policy_base` points to a valid policy buffer
/// and that `root` is a policy root from that same buffer, so its descriptor
/// offset and count describe an in-bounds, properly aligned descriptor array.
unsafe fn validate_io_policy(policy_base: *const u8, root: &PolicyRootV1) -> Result<(), PolicyCheckError> {
    // SAFETY: The caller guarantees `policy_base` is a valid policy buffer and
    // that `root` belongs to it, so the descriptor slice is in bounds.
    let descriptors = unsafe { root.get_io_descriptors(policy_base) };

    for (i, desc) in descriptors.iter().enumerate() {
        if desc.reserved != 0 {
            return Err(PolicyCheckError::InvalidReservedField { policy_type: TYPE_IO, entry_index: i });
        }
    }
    Ok(())
}

/// Validates the memory policy descriptors referenced by `root`.
///
/// ## Safety
///
/// The caller must ensure that `policy_base` points to a valid policy buffer
/// and that `root` is a policy root from that same buffer, so its descriptor
/// offset and count describe an in-bounds, properly aligned descriptor array.
unsafe fn validate_mem_policy(policy_base: *const u8, root: &PolicyRootV1) -> Result<(), PolicyCheckError> {
    // SAFETY: The caller guarantees `policy_base` is a valid policy buffer and
    // that `root` belongs to it, so the descriptor slice is in bounds.
    let descriptors = unsafe { root.get_mem_descriptors(policy_base) };

    for (i, desc) in descriptors.iter().enumerate() {
        if desc.reserved != 0 {
            return Err(PolicyCheckError::InvalidReservedField { policy_type: TYPE_MEM, entry_index: i });
        }
    }
    Ok(())
}

/// Validates the MSR policy descriptors referenced by `root`.
///
/// ## Safety
///
/// The caller must ensure that `policy_base` points to a valid policy buffer.
/// But MSR descriptors don't have reserved fields, so we don't need to validate
/// any offsets/counts here.
unsafe fn validate_msr_policy(_policy_base: *const u8, _root: &PolicyRootV1) -> Result<(), PolicyCheckError> {
    // MSR descriptors don't have reserved fields
    Ok(())
}

/// Validates the instruction policy descriptors referenced by `root`.
///
/// ## Safety
///
/// The caller must ensure that `policy_base` points to a valid policy buffer
/// and that `root` is a policy root from that same buffer, so its descriptor
/// offset and count describe an in-bounds, properly aligned descriptor array.
unsafe fn validate_instruction_policy(policy_base: *const u8, root: &PolicyRootV1) -> Result<(), PolicyCheckError> {
    // SAFETY: The caller guarantees `policy_base` is a valid policy buffer and
    // that `root` belongs to it, so the descriptor slice is in bounds.
    let descriptors = unsafe { root.get_instruction_descriptors(policy_base) };

    for (i, desc) in descriptors.iter().enumerate() {
        if desc.reserved != 0 {
            return Err(PolicyCheckError::InvalidReservedField { policy_type: TYPE_INSTRUCTION, entry_index: i });
        }
    }
    Ok(())
}

/// Validates the save state policy descriptors referenced by `root`.
///
/// ## Safety
///
/// The caller must ensure that `policy_base` points to a valid policy buffer
/// and that `root` is a policy root from that same buffer, so its descriptor
/// offset and count describe an in-bounds, properly aligned descriptor array.
unsafe fn validate_save_state_policy(policy_base: *const u8, root: &PolicyRootV1) -> Result<(), PolicyCheckError> {
    // SAFETY: The caller guarantees `policy_base` is a valid policy buffer and
    // that `root` belongs to it, so the descriptor slice is in bounds.
    let descriptors = unsafe { root.get_save_state_descriptors(policy_base) };

    for (i, desc) in descriptors.iter().enumerate() {
        // Check for unsupported write attributes
        if (desc.attributes & (RESOURCE_ATTR_WRITE | RESOURCE_ATTR_COND_WRITE)) != 0 {
            return Err(PolicyCheckError::UnsupportedAttribute {
                policy_type: TYPE_SAVE_STATE,
                entry_index: i,
                attributes: desc.attributes,
            });
        }

        // Check for conflicting conditions
        if (desc.attributes & RESOURCE_ATTR_COND_READ) == 0
            && desc.access_condition != SaveStateCondition::Unconditional as u32
        {
            return Err(PolicyCheckError::ConflictingCondition { entry_index: i });
        }

        if desc.reserved != 0 {
            return Err(PolicyCheckError::InvalidReservedField { policy_type: TYPE_SAVE_STATE, entry_index: i });
        }
    }
    Ok(())
}

/// Memory policy builder for collecting memory descriptors from page table walking.
///
/// This is used to generate memory policy from page table entries.
pub struct MemoryPolicyBuilder {
    /// Current descriptor being built
    current: Option<MemDescriptorV1_0>,
    /// Maximum number of descriptors we can store
    max_count: usize,
    /// Buffer for descriptors
    buffer_ptr: *mut MemDescriptorV1_0,
    /// Current count of descriptors
    count: usize,
}

impl MemoryPolicyBuilder {
    /// Creates a new memory policy builder.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `buffer_ptr` points to a valid buffer
    /// with space for at least `max_count` descriptors.
    pub unsafe fn new(buffer_ptr: *mut MemDescriptorV1_0, max_count: usize) -> Self {
        Self { current: None, max_count, buffer_ptr, count: 0 }
    }

    /// Adds a memory region to the policy.
    ///
    /// Adjacent regions with the same attributes will be coalesced. Returns
    /// `Err(())` if the descriptor buffer is full.
    pub fn add_region(&mut self, base: u64, size: u64, attributes: u32) -> Result<(), ()> {
        let new_desc = MemDescriptorV1_0 { base_address: base, size, mem_attributes: attributes, reserved: 0 };

        if let Some(ref mut current) = self.current {
            // Check if we can coalesce with current
            let current_end = current.base_address.saturating_add(current.size);
            if base == current_end && attributes == current.mem_attributes {
                // Coalesce
                current.size = current.size.saturating_add(size);
                return Ok(());
            } else {
                // Flush current and start new
                self.flush_current()?;
            }
        }

        self.current = Some(new_desc);
        Ok(())
    }

    /// Flushes the current descriptor to the buffer.
    fn flush_current(&mut self) -> Result<(), ()> {
        if let Some(desc) = self.current.take() {
            if self.count >= self.max_count {
                return Err(());
            }

            // SAFETY: We checked bounds
            unsafe {
                *self.buffer_ptr.add(self.count) = desc;
            }
            self.count += 1;
        }
        Ok(())
    }

    /// Finishes building and returns the count of descriptors.
    pub fn finish(mut self) -> Result<usize, ()> {
        self.flush_current()?;
        Ok(self.count)
    }
}

/// Converts effective page-table memory attributes to policy R/W/X attributes.
///
/// The iterator yields only present leaf mappings whose attributes already
/// fold in restrictions inherited from parent table entries, so this is a
/// direct translation: read is always granted, write unless read-only, and
/// execute unless execute-protected.
#[inline]
fn mem_attrs_to_policy_attrs(attributes: MemoryAttributes) -> u32 {
    let mut attrs = RESOURCE_ATTR_READ;

    if !attributes.contains(MemoryAttributes::ReadOnly) {
        attrs |= RESOURCE_ATTR_WRITE;
    }

    if !attributes.contains(MemoryAttributes::ExecuteProtect) {
        attrs |= RESOURCE_ATTR_EXECUTE;
    }

    attrs
}

/// Errors that can occur during page table walking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTableWalkError {
    /// Buffer is full, cannot add more descriptors.
    BufferFull,
    /// The CR3 value is invalid (null).
    InvalidCr3,
}

/// Callback type for checking if a buffer is inside MMRAM.
///
/// Returns `true` if the buffer `[base, base + size)` is fully inside MMRAM.
pub type IsInsideMmramFn = fn(base: u64, size: u64) -> bool;

/// Walks x86_64 4-level page tables and generates memory policy descriptors.
///
/// This function traverses the page table hierarchy starting from the PML4
/// table (pointed to by CR3), and for each mapped page, generates a memory
/// policy descriptor with the effective R/W/X attributes.
///
/// Adjacent pages with the same attributes are coalesced into single descriptors.
/// Regions that lie fully inside MMRAM are skipped. Returns the number of memory
/// policy descriptors generated.
///
/// ## Safety
///
/// The caller must ensure that:
/// - `cr3` points to a valid PML4 table
/// - `buffer` has space for at least `max_count` descriptors
/// - The page table memory is accessible and won't change during the walk
pub unsafe fn walk_page_table(
    cr3: u64,
    buffer: *mut MemDescriptorV1_0,
    max_count: usize,
    is_inside_mmram: IsInsideMmramFn,
) -> Result<usize, PageTableWalkError> {
    if cr3 == 0 || buffer.is_null() {
        return Err(PageTableWalkError::InvalidCr3);
    }

    // Construct a read-only view of the active page table rooted at CR3. Clear
    // the low 12 flag bits to obtain the page-aligned PML4 base. The iterator
    // only reads entries, so the allocator is stored but never invoked.
    let base = cr3 & !0xFFF;
    let allocator = SharedPagingAllocator::new(security_state().paging_allocator());
    // SAFETY: The caller guarantees `cr3` points to a valid PML4 table that
    // remains stable for the duration of the walk.
    let page_table = unsafe { X64PageTable::from_existing(base, allocator, PagingType::Paging4Level) }
        .map_err(|_| PageTableWalkError::InvalidCr3)?;

    // SAFETY: per this function's contract `buffer` has space for `max_count` `MemDescriptorV1_0`
    // entries, satisfying `MemoryPolicyBuilder::new`'s requirement.
    let mut builder = unsafe { MemoryPolicyBuilder::new(buffer, max_count) };

    for region in page_table.iter_mapped_regions(None) {
        // Skip regions that lie fully inside MMRAM.
        if is_inside_mmram(region.pa, region.size) {
            continue;
        }

        let attrs = mem_attrs_to_policy_attrs(region.attributes);
        builder.add_region(region.pa, region.size, attrs).map_err(|()| PageTableWalkError::BufferFull)?;
    }

    builder.finish().map_err(|()| PageTableWalkError::BufferFull)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dump_mem_policy_entry() {
        // Just verify it doesn't panic
        let desc = MemDescriptorV1_0 {
            base_address: 0x1000,
            size: 0x1000,
            mem_attributes: RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE,
            reserved: 0,
        };
        dump_mem_policy_entry(&desc);
    }
}
