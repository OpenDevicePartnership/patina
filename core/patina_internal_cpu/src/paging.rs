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

use patina_paging::{MemoryAttributes, PtError};

cfg_if::cfg_if! {
    if #[cfg(all(target_arch = "x86_64"))] {
        mod x64;
        pub use x64::create_cpu_x64_paging as create_cpu_paging;
    } else if #[cfg(all(target_arch = "aarch64"))] {
        mod aarch64;
        pub use aarch64::create_cpu_aarch64_paging as create_cpu_paging;
    } else {
        mod null;
        pub use null::create_cpu_null_paging as create_cpu_paging;
    }
}

/// Enum representing the cache attribute value of a memory region if it is not maintained
/// by the page table. On x64 platforms, this allows for unmapped pages to still reflect
/// the cache attributes as managed by MTRRs. On ARM64 this will always be NotSupported as
/// cache attributes are always managed by the page table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheAttributeValue {
    /// Valid cache attributes for the memory region
    Valid(MemoryAttributes),
    /// The memory region is unmapped
    Unmapped,
    /// Cache attributes are only supported via the page table for this architecture
    NotSupported,
}

/// The PatinaPageTable trait is Patina's abstraction layer over the PageTable trait
/// provided by patina-paging. This trait manages architectural abstractions over the page tables.
pub trait PatinaPageTable {
    /// Function to map the designated memory region to with provided
    /// attributes. The requested memory region will be mapped with the specified
    /// attributes, regardless of the current mapping state of the region.
    ///
    /// ## Arguments
    /// * `address` - The memory address to map.
    /// * `size` - The memory size to map.
    /// * `attributes` - The memory attributes to map. The acceptable
    ///   input will be ExecuteProtect, ReadOnly, as well as Uncacheable,
    ///   WriteCombining, WriteThrough, Writeback, UncacheableExport
    ///   Compatible attributes can be "Ored"
    ///
    /// ## Errors
    /// * Returns `Ok(())` if successful else `Err(PtError)` if failed
    fn map_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PtError>;

    /// Function to unmap the memory region provided by the caller. The
    /// requested memory region must be fully mapped prior to this call. The
    /// entire region does not need to have the same mapping state in order
    /// to unmap it.
    ///
    /// ## Arguments
    /// * `address` - The memory address to map.
    /// * `size` - The memory size to map.
    ///
    /// ## Errors
    /// * Returns `Ok(())` if successful else `Err(PtError)` if failed
    fn unmap_memory_region(&mut self, address: u64, size: u64) -> Result<(), PtError>;

    /// Function to install the page table from this page table instance.
    ///
    /// ## Errors
    /// * Returns `Ok(())` if successful else `Err(PtError)` if failed
    fn install_page_table(&mut self) -> Result<(), PtError>;

    /// Function to query the mapping status and return attribute of supplied
    /// memory region if it is properly and consistently mapped. This function
    /// returns the caching attributes for platforms where caching attributes
    /// are managed outside of the page table (e.g., x86_64 MTRRs), even when
    /// the page range itself is unmapped.
    ///
    /// ## Arguments
    /// * `address` - The memory address to map.
    /// * `size` - The memory size to map.
    ///
    /// ## Returns
    ///   `Ok(MemoryAttributes)` if the page range is mapped else
    ///   `Err(PtError, None)` if the page is unmapped and the cache attributes are not available
    ///   `Err(PtError, Some(MemoryAttributes))` if the page is unmapped but caching attributes are available
    fn query_memory_region(&self, address: u64, size: u64) -> Result<MemoryAttributes, (PtError, CacheAttributeValue)>;

    /// Function to dump memory ranges with their attributes. It uses current
    /// cr3 as the base. This function can be used from
    /// `test_dump_page_tables()` test case
    ///
    /// ## Arguments
    /// * `address` - The memory address to map.
    /// * `size` - The memory size to map.
    fn dump_page_tables(&self, address: u64, size: u64) -> Result<(), PtError>;
}
