//! Firmware policy validation.
//!
//! Decides once, during initialization, whether the supervisor accepts the firmware policy blob
//! from the MM IPL. It rejects a blob that repeats a policy type, sets a reserved field or header
//! bit, uses an attribute the supervisor does not implement, has a contradictory save-state
//! condition, uses the legacy memory policy format, or declares a size that does not match the
//! bytes scanned.
//!
//! It also provides [`walk_page_table`], which builds the memory policy from the page table.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use super::{
    InstructionDescriptorV1_0, IoDescriptorV1_0, MemDescriptorV1_0, MsrDescriptorV1_0, PolicyRootV1,
    RESOURCE_ATTR_COND_READ, RESOURCE_ATTR_COND_WRITE, RESOURCE_ATTR_EXECUTE, RESOURCE_ATTR_READ, RESOURCE_ATTR_WRITE,
    SaveStateCondition, SaveStateDescriptorV1_0, SecurePolicyDataV1_0, TYPE_INSTRUCTION, TYPE_IO, TYPE_MEM, TYPE_MSR,
    TYPE_SAVE_STATE, gate::PolicyGate,
};
use core::mem::size_of;

use patina::{MemoryRange, UEFI_PAGE_MASK};

use patina_paging::{MemoryAttributes, PagingType, x64::X64PageTable};

use crate::{
    error::MmSupervisorResult,
    intrinsics::{CR4_LA57, read_cr4},
    memory::SharedPagingAllocator,
    state::security_state,
};

/// Errors that can occur during policy validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyValidationError {
    /// A reserved field contains non-zero data.
    InvalidReservedField {
        /// Policy type of the root or descriptor with the bad field.
        policy_type: u32,
        /// Index of the descriptor within its root, or of the root itself for a root field.
        entry_index: usize,
    },
    /// The same policy type appears in more than one root.
    DuplicatePolicyType {
        /// The repeated policy type.
        policy_type: u32,
    },
    /// The declared policy size does not match the size of the structures it contains.
    SizeMismatch {
        /// Size computed from the header, roots and descriptors.
        expected: usize,
        /// Size declared in the header.
        declared: usize,
    },
    /// A root has a policy type the supervisor does not recognize.
    UnrecognizedPolicyType {
        /// The unrecognized policy type.
        policy_type: u32,
    },
    /// The header sets flag or capability bits the supervisor does not recognize.
    UnrecognizedHeaderBits,
    /// A descriptor sets attribute bits its policy type does not support.
    UnsupportedAttribute {
        /// Policy type of the descriptor.
        policy_type: u32,
        /// Index of the descriptor within its root.
        entry_index: usize,
        /// The attribute bits the descriptor sets.
        attributes: u32,
    },
    /// A save state descriptor combines access conditions that cannot both hold.
    ConflictingCondition {
        /// Index of the descriptor within its root.
        entry_index: usize,
    },
    /// An I/O descriptor has a zero length or runs past the end of the 16-bit port space.
    InvalidIoRange {
        /// Index of the descriptor within its root.
        entry_index: usize,
    },
    /// The policy uses the legacy memory policy fields instead of a `TYPE_MEM` root.
    LegacyMemoryPolicyDetected,
}

impl core::error::Error for PolicyValidationError {}

