//! DXE Core Memory Attributes Table (MAT)
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;
use alloc::vec::Vec;

#[cfg(not(test))]
use spin::Once;

use core::{ffi::c_void, fmt::Debug, mem::size_of, slice};

use crate::{
    GCD,
    allocator::{MemoryDescriptorSlice, core_allocate_pool, core_free_pool},
    config_tables::{core_install_configuration_table, get_configuration_table},
    events::EVENT_DB,
    gcd::MemoryProtectionPolicy,
    systemtables,
};
use patina::standard::efi;
use patina::writelncrlf;

// create a wrapper struct so that we can create an install method on it. That way, we can have the install function
// be a no-op until after ReadyToBoot
pub struct MemoryAttributesTable(*mut efi::MemoryAttributesTable);

// this is a flag to indicate that we have passed ReadyToBoot and can install the MAT on the next runtime memory
// allocation/deallocation.
#[cfg(not(test))]
static POST_RTB: Once<()> = Once::new();
#[cfg(test)]
static POST_RTB: crate::test_support::TestOnce<()> = crate::test_support::TestOnce::new();

impl MemoryAttributesTable {
    ///
    /// Install the Memory Attributes Table
    /// This function is intended to be called by the DXE Core to install the Memory Attributes Table for runtime memory
    /// allocations/deallocations after `ReadyToBoot` has occurred. This function will be a no-op until after `ReadyToBoot`.
    /// Callers of the function are not expected to check return status as it is immaterial to the caller whether it
    /// succeeds or not and they will take no different action based on return status.
    ///
    /// ## Example
    ///
    /// ```ignore
    /// use patina_dxe_core::memory_attributes_table::MemoryAttributesTable;
    /// // do a runtime memory allocation/deallocation here that succeeds in getting a new page or freeing a page
    /// MemoryAttributesTable::install();
    /// // continue allocator logic
    /// ```
    ///
    pub fn install() {
        if POST_RTB.is_completed() {
            core_install_memory_attributes_table();
        }
    }
}

impl Debug for MemoryAttributesTable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // SAFETY: self.0 is a valid MAT pointer when Debug is invoked.
        let mat = unsafe { self.0.as_ref().expect("BAD MAT PTR") };
        // SAFETY: mat.entry points to number_of_entries descriptors in the MAT.
        let entries = unsafe { slice::from_raw_parts(mat.entry.as_ptr(), mat.number_of_entries as usize) };

        writelncrlf!(f, "MemoryAttributesTable {{")?;
        writelncrlf!(f, "  version: {:#X}", mat.version)?;
        writelncrlf!(f, "  number_of_entries: {:#X}", mat.number_of_entries)?;
        writelncrlf!(f, "  descriptor_size: {:#X}", mat.descriptor_size)?;
        writelncrlf!(f, "  flags: {:#X}", mat.flags)?;
        writelncrlf!(f, "  entries: [")?;

        writelncrlf!(f, "{:?}", MemoryDescriptorSlice(entries))?;

        writelncrlf!(f, "  ]")?;
        writelncrlf!(f, "}}")
    }
}

// this function is intended to be called by dxe_main to set up the event to create the MAT for the first time
// on Ready to Boot.
pub fn init_memory_attributes_table_support() {
    if let Err(status) = EVENT_DB.create_event(
        efi::EVT_NOTIFY_SIGNAL,
        efi::TPL_CALLBACK,
        Some(core_install_memory_attributes_table_event_wrapper),
        None,
        Some(efi::EVENT_GROUP_READY_TO_BOOT),
    ) {
        log::error!("Failed to register an event at Ready to Boot to create the MAT! Status {status}");
    }
}

// this callback is invoked on ready to boot to install the memory attributes table for the first time.
// After this point, subsequent runtime memory allocations/deallocations will create new MAT tables
extern "efiapi" fn core_install_memory_attributes_table_event_wrapper(event: efi::Event, _context: *mut c_void) {
    core_install_memory_attributes_table();

    if let Err(status) = EVENT_DB.close_event(event) {
        log::error!("Failed to close MAT ready to boot event with status {status}. This should be okay.");
    }
}

