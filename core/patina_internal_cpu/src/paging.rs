//! UEFI Paging Module
//!
//! This module provides implementation for handling paging.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::error::EfiError;
#[cfg(target_arch = "x86_64")]
use patina_mtrr::error::MtrrError;
use patina_paging::{MemoryAttributes, PtError};

cfg_if::cfg_if! {
    if #[cfg(all(target_arch = "x86_64"))] {
        mod x64;
        pub use x64::create_cpu_x64_paging as create_cpu_paging;
        pub use x64::open_active_cpu_x64_paging as open_active_cpu_paging;
    } else if #[cfg(all(target_arch = "aarch64"))] {
        mod aarch64;
        pub use aarch64::create_cpu_aarch64_paging as create_cpu_paging;
        pub use aarch64::open_active_cpu_aarch64_paging as open_active_cpu_paging;
    } else {
        mod null;
        pub use null::create_cpu_null_paging as create_cpu_paging;
        pub use null::open_active_cpu_null_paging as open_active_cpu_paging;
    }
}

/// Enum representing the cache attribute value of a memory region if it is not maintained
/// by the page table. On x64 platforms, this allows for unmapped pages to still reflect
/// the cache attributes as managed by MTRRs. On ARM64 this will always be `NotSupported` as
/// cache attributes are always managed by the page table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheAttributeValue {
    /// Valid cache attributes for the memory region
    Valid(MemoryAttributes),
    /// The memory region is unmapped
    Unmapped,
    /// Cache attributes are only supported via the page table for this architecture
    NotSupported(MemoryAttributes),
}

/// Errors returned by Patina paging operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagingError {
    /// An input parameter is invalid.
    InvalidParameter,
    /// The paging operation ran out of resources.
    OutOfResources,
    /// No mapping exists for the requested range.
    NoMapping,
    /// The requested operation is not supported.
    Unsupported,
    /// The requested memory attributes are incompatible.
    IncompatibleMemoryAttributes,
    /// The address is not aligned.
    UnalignedAddress,
    /// The memory range is not aligned.
    UnalignedMemoryRange,
    /// The memory range is invalid.
    InvalidMemoryRange,
    /// The range contains both mapped and unmapped pages.
    InconsistentMappingAcrossRange,
    /// An internal paging error occurred.
    InternalError,
    /// The range has non-uniform memory attributes.
    NonUniformMemoryAttributes,
    /// The operation has already started.
    AlreadyStarted,
    /// Cache attributes are managed only by the page table. This is not an error, but a condition
    /// to indicate that only the page table attributes should be respected.
    CacheAttributesOnlyInPageTable,
}