impl core::fmt::Display for PolicyValidationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidReservedField { policy_type, entry_index } => {
                write!(f, "entry {entry_index} of policy type {policy_type} has a non-zero reserved field")
            }
            Self::DuplicatePolicyType { policy_type } => {
                write!(f, "policy type {policy_type} appears more than once")
            }
            Self::SizeMismatch { expected, declared } => {
                write!(f, "the policy declares a size of {declared} bytes, but {expected} bytes were scanned")
            }
            Self::UnrecognizedPolicyType { policy_type } => write!(f, "policy type {policy_type} is not recognized"),
            Self::UnrecognizedHeaderBits => write!(f, "the policy header sets bits the supervisor does not recognize"),
            Self::UnsupportedAttribute { policy_type, entry_index, attributes } => write!(
                f,
                "entry {entry_index} of policy type {policy_type} requests unsupported attributes 0x{attributes:x}"
            ),
            Self::ConflictingCondition { entry_index } => {
                write!(f, "save-state entry {entry_index} declares conflicting conditions")
            }
            Self::InvalidIoRange { entry_index } => {
                write!(f, "I/O entry {entry_index} has a zero length or runs past port 0xFFFF")
            }
            Self::LegacyMemoryPolicyDetected => {
                write!(f, "the policy uses the legacy memory policy format, which is not accepted")
            }
        }
    }
}

/// Checks that the supervisor accepts the policy a [`PolicyGate`] was built over.
///
/// Taking the gate instead of a raw pointer keeps this safe. [`PolicyGate::new`] already
/// rejected a null pointer, an unsupported version, and any internal offset outside the blob,
/// so this reads the header and roots through safe accessors and checks only what the layout
/// rules do not cover.
pub(crate) fn security_policy_check(gate: &PolicyGate) -> MmSupervisorResult<()> {
    let policy = gate.policy();
    let policy_ptr = gate.as_ptr();

    log::debug!("Security policy check entry...");

    if policy.reserved != 0 || policy.flags != 0 || policy.capabilities != 0 {
        return Err(PolicyValidationError::UnrecognizedHeaderBits.into());
    }

    let mut total_scanned_size = size_of::<SecurePolicyDataV1_0>();
    let mut type_flags: u64 = 0;

    for (root_index, root) in gate.policy_roots().iter().enumerate() {
        // A type too large for the bitmask cannot be a recognized type.
        let type_bit = 1u64
            .checked_shl(root.policy_type)
            .ok_or(PolicyValidationError::UnrecognizedPolicyType { policy_type: root.policy_type })?;

        if (type_flags & type_bit) != 0 {
            return Err(PolicyValidationError::DuplicatePolicyType { policy_type: root.policy_type }.into());
        }
        type_flags |= type_bit;

        if !root.has_valid_reserved() {
            return Err(PolicyValidationError::InvalidReservedField {
                policy_type: root.policy_type,
                entry_index: root_index,
            }
            .into());
        }

        // `PolicyGate::new` already checked that each descriptor array lies wholly inside the
        // blob, so the bounds are not re-checked here.
        match root.policy_type {
            TYPE_IO => {
                // SAFETY: `policy_ptr` is the gate's validated buffer and `root` came from it, so
                // the descriptor array it names is in bounds.
                unsafe { validate_io_policy(policy_ptr, root)? };
                total_scanned_size += (root.count as usize) * size_of::<IoDescriptorV1_0>();
            }
            TYPE_MEM => {
                // SAFETY: as above; `policy_ptr`/`root` describe an in-bounds descriptor array.
                unsafe { validate_mem_policy(policy_ptr, root)? };
                total_scanned_size += (root.count as usize) * size_of::<MemDescriptorV1_0>();
            }
            TYPE_MSR => {
                // MSR descriptors have no reserved fields, so there is nothing to validate.
                total_scanned_size += (root.count as usize) * size_of::<MsrDescriptorV1_0>();
            }
            TYPE_INSTRUCTION => {
                // SAFETY: as above; `policy_ptr`/`root` describe an in-bounds descriptor array.
                unsafe { validate_instruction_policy(policy_ptr, root)? };
                total_scanned_size += (root.count as usize) * size_of::<InstructionDescriptorV1_0>();
            }
            TYPE_SAVE_STATE => {
                // SAFETY: as above; `policy_ptr`/`root` describe an in-bounds descriptor array.
                unsafe { validate_save_state_policy(policy_ptr, root)? };
                total_scanned_size += (root.count as usize) * size_of::<SaveStateDescriptorV1_0>();
            }
            _ => {
                return Err(PolicyValidationError::UnrecognizedPolicyType { policy_type: root.policy_type }.into());
            }
        }

        total_scanned_size += size_of::<PolicyRootV1>();
    }

    if policy.memory_policy_count != 0 {
        return Err(PolicyValidationError::LegacyMemoryPolicyDetected.into());
    }

    if total_scanned_size != policy.size as usize {
        return Err(PolicyValidationError::SizeMismatch {
            expected: total_scanned_size,
            declared: policy.size as usize,
        }
        .into());
    }

    log::debug!("Security policy check passed.");
    Ok(())
}