pub fn core_install_memory_attributes_table() {
    if !POST_RTB.is_completed() {
        if get_configuration_table(&efi::MEMORY_ATTRIBUTES_TABLE_GUID).is_none() {
            // we need to install an empty configuration table the first time here, because core_install_configuration_table
            // may allocate runtime memory. Because it actually gets installed we need to allocate one here, it will be
            // freed below when we install the real MAT. If we don't allocate this on the heap, we may have undefined
            // behavior with a stack pointer that goes out of scope
            match core_allocate_pool(efi::BOOT_SERVICES_DATA, size_of::<efi::MemoryAttributesTable>()) {
                Ok(empty_ptr) => {
                    // SAFETY: empty_ptr is a valid allocation for MemoryAttributesTable.
                    if let Some(empty_mat) = unsafe { (empty_ptr as *mut efi::MemoryAttributesTable).as_mut() } {
                        *empty_mat = efi::MemoryAttributesTable {
                            version: 0,
                            number_of_entries: 0,
                            descriptor_size: 0,
                            flags: 0,
                            entry: [],
                        };
                        let mut st_guard = systemtables::SYSTEM_TABLE.lock();
                        let st = st_guard.as_mut().expect("System table support not initialized");

                        if let Err(status) =
                            core_install_configuration_table(efi::MEMORY_ATTRIBUTES_TABLE_GUID, empty_ptr, st)
                        {
                            log::error!("Failed to create a null MAT table with status {status}, cannot create MAT");
                            return;
                        }
                    }
                }
                Err(err) => {
                    log::error!("Failed to allocate memory for a null MAT! Status {err}");
                    return;
                }
            }
        }
        // now we want to capture any future runtime memory changes, so we will mark that ReadyToBoot has occurred
        // and the install callback will be invoked on the next runtime memory allocation
        POST_RTB.call_once(|| {});
    }

    // Reserve a buffer for the memory map descriptors
    let descriptor_count = GCD.memory_descriptor_count();
    let mut desc_buffer = Vec::with_capacity(descriptor_count);
    desc_buffer.resize(
        descriptor_count,
        efi::MemoryDescriptor { r#type: 0, physical_start: 0, virtual_start: 0, number_of_pages: 0, attribute: 0 },
    );

    let Ok(actual_count) = GCD.populate_efi_memory_map(&mut desc_buffer, true) else {
        log::error!("Failed to populate memory map descriptors.");
        return;
    };

    // Truncate to the actual number of descriptors written
    desc_buffer.truncate(actual_count);

    if desc_buffer.is_empty() {
        log::error!("Failed to install memory attributes table! Could not get memory map descriptors.");
        return;
    }

    // Filter for runtime sections and apply MAT policy
    let mat_desc_list: Vec<efi::MemoryDescriptor> = desc_buffer
        .iter()
        .filter_map(|descriptor| {
            // we only want the EfiRuntimeServicesCode and EfiRuntimeServicesData sections in the MAT
            match descriptor.r#type {
                efi::RUNTIME_SERVICES_CODE | efi::RUNTIME_SERVICES_DATA => {
                    Some(efi::MemoryDescriptor {
                        attribute: MemoryProtectionPolicy::apply_memory_attributes_table_policy(
                            descriptor.attribute,
                            descriptor.r#type,
                        ),
                        // use all other fields from the GCD descriptor
                        ..*descriptor
                    })
                }
                _ => None,
            }
        })
        .collect();

    // allocate memory for the MAT and publish it
    let buffer_size =
        mat_desc_list.len() * size_of::<efi::MemoryDescriptor>() + size_of::<efi::MemoryAttributesTable>();
    match core_allocate_pool(efi::BOOT_SERVICES_DATA, buffer_size) {
        Err(err) => {
            log::error!("Failed to allocate memory for the MAT! Status {err}");
            return;
        }
        Ok(void_ptr) => {
            let mat_descriptors_ptr = mat_desc_list.as_ptr() as *mut u8;
            let mat_ptr = void_ptr as *mut efi::MemoryAttributesTable;
            if mat_ptr.is_null() {
                log::error!("Got a null ptr in successful return from allocate_pool. Failed to create MAT.");
                return;
            }

            // this ends up being a large unsafe block because we have to dereference the raw pointer core_allocate_pool
            // gave us and convert it to a real type and back in order to install it
            // SAFETY: mat_ptr is a valid allocation for MemoryAttributesTable and copy size is bounded by buffer_size.
            unsafe {
                let mat = &mut *mat_ptr;
                mat.version = efi::MEMORY_ATTRIBUTES_TABLE_VERSION;
                mat.number_of_entries = mat_desc_list.len() as u32;
                mat.descriptor_size = size_of::<efi::MemoryDescriptor>() as u32;
                mat.flags = 0;

                let copy_ptr = core::ptr::from_ref(&mat.entry) as *mut u8;

                core::ptr::copy(
                    mat_descriptors_ptr,
                    copy_ptr,
                    mat_desc_list.len() * size_of::<efi::MemoryDescriptor>(),
                );

                let mut st_guard = systemtables::SYSTEM_TABLE.lock();
                let st = st_guard.as_mut().expect("System table support not initialized");

                match core_install_configuration_table(efi::MEMORY_ATTRIBUTES_TABLE_GUID, void_ptr, st) {
                    Err(status) => {
                        log::error!("Failed to install MAT table! Status {status}");
                        if let Err(err) = core_free_pool(void_ptr) {
                            log::error!("Error freeing newly allocated MAT pointer: {err}");
                        }
                        return;
                    }
                    Ok(Some(current_ptr)) => {
                        // free the old MAT table if we have one
                        if let Err(err) = core_free_pool(current_ptr.as_ptr()) {
                            log::error!("Error freeing previous MAT pointer: {err}");
                        }
                    }
                    Ok(None) => (),
                }
            }

            log::info!("Dumping MAT: {:?}", MemoryAttributesTable(mat_ptr));
        }
    }
    log::info!("Successfully installed MAT table!");
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    extern crate std;
    use super::*;

    use crate::{
        allocator::core_allocate_pages,
        dxe_services::{core_set_memory_space_attributes, core_set_memory_space_capabilities},
        systemtables::init_system_table,
        test_support,
    };
    use patina::{UEFI_PAGE_SIZE, uefi_size_to_pages};

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_clean_global_lock(|| {
            let _post_rtb_guard = test_support::StateGuard::new(|| POST_RTB.reset());
            POST_RTB.reset();

            // SAFETY: Test-only initialization under the global lock.
            unsafe {
                test_support::init_test_gcd(None);
                test_support::reset_allocators();
                init_system_table();
            }
            f();
        })
        .unwrap();
    }

    #[test]
    fn test_mat_init() {
        with_locked_state(|| {
            init_memory_attributes_table_support();
        });
    }

    #[test]
    fn test_memory_attributes_table_generation() {
        with_locked_state(|| {
            // Create a vector to store the allocated pages

            let mut allocated_pages = Vec::new();
            let mut entry_count = 0;

            // Simulate random calls to core_allocate_pages with different types
            for i in 0..15 {
                let page_type = match i % 3 {
                    0 => {
                        entry_count += 1;
                        (efi::RUNTIME_SERVICES_CODE, efi::MEMORY_RO | efi::MEMORY_RUNTIME)
                    }
                    1 => {
                        entry_count += 1;
                        (efi::RUNTIME_SERVICES_DATA, efi::MEMORY_XP | efi::MEMORY_RUNTIME)
                    }
                    _ => (efi::BOOT_SERVICES_DATA, efi::MEMORY_XP),
                };

                // Allocate in granularity chunks. Otherwise the allocation will auto-expand.
                let page_count =
                    entry_count * uefi_size_to_pages!(crate::allocator::RUNTIME_PAGE_ALLOCATION_GRANULARITY);

                let address = match core_allocate_pages(efi::ALLOCATE_ANY_PAGES, page_type.0, page_count, 0, None) {
                    // because we allocate top down, we need to insert at the front of the vector
                    Ok(address) => {
                        if page_type.0 != efi::BOOT_SERVICES_DATA {
                            allocated_pages.insert(0, (address, page_type, page_count));
                        }
                        address
                    }
                    _ => panic!("Failed to allocate pages"),
                };

                let len = page_count * UEFI_PAGE_SIZE;
                // ignore failures here, we can't set attributes in the actual page table here, but the GCD will
                // get updated
                let _ = core_set_memory_space_capabilities(address, len as u64, u64::MAX);
                let _ = core_set_memory_space_attributes(address, len as u64, page_type.1);
            }

            // before we create the MAT, we expect MEMORY_ATTRIBUTES_TABLE to be None
            assert!(get_configuration_table(&efi::MEMORY_ATTRIBUTES_TABLE_GUID).is_none());

            // Create a dummy event
            let dummy_event: efi::Event = core::ptr::null_mut();

            // Ensure POST_RTB is false before the event
            assert!(!POST_RTB.is_completed());

            // Call the event wrapper
            core_install_memory_attributes_table_event_wrapper(dummy_event, core::ptr::null_mut());

            // Check if POST_RTB is set after the event
            assert!(POST_RTB.is_completed());

            // Check if MEMORY_ATTRIBUTES_TABLE is set after installation
            let mat_ptr = get_configuration_table(&efi::MEMORY_ATTRIBUTES_TABLE_GUID).unwrap();
            // SAFETY: MAT pointer comes from configuration table and is validated before dereference.
            unsafe {
                let mat = &*(mat_ptr.as_ptr() as *const efi::MemoryAttributesTable);

                assert_eq!(mat.version, efi::MEMORY_ATTRIBUTES_TABLE_VERSION);
                // we have one extra entry here because init_system_table allocates runtime pages
                assert!(mat.number_of_entries == entry_count as u32 + 1);
                assert_eq!(mat.descriptor_size, size_of::<efi::MemoryDescriptor>() as u32);

                let entry_slice = slice::from_raw_parts(mat.entry.as_ptr(), mat.number_of_entries as usize);

                // Validate each of our runtime allocations exists in the MAT with expected values.
                // We don't assume ordering; find by physical_start and number_of_pages.
                for page in &allocated_pages {
                    let expected_type = page.1.0;
                    let expected_physical_start = page.0;
                    let expected_number_of_pages = page.2 as u64;
                    // expected_attribute from setup isn't used directly; MAT constrains attrs based on type.

                    let entry = entry_slice
                        .iter()
                        .find(|e| {
                            e.physical_start == expected_physical_start && e.number_of_pages == expected_number_of_pages
                        })
                        .expect("Expected MAT entry not found for allocated runtime pages");

                    assert_eq!(entry.r#type, expected_type);
                    assert_eq!(entry.virtual_start, 0);
                    // Attributes in MAT may include additional allowed bits depending on current GCD state.
                    // Enforce type-appropriate expectations instead of exact masks:
                    // - Always require MEMORY_RUNTIME for runtime sections.
                    // - For CODE, require either RO or XP is present (some platforms default XP in GCD).
                    // - For DATA, require XP is present (RO isn't required for data).
                    assert!(entry.attribute & efi::MEMORY_RUNTIME != 0, "MAT entry missing MEMORY_RUNTIME");
                    match expected_type {
                        efi::RUNTIME_SERVICES_CODE => {
                            assert!(
                                entry.attribute & (efi::MEMORY_RO | efi::MEMORY_XP) != 0,
                                "Runtime code entry missing RO/XP; attrs={:#X}",
                                entry.attribute
                            );
                        }
                        efi::RUNTIME_SERVICES_DATA => {
                            assert!(
                                entry.attribute & efi::MEMORY_XP != 0,
                                "Runtime data entry missing XP; attrs={:#X}",
                                entry.attribute
                            );
                        }
                        _ => {}
                    }
                }
            }
        });
    }
}
