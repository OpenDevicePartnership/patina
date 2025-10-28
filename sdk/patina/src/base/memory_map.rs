//! UEFI Memory Map Utilities
//!
//! Utilities for working with UEFI memory maps.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use alloc::collections::BTreeMap;

use super::{SIZE_1MB, UEFI_PAGE_SIZE};
use r_efi::efi;

/// Prints detailed information about a UEFI memory map from a collection of memory descriptors.
///
/// This function logs detailed information about each memory descriptor in the provided
/// including type, base address, number of pages, size, and attributes.
///
/// It also provides a summary of memory by type.
///
/// # Arguments
///
/// * `descriptors` - A slice of UEFI memory descriptors to analyze and print
///
/// # Example
///
/// ```rust,no_run
/// use patina::base::memory_map;
/// use r_efi::efi;
///
/// let descriptors: Vec<efi::MemoryDescriptor> = vec![]; // Get from get_memory_map()
/// memory_map::print_details(&descriptors);
/// ```
#[coverage(off)]
pub fn print_details(descriptors: &[efi::MemoryDescriptor]) {
    log::info!(target: "memory_map_test", "\n");
    log::info!(target: "memory_map_test", "UEFI Memory Map ({} descriptors):", descriptors.len());
    log::info!(target: "memory_map_test", "====================");

    let mut total_memory = 0u64;
    let mut type_counts: BTreeMap<u32, (usize, u64)> = BTreeMap::new();

    log::info!(target: "memory_map_test", "\n");
    log::info!(target: "memory_map_test", "All Descriptors Returned:");
    for (i, desc) in descriptors.iter().enumerate() {
        let size_mb = (desc.number_of_pages * UEFI_PAGE_SIZE as u64) / SIZE_1MB as u64;
        let size_bytes = desc.number_of_pages * UEFI_PAGE_SIZE as u64;

        log::info!(
            target: "memory_map_test",
            "  [{:2}] Type: {:2} | Base: 0x{:016X} | Pages: {:8} | Size: {:4} MB | Attr: 0x{:X}",
            i, desc.r#type, desc.physical_start, desc.number_of_pages, size_mb, desc.attribute
        );

        total_memory += size_bytes;

        let entry = type_counts.entry(desc.r#type).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += size_bytes;
    }

    log::info!(target: "memory_map_test", "\n");
    log::info!(target: "memory_map_test", "Memory Summary by Type:");
    log::info!(target: "memory_map_test", "  Total Memory: {} MB", total_memory / SIZE_1MB as u64);
    for (mem_type, (count, bytes)) in type_counts.iter() {
        let type_name = match *mem_type {
            efi::RESERVED_MEMORY_TYPE => "Reserved",
            efi::LOADER_CODE => "Loader Code",
            efi::LOADER_DATA => "Loader Data",
            efi::BOOT_SERVICES_CODE => "Boot Services Code",
            efi::BOOT_SERVICES_DATA => "Boot Services Data",
            efi::RUNTIME_SERVICES_CODE => "Runtime Services Code",
            efi::RUNTIME_SERVICES_DATA => "Runtime Services Data",
            efi::CONVENTIONAL_MEMORY => "Conventional Memory",
            efi::UNUSABLE_MEMORY => "Unusable",
            efi::ACPI_RECLAIM_MEMORY => "ACPI Reclaim",
            efi::ACPI_MEMORY_NVS => "ACPI NVS",
            efi::MEMORY_MAPPED_IO => "MMIO",
            efi::MEMORY_MAPPED_IO_PORT_SPACE => "MMIO Port Space",
            efi::PAL_CODE => "PAL Code",
            efi::PERSISTENT_MEMORY => "Persistent Memory",
            _ => "Unknown",
        };
        log::info!(target: "memory_map_test", "  {:<25} [{:2}]: {:3} descriptors, {:8} MB", type_name, mem_type, count, bytes / SIZE_1MB as u64);
    }
    log::info!(target: "memory_map_test", "====================");
    log::info!(target: "memory_map_test", "\n");
}