/// Validates the I/O policy descriptors of `root`.
///
/// ## Safety
///
/// `policy_base` must point to a valid policy buffer and `root` must be a policy root from that
/// buffer, so its descriptor offset and count describe an in-bounds, aligned array.
unsafe fn validate_io_policy(policy_base: *const u8, root: &PolicyRootV1) -> MmSupervisorResult<()> {
    // SAFETY: the caller guarantees `root` belongs to the valid policy buffer at
    // `policy_base`, so the descriptor slice is in bounds.
    let descriptors = unsafe { root.get_io_descriptors(policy_base) };

    for (index, desc) in descriptors.iter().enumerate() {
        if desc.reserved != 0 {
            return Err(PolicyValidationError::InvalidReservedField { policy_type: TYPE_IO, entry_index: index }.into());
        }
        // The gate compares accesses against `[io_address, io_address + length_or_width)`, so an
        // empty range or one past the port space cannot describe real ports.
        let end = u32::from(desc.io_address) + u32::from(desc.length_or_width);
        if desc.length_or_width == 0 || end > u32::from(u16::MAX) + 1 {
            return Err(PolicyValidationError::InvalidIoRange { entry_index: index }.into());
        }
    }
    Ok(())
}

/// Validates the memory policy descriptors of `root`.
///
/// ## Safety
///
/// `policy_base` must point to a valid policy buffer and `root` must be a policy root from that
/// buffer, so its descriptor offset and count describe an in-bounds, aligned array.
unsafe fn validate_mem_policy(policy_base: *const u8, root: &PolicyRootV1) -> MmSupervisorResult<()> {
    // SAFETY: the caller guarantees `root` belongs to the valid policy buffer at
    // `policy_base`, so the descriptor slice is in bounds.
    let descriptors = unsafe { root.get_mem_descriptors(policy_base) };

    for (index, desc) in descriptors.iter().enumerate() {
        if desc.reserved != 0 {
            return Err(
                PolicyValidationError::InvalidReservedField { policy_type: TYPE_MEM, entry_index: index }.into()
            );
        }
    }
    Ok(())
}

/// Validates the instruction policy descriptors of `root`.
///
/// ## Safety
///
/// `policy_base` must point to a valid policy buffer and `root` must be a policy root from that
/// buffer, so its descriptor offset and count describe an in-bounds, aligned array.
unsafe fn validate_instruction_policy(policy_base: *const u8, root: &PolicyRootV1) -> MmSupervisorResult<()> {
    // SAFETY: the caller guarantees `root` belongs to the valid policy buffer at
    // `policy_base`, so the descriptor slice is in bounds.
    let descriptors = unsafe { root.get_instruction_descriptors(policy_base) };

    for (index, desc) in descriptors.iter().enumerate() {
        if desc.reserved != 0 {
            return Err(PolicyValidationError::InvalidReservedField {
                policy_type: TYPE_INSTRUCTION,
                entry_index: index,
            }
            .into());
        }
    }
    Ok(())
}