impl From<PtError> for PagingError {
    fn from(error: PtError) -> Self {
        match error {
            PtError::InvalidParameter => Self::InvalidParameter,
            PtError::OutOfResources | PtError::AllocationFailure => Self::OutOfResources,
            PtError::NoMapping => Self::NoMapping,
            PtError::IncompatibleMemoryAttributes => Self::IncompatibleMemoryAttributes,
            PtError::UnalignedPageBase | PtError::UnalignedAddress => Self::UnalignedAddress,
            PtError::UnalignedMemoryRange => Self::UnalignedMemoryRange,
            PtError::InvalidMemoryRange => Self::InvalidMemoryRange,
            PtError::InconsistentMappingAcrossRange => Self::InconsistentMappingAcrossRange,
            PtError::UnsupportedPagingType => Self::Unsupported,
            PtError::AdditionOverflow | PtError::SubtractionUnderflow | PtError::InternalError => Self::InternalError,
            PtError::NonUniformMemoryAttributes => Self::NonUniformMemoryAttributes,
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[cfg_attr(coverage, coverage(off))]
impl From<MtrrError> for PagingError {
    fn from(error: MtrrError) -> Self {
        match error {
            MtrrError::MtrrNotSupported => Self::CacheAttributesOnlyInPageTable,
            MtrrError::VariableRangeMtrrExhausted | MtrrError::OutOfResources => Self::OutOfResources,
            MtrrError::FixedRangeMtrrBaseAddressNotAligned
            | MtrrError::FixedRangeMtrrLengthNotAligned
            | MtrrError::InvalidParameter => Self::InvalidParameter,
            MtrrError::BufferTooSmall => Self::InternalError,
            MtrrError::AlreadyStarted => Self::AlreadyStarted,
        }
    }
}

#[cfg_attr(coverage, coverage(off))]
impl From<EfiError> for PagingError {
    fn from(error: EfiError) -> Self {
        match error {
            EfiError::OutOfResources => Self::OutOfResources,
            EfiError::NotFound | EfiError::NoMapping => Self::NoMapping,
            EfiError::Unsupported => Self::Unsupported,
            EfiError::BufferTooSmall => Self::InternalError,
            EfiError::AlreadyStarted => Self::AlreadyStarted,
            _ => Self::InvalidParameter,
        }
    }
}

#[cfg_attr(coverage, coverage(off))]
impl core::fmt::Display for PagingError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl core::error::Error for PagingError {}

/// The `PatinaPageTable` trait is Patina's abstraction layer over the `PageTable` trait
/// provided by patina-paging. This trait manages architectural abstractions over the page tables.
pub trait PatinaPageTable {
    /// Function to identity map the designated memory region with the provided
    /// attributes. The requested memory region will be mapped with the specified
    /// attributes, regardless of the current mapping state of the region.
    ///
    /// ## Arguments
    /// * `address` - The memory address to map. VA == PA
    /// * `size` - The memory size to map.
    /// * `attributes` - The memory attributes to map. The acceptable
    ///   input will be `ExecuteProtect`, `ReadOnly`, as well as `Uncached`,
    ///   `WriteCombining`, `WriteThrough`, `Writeback`, `UncachedExport`
    ///   Compatible attributes can be "`ORed`"
    ///
    /// ## Errors
    /// * Returns `Ok(())` if successful else `Err(PagingError)` if failed
    fn map_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PagingError>;

    /// Function to map the designated VA to the specified PA with the provided
    /// attributes. The requested memory region will be mapped with the specified
    /// attributes, regardless of the current mapping state of the region.
    ///
    /// ## Arguments
    /// * `va` - The virtual address to map.
    /// * `pa` - The physical address to map.
    /// * `size` - The memory size to map.
    /// * `attributes` - The memory attributes to map. The acceptable
    ///   input will be `ExecuteProtect`, `ReadOnly`, as well as `Uncached`,
    ///   `WriteCombining`, `WriteThrough`, `Writeback`, `UncachedExport`
    ///   Compatible attributes can be "`ORed`"
    ///
    /// ## Errors
    /// * Returns `Ok(())` if successful else `Err(PagingError)` if failed
    fn map_aliased_memory_region(
        &mut self,
        va: u64,
        pa: u64,
        size: u64,
        attributes: MemoryAttributes,
    ) -> Result<(), PagingError>;

    /// Function to unmap the memory region provided by the caller. The
    /// requested memory region must be fully mapped prior to this call. The
    /// entire region does not need to have the same mapping state in order
    /// to unmap it. This API works for either identity mapped or aliased mappings.
    ///
    /// ## Arguments
    /// * `address` - The memory address to unmap.
    /// * `size` - The memory size to map.
    ///
    /// ## Errors
    /// * Returns `Ok(())` if successful else `Err(PagingError)` if failed
    fn unmap_memory_region(&mut self, address: u64, size: u64) -> Result<(), PagingError>;

    /// Function to install the page table from this page table instance.
    ///
    /// ## Errors
    /// * Returns `Ok(())` if successful else `Err(PagingError)` if failed
    fn install_page_table(&mut self) -> Result<(), PagingError>;

    /// Function to query the mapping status and return attribute of supplied
    /// memory region if it is properly and consistently mapped.
    ///
    /// ## Arguments
    /// * `address` - The memory address to query.
    /// * `size` - The memory size to query.
    ///
    /// ## Returns
    /// Returns memory attributes
    ///
    ///   `Ok(MemoryAttributes)` if the page range is mapped else
    ///   `Err(PagingError, CacheAttributeValue::NotSupported)` if cache attributes are not available
    ///   `Err(PagingError, CacheAttributeValue)` if the page is unmapped but caching attributes are available
    ///   `Err(CacheAttributesOnlyInPageTable, CacheAttributeValue::NotSupported(MemoryAttributes))`
    ///    if the cache attributes are only available in the page table. `MemoryAttributes` are the page table attributes.
    fn query_memory_region(
        &self,
        address: u64,
        size: u64,
    ) -> Result<MemoryAttributes, (PagingError, CacheAttributeValue)>;

    /// Function to dump memory ranges with their attributes. It uses current
    /// cr3 as the base. This function can be used from
    /// `test_dump_page_tables()` test case
    ///
    /// ## Arguments
    /// * `address` - The memory address to map.
    /// * `size` - The memory size to map.
    fn dump_page_tables(&self, address: u64, size: u64) -> Result<(), PagingError>;

    /// Function to handle a change in the cacheability of a memory region.
    /// This function is called when the cacheability of a memory region changes.
    ///
    /// ## Arguments
    /// * `address` - The memory address of the region.
    /// * `size` - The size of the region.
    /// * `old_cache_attributes` - The old cache attributes for the region.
    /// * `new_cache_attributes` - The new cache attributes for the region.
    fn handle_cacheability_change(
        &self,
        address: u64,
        size: u64,
        old_cache_attributes: MemoryAttributes,
        new_cache_attributes: MemoryAttributes,
    );
}
