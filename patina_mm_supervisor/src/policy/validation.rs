//! Firmware Policy Validation
//!
//! Decides whether the firmware policy blob the MM IPL hands down is one the
//! supervisor will accept. The check runs once during initialization and
//! rejects a blob that repeats a policy type, leaves a reserved field or header
//! bit set, carries an attribute the supervisor does not implement, states a
//! save-state condition that contradicts itself, still uses the legacy memory
//! policy format, or whose declared size does not match the bytes actually
//! scanned.
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

use patina::MemoryRange;

use patina_paging::{MemoryAttributes, PagingType, x64::X64PageTable};

use crate::{error::MmSupervisorResult, memory::SharedPagingAllocator, state::security_state};

/// Errors that can occur during policy validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyValidationError {
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
            Self::LegacyMemoryPolicyDetected => {
                write!(f, "the policy uses the legacy memory policy format, which is not accepted")
            }
        }
    }
}

/// Checks that the policy a [`PolicyGate`] was built over is one the supervisor will accept.
///
/// Taking the gate rather than a raw pointer is what makes this safe and keeps it to one job.
/// [`PolicyGate::new`] has already rejected a null pointer, an unsupported version, and any
/// internal offset that falls outside the blob, so this reads the header and the roots through
/// safe accessors and checks only what the layout rules do not cover.
pub(crate) fn security_policy_check(gate: &PolicyGate) -> MmSupervisorResult<()> {
    let policy = gate.policy();
    let policy_ptr = gate.as_ptr();

    log::info!("Security policy check entry...");

    // Check for unrecognized header bits
    if policy.reserved != 0 || policy.flags != 0 || policy.capabilities != 0 {
        return Err(PolicyValidationError::UnrecognizedHeaderBits.into());
    }

    let mut total_scanned_size = size_of::<SecurePolicyDataV1_0>();
    let mut type_flags: u64 = 0;

    for root in gate.policy_roots() {
        let type_bit = 1u64 << root.policy_type;

        if (type_flags & type_bit) != 0 {
            return Err(PolicyValidationError::DuplicatePolicyType { policy_type: root.policy_type }.into());
        }
        type_flags |= type_bit;

        if !root.has_valid_reserved() {
            return Err(
                PolicyValidationError::InvalidReservedField { policy_type: root.policy_type, entry_index: 0 }.into()
            );
        }

        // Every arm below reads a descriptor array that `PolicyGate::new` already proved lies
        // wholly inside the blob, so the bounds are not re-checked here.
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
                // SAFETY: as above; `policy_ptr`/`root` describe an in-bounds descriptor array.
                unsafe { validate_msr_policy(policy_ptr, root)? };
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
unsafe fn validate_io_policy(policy_base: *const u8, root: &PolicyRootV1) -> MmSupervisorResult<()> {
    // SAFETY: The caller guarantees `policy_base` is a valid policy buffer and
    // that `root` belongs to it, so the descriptor slice is in bounds.
    let descriptors = unsafe { root.get_io_descriptors(policy_base) };

    for (index, desc) in descriptors.iter().enumerate() {
        if desc.reserved != 0 {
            return Err(PolicyValidationError::InvalidReservedField { policy_type: TYPE_IO, entry_index: index }.into());
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
unsafe fn validate_mem_policy(policy_base: *const u8, root: &PolicyRootV1) -> MmSupervisorResult<()> {
    // SAFETY: The caller guarantees `policy_base` is a valid policy buffer and
    // that `root` belongs to it, so the descriptor slice is in bounds.
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

/// Validates the MSR policy descriptors referenced by `root`.
///
/// ## Safety
///
/// The caller must ensure that `policy_base` points to a valid policy buffer.
/// But MSR descriptors don't have reserved fields, so we don't need to validate
/// any offsets/counts here.
unsafe fn validate_msr_policy(_policy_base: *const u8, _root: &PolicyRootV1) -> MmSupervisorResult<()> {
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
unsafe fn validate_instruction_policy(policy_base: *const u8, root: &PolicyRootV1) -> MmSupervisorResult<()> {
    // SAFETY: The caller guarantees `policy_base` is a valid policy buffer and
    // that `root` belongs to it, so the descriptor slice is in bounds.
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

/// Validates the save state policy descriptors referenced by `root`.
///
/// ## Safety
///
/// The caller must ensure that `policy_base` points to a valid policy buffer
/// and that `root` is a policy root from that same buffer, so its descriptor
/// offset and count describe an in-bounds, properly aligned descriptor array.
unsafe fn validate_save_state_policy(policy_base: *const u8, root: &PolicyRootV1) -> MmSupervisorResult<()> {
    // SAFETY: The caller guarantees `policy_base` is a valid policy buffer and
    // that `root` belongs to it, so the descriptor slice is in bounds.
    let descriptors = unsafe { root.get_save_state_descriptors(policy_base) };

    for (index, desc) in descriptors.iter().enumerate() {
        // Check for unsupported write attributes
        if (desc.attributes & (RESOURCE_ATTR_WRITE | RESOURCE_ATTR_COND_WRITE)) != 0 {
            return Err(PolicyValidationError::UnsupportedAttribute {
                policy_type: TYPE_SAVE_STATE,
                entry_index: index,
                attributes: desc.attributes,
            }
            .into());
        }

        // Check for conflicting conditions
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

/// Memory policy builder for collecting memory descriptors from page table walking.
///
/// This is used to generate memory policy from page table entries.
pub struct MemoryPolicyBuilder {
    /// Region of the descriptor being built, and its attributes
    current: Option<(MemoryRange, u32)>,
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

    /// Flushes the current descriptor to the buffer.
    fn flush_current(&mut self) -> Result<(), ()> {
        if let Some((range, mem_attributes)) = self.current.take() {
            if self.count >= self.max_count {
                log::error!("Memory policy buffer is full at {} descriptor(s); cannot record {range}", self.max_count);
                return Err(());
            }

            let desc =
                MemDescriptorV1_0 { base_address: range.base(), size: range.size(), mem_attributes, reserved: 0 };

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
            Self::InvalidCr3 => write!(f, "the CR3 value is null, so no page table can be walked"),
            Self::InvalidRegion { base, size } => {
                write!(f, "the page table maps an invalid region at 0x{base:x} (size 0x{size:x})")
            }
        }
    }
}

/// Callback type for checking if a range is inside MMRAM.
///
/// Returns `true` if the range is fully inside MMRAM.
pub type IsInsideMmramFn = fn(&MemoryRange) -> bool;

/// Walks `x86_64` 4-level page tables and generates memory policy descriptors.
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
) -> MmSupervisorResult<usize> {
    if cr3 == 0 || buffer.is_null() {
        return Err(PageTableWalkError::InvalidCr3.into());
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
        let range = MemoryRange::new(region.pa, region.size)
            .map_err(|_| PageTableWalkError::InvalidRegion { base: region.pa, size: region.size })?;

        // Skip regions that lie fully inside MMRAM.
        if is_inside_mmram(&range) {
            continue;
        }

        let attrs = mem_attrs_to_policy_attrs(region.attributes);
        builder.add_region(range, attrs).map_err(|()| PageTableWalkError::BufferFull)?;
    }

    builder.finish().map_err(|()| PageTableWalkError::BufferFull.into())
}