/// Validates the save state policy descriptors of `root`.
///
/// ## Safety
///
/// `policy_base` must point to a valid policy buffer and `root` must be a policy root from that
/// buffer, so its descriptor offset and count describe an in-bounds, aligned array.
unsafe fn validate_save_state_policy(policy_base: *const u8, root: &PolicyRootV1) -> MmSupervisorResult<()> {
    // SAFETY: the caller guarantees `root` belongs to the valid policy buffer at
    // `policy_base`, so the descriptor slice is in bounds.
    let descriptors = unsafe { root.get_save_state_descriptors(policy_base) };

    for (index, desc) in descriptors.iter().enumerate() {
        // Save state access is read-only.
        if (desc.attributes & (RESOURCE_ATTR_WRITE | RESOURCE_ATTR_COND_WRITE)) != 0 {
            return Err(PolicyValidationError::UnsupportedAttribute {
                policy_type: TYPE_SAVE_STATE,
                entry_index: index,
                attributes: desc.attributes,
            }
            .into());
        }

        // A condition requires the conditional-read attribute.
        if (desc.attributes & RESOURCE_ATTR_COND_READ) == 0
            && desc.access_condition != SaveStateCondition::Unconditional as u32
        {
            return Err(PolicyValidationError::ConflictingCondition { entry_index: index }.into());
        }

        if desc.reserved != 0 {
            return Err(PolicyValidationError::InvalidReservedField {
                policy_type: TYPE_SAVE_STATE,
                entry_index: index,
            }
            .into());
        }
    }
    Ok(())
}

/// Collects memory policy descriptors while walking the page table.
pub struct MemoryPolicyBuilder {
    /// Region of the descriptor being built, and its attributes.
    current: Option<(MemoryRange, u32)>,
    /// Maximum number of descriptors the buffer holds.
    max_count: usize,
    /// Descriptor buffer.
    buffer_ptr: *mut MemDescriptorV1_0,
    /// Number of descriptors written to the buffer.
    count: usize,
}

impl MemoryPolicyBuilder {
    /// Creates a memory policy builder that writes into `buffer_ptr`.
    ///
    /// ## Safety
    ///
    /// `buffer_ptr` must point to a writable buffer with space for at least `max_count`
    /// descriptors.
    pub unsafe fn new(buffer_ptr: *mut MemDescriptorV1_0, max_count: usize) -> Self {
        Self { current: None, max_count, buffer_ptr, count: 0 }
    }

    /// Adds a memory region to the policy.
    ///
    /// A region that starts where the current one ends, with the same attributes, is coalesced
    /// into it. Returns `Err(())` if the descriptor buffer is full.
    pub fn add_region(&mut self, range: MemoryRange, attributes: u32) -> Result<(), ()> {
        if let Some((current, current_attributes)) = &mut self.current
            && *current_attributes == attributes
            && current.end() == range.base()
            && let Ok(merged) = current.union(&range)
        {
            *current = merged;
            return Ok(());
        }

        self.flush_current()?;
        self.current = Some((range, attributes));
        Ok(())
    }

    /// Writes the pending descriptor to the buffer. Returns `Err(())` if the buffer is full.
    fn flush_current(&mut self) -> Result<(), ()> {
        if let Some((range, mem_attributes)) = self.current.take() {
            if self.count >= self.max_count {
                log::error!("Memory policy buffer is full at {} descriptor(s); cannot record {range}", self.max_count);
                return Err(());
            }

            let desc =
                MemDescriptorV1_0 { base_address: range.base(), size: range.size(), mem_attributes, reserved: 0 };

            // SAFETY: `count < max_count` (checked above), and `new` requires the buffer to hold
            // `max_count` descriptors.
            unsafe {
                *self.buffer_ptr.add(self.count) = desc;
            }
            self.count += 1;
        }
        Ok(())
    }

    /// Writes the pending descriptor and returns the descriptor count. Returns `Err(())` if the
    /// buffer is full.
    pub fn finish(mut self) -> Result<usize, ()> {
        self.flush_current()?;
        Ok(self.count)
    }
}

/// Converts effective page-table memory attributes to policy R/W/X attributes.
///
/// The iterator yields only present leaf mappings, with restrictions from parent entries
/// already folded in, so this is a direct translation: read is always granted, write unless
/// read-only, and execute unless execute-protected.
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
    /// The CR3 value or the descriptor buffer is null.
    InvalidCr3,
    /// The page table reported a mapped region that is empty or runs past the end of the address
    /// space.
    InvalidRegion {
        /// Base address of the region.
        base: u64,
        /// Size of the region in bytes.
        size: u64,
    },
}

impl core::error::Error for PageTableWalkError {}

impl core::fmt::Display for PageTableWalkError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BufferFull => write!(f, "the descriptor buffer is full, the walk cannot continue"),
            Self::InvalidCr3 => {
                write!(f, "the CR3 value or the descriptor buffer is null, so no page table can be walked")
            }
            Self::InvalidRegion { base, size } => {
                write!(f, "the page table maps an invalid region at 0x{base:x} (size 0x{size:x})")
            }
        }
    }
}

/// Callback that returns `true` if a range is fully inside MMRAM.
pub type IsInsideMmramFn = fn(&MemoryRange) -> bool;

/// Walks the `x86_64` page table at CR3, 4-level or 5-level as CR4.LA57 selects, and writes memory
/// policy descriptors with the effective R/W/X attributes of each mapped region.
///
/// Adjacent regions with the same attributes are merged into one descriptor, and regions fully
/// inside MMRAM are skipped. Returns the number of descriptors written.
///
/// ## Errors
///
/// Returns `InvalidCr3` for a null `cr3` or `buffer`, `InvalidRegion` for an invalid mapped
/// region, and `BufferFull` when more than `max_count` descriptors are needed.
///
/// ## Safety
///
/// The caller must ensure that:
/// - `cr3` points to a valid root page table for the active paging mode
/// - `buffer` has space for at least `max_count` descriptors
/// - The page table memory is accessible and does not change during the walk
pub unsafe fn walk_page_table(
    cr3: u64,
    buffer: *mut MemDescriptorV1_0,
    max_count: usize,
    is_inside_mmram: IsInsideMmramFn,
) -> MmSupervisorResult<usize> {
    if cr3 == 0 || buffer.is_null() {
        return Err(PageTableWalkError::InvalidCr3.into());
    }

    // Build a read-only view of the page table at CR3. Clearing the low 12 flag bits gives the
    // root table base. The iterator only reads entries, so the allocator is never called. The MMI
    // entry code sets CR4.LA57 when the producer built a 5-level table, so LA57 gives its depth.
    let base = cr3 & !(UEFI_PAGE_MASK as u64);
    let paging_type = if read_cr4() & CR4_LA57 != 0 { PagingType::Paging5Level } else { PagingType::Paging4Level };
    let allocator = SharedPagingAllocator::new(security_state().paging_allocator());
    // SAFETY: the caller guarantees `cr3` points to a valid root table for the active paging mode
    // that stays stable for the walk.
    let page_table = unsafe { X64PageTable::from_existing(base, allocator, paging_type) }
        .map_err(|_| PageTableWalkError::InvalidCr3)?;

    // SAFETY: the caller guarantees `buffer` has space for `max_count` descriptors, as
    // `MemoryPolicyBuilder::new` requires.
    let mut builder = unsafe { MemoryPolicyBuilder::new(buffer, max_count) };

    for region in page_table.iter_mapped_regions(None) {
        let range = MemoryRange::new(region.pa, region.size)
            .map_err(|_| PageTableWalkError::InvalidRegion { base: region.pa, size: region.size })?;

        if is_inside_mmram(&range) {
            continue;
        }

        let attrs = mem_attrs_to_policy_attrs(region.attributes);
        builder.add_region(range, attrs).map_err(|()| PageTableWalkError::BufferFull)?;
    }

    builder.finish().map_err(|()| PageTableWalkError::BufferFull.into())
}
