//! DXE Core Memory Attributes Table (MAT)
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
extern crate alloc;
use alloc::vec::Vec;

use core::{
    ffi::c_void,
    fmt::Debug,
    mem::size_of,
    slice,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use crate::{
    allocator::{core_allocate_pool, core_free_pool, get_memory_map_descriptors, MemoryDescriptorSlice},
    boot_services::with_event_db,
    misc_boot_services::core_install_configuration_table,
    systemtables,
};
use r_efi::efi;

// We cache the MAT here because we need to free it in whenever we get a new runtime code/data allocation
static MEMORY_ATTRIBUTES_TABLE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

// create a wrapper struct so that we can create an install method on it. That way, we can have the install function
// be a no-op until after ReadyToBoot
pub struct MemoryAttributesTable(*mut efi::MemoryAttributesTable);

// this is a flag to indicate that we have passed ReadyToBoot and can install the MAT on the next runtime memory
// allocation/deallocation
static POST_RTB: AtomicBool = AtomicBool::new(false);

impl MemoryAttributesTable {
    ///
    /// Install the Memory Attributes Table
    /// This function is intended to be called by the DXE Core to install the Memory Attributes Table for runtime memory
    /// allocations/deallocations after ReadyToBoot has occurred. This function will be a no-op until after ReadyToBoot.
    /// Callers of the function are not expected to check return status as it is immaterial to the caller whether it
    /// succeeds or not and they will take no different action based on return status.
    ///
    /// ## Example
    ///
    /// ```ignore
    /// use dxe_core::memory_attributes_table::MemoryAttributesTable;
    /// // do a runtime memory allocation/deallocation here that succeeds in getting a new page or freeing a page
    /// MemoryAttributesTable::install();
    /// // continue allocator logic
    /// ```
    ///
    pub fn install() {
        if POST_RTB.load(Ordering::Relaxed) {
            core_install_memory_attributes_table()
        }
    }
}

impl Debug for MemoryAttributesTable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mat = unsafe { self.0.as_ref().expect("BAD MAT PTR") };
        let entries = unsafe { slice::from_raw_parts(mat.entry.as_ptr(), mat.number_of_entries as usize) };

        writeln!(f, "MemoryAttributesTable {{")?;
        writeln!(f, "  version: {:#X}", mat.version)?;
        writeln!(f, "  number_of_entries: {:#X}", mat.number_of_entries)?;
        writeln!(f, "  descriptor_size: {:#X}", mat.descriptor_size)?;
        writeln!(f, "  reserved: {:#X}", mat.reserved)?;
        writeln!(f, "  entries: [")?;

        writeln!(f, "{:?}", MemoryDescriptorSlice(entries))?;

        writeln!(f, "  ]")?;
        writeln!(f, "}}")
    }
}

// this function is intended to be called by dxe_main to set up the event to create the MAT for the first time
// on Ready to Boot.
pub fn init_memory_attributes_table_support() {
    if let Err(status) = with_event_db(|db| {
        db.create_event(
            efi::EVT_NOTIFY_SIGNAL,
            efi::TPL_CALLBACK,
            Some(core_install_memory_attributes_table_event_wrapper),
            None,
            Some(efi::EVENT_GROUP_READY_TO_BOOT),
        )
    }) {
        log::error!("Failed to register an event at Ready to Boot to create the MAT! Status {:#X?}", status);
    }
}

// this callback is invoked on ready to boot to install the memory attributes table for the first time.
// After this point, subsequent runtime memory allocations/deallocations will create new MAT tables
extern "efiapi" fn core_install_memory_attributes_table_event_wrapper(event: efi::Event, _context: *mut c_void) {
    core_install_memory_attributes_table();
    // now we want to capture any future runtime memory changes, so we will mark that ReadyToBoot has occurred
    // and the install callback will be invoked on the next runtime memory allocation
    POST_RTB.store(true, Ordering::Relaxed);

    if let Err(status) = with_event_db(|db| db.close_event(event)) {
        log::error!("Failed to close MAT ready to boot event with status {:#X?}. This should be okay.", status);
    }
}

pub fn core_install_memory_attributes_table() {
    let mut st_guard = systemtables::SYSTEM_TABLE.lock();
    let st = st_guard.as_mut().expect("System table support not initialized");

    let current_ptr = MEMORY_ATTRIBUTES_TABLE.load(Ordering::Relaxed);
    if current_ptr.is_null() {
        // we need to install an empty configuration table the first time here, because core_install_configuration_table
        // may allocate runtime memory. Because it actually gets installed we need to allocate one here, it will be
        // freed below when we install the real MAT. If we don't allocate this on the heap, we may have undefined
        // behavior with a stack pointer that goes out of scope
        match core_allocate_pool(efi::BOOT_SERVICES_DATA, size_of::<efi::MemoryAttributesTable>()) {
            Ok(empty_ptr) => {
                if let Some(empty_mat) = unsafe { (empty_ptr as *mut efi::MemoryAttributesTable).as_mut() } {
                    *empty_mat = efi::MemoryAttributesTable {
                        version: 0,
                        number_of_entries: 0,
                        descriptor_size: 0,
                        reserved: 0,
                        entry: [],
                    };
                    MEMORY_ATTRIBUTES_TABLE.store(empty_ptr, Ordering::Relaxed);

                    // it is unsafe to get a mutable reference to the MAT here, but we know that we have a valid ptr
                    unsafe {
                        if let Err(status) =
                            core_install_configuration_table(efi::MEMORY_ATTRIBUTES_TABLE_GUID, empty_ptr.as_mut(), st)
                        {
                            log::error!(
                                "Failed to create a null MAT table with status {:#X?}, cannot create MAT",
                                status
                            );
                            return;
                        }
                    }
                }
            }
            Err(err) => {
                log::error!("Failed to allocate memory for a null MAT! Status {:#X?}", err);
                return;
            }
        }
    }

    // get the GCD memory map descriptors and filter out the non-runtime sections
    let desc_list = match get_memory_map_descriptors() {
        Ok(descriptors) => descriptors,
        Err(_) => {
            log::error!("Failed to get memory map descriptors.");
            return;
        }
    };
    let mat_allowed_attrs = efi::MEMORY_RO | efi::MEMORY_XP | efi::MEMORY_RUNTIME;

    if desc_list.is_empty() {
        log::error!("Failed to install memory attributes table! Could not get memory map descriptors.");
        return;
    }

    // this allocates memory to do the collect, but that's okay because it is boot services memory
    let mat_desc_list: Vec<efi::MemoryDescriptor> = desc_list
        .iter()
        .filter_map(|descriptor| {
            // we only want the EfiRuntimeServicesCode and EfiRuntimeServicesData sections in the MAT
            match descriptor.r#type {
                efi::RUNTIME_SERVICES_CODE | efi::RUNTIME_SERVICES_DATA => {
                    Some(efi::MemoryDescriptor {
                        attribute: match descriptor.attribute & (efi::MEMORY_RO | efi::MEMORY_XP) {
                            // if we don't have any attributes set here, we should mark code as RO and XP. These are
                            // likely extra sections in the memory bins and so should not be used
                            // Data we will mark as XP only, as likely the caching attributes were changed, which
                            // dropped the XP attribute, so we need to set it here.
                            0 if descriptor.r#type == efi::RUNTIME_SERVICES_CODE => mat_allowed_attrs,
                            0 if descriptor.r#type == efi::RUNTIME_SERVICES_DATA => {
                                efi::MEMORY_RUNTIME | efi::MEMORY_XP
                            }
                            _ => descriptor.attribute & mat_allowed_attrs,
                        },
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
            log::error!("Failed to allocate memory for the MAT! Status {:#X?}", err);
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
            unsafe {
                let mat = &mut *mat_ptr;
                mat.version = efi::MEMORY_ATTRIBUTES_TABLE_VERSION;
                mat.number_of_entries = mat_desc_list.len() as u32;
                mat.descriptor_size = size_of::<efi::MemoryDescriptor>() as u32;
                mat.reserved = 0;

                let copy_ptr = core::ptr::from_ref(&mat.entry) as *mut u8;

                core::ptr::copy(
                    mat_descriptors_ptr,
                    copy_ptr,
                    mat_desc_list.len() * size_of::<efi::MemoryDescriptor>(),
                );

                match core_install_configuration_table(efi::MEMORY_ATTRIBUTES_TABLE_GUID, void_ptr.as_mut(), st) {
                    Err(status) => {
                        log::error!("Failed to install MAT table! Status {:#X?}", status);
                        if let Err(err) = core_free_pool(void_ptr) {
                            log::error!("Error freeing newly allocated MAT pointer: {:#X?}", err);
                        }
                        return;
                    }

                    Ok(_) => {
                        // free the old MAT table if we have one
                        let current_ptr = MEMORY_ATTRIBUTES_TABLE.load(Ordering::Relaxed);
                        if !current_ptr.is_null() {
                            if let Err(err) = core_free_pool(current_ptr) {
                                log::error!("Error freeing previous MAT pointer: {:#X?}", err);
                            }
                        }
                        MEMORY_ATTRIBUTES_TABLE.store(void_ptr, Ordering::Relaxed);
                    }
                }
            }

            log::info!("Dumping MAT: {:?}", MemoryAttributesTable(mat_ptr));
        }
    }
    log::info!("Successfully installed MAT table!");
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    use crate::{
        allocator::core_allocate_pages,
        dxe_services::{core_set_memory_space_attributes, core_set_memory_space_capabilities},
        systemtables::init_system_table,
        test_support,
    };

    fn with_locked_state<F: Fn()>(f: F) {
        test_support::with_global_lock(|| {
            POST_RTB.store(false, Ordering::Relaxed);
            MEMORY_ATTRIBUTES_TABLE.store(core::ptr::null_mut(), Ordering::Relaxed);

            unsafe {
                test_support::init_test_gcd(None);
                init_system_table();
            }
            f();
        });
    }

    #[test]
    fn test_mat_init() {
        init_memory_attributes_table_support();
    }

    #[test]
    fn test_memory_attributes_table_generation() {
        with_locked_state(|| {
            const UEFI_PAGE_SIZE: u64 = 0x1000;
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

                let mut buffer_ptr: *mut u8 = core::ptr::null_mut();
                match core_allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    page_type.0,
                    entry_count as usize + 0x1,
                    core::ptr::addr_of_mut!(buffer_ptr) as *mut efi::PhysicalAddress,
                ) {
                    // because we allocate top down, we need to insert at the front of the vector
                    Ok(_) if page_type.0 != efi::BOOT_SERVICES_DATA => {
                        allocated_pages.insert(0, (buffer_ptr, page_type, entry_count + 1))
                    }
                    Ok(_) => (),
                    _ => panic!("Failed to allocate pages"),
                }

                let len = (entry_count + 1) * UEFI_PAGE_SIZE;
                // ignore failures here, we can't set attributes in the actual page table here, but the GCD will
                // get updated
                let _ = core_set_memory_space_capabilities(buffer_ptr as u64, len, u64::MAX);
                let _ = core_set_memory_space_attributes(buffer_ptr as u64, len, page_type.1);
            }

            // before we create the MAT, we expect MEMORY_ATTRIBUTES_TABLE to be None
            assert!(MEMORY_ATTRIBUTES_TABLE.load(Ordering::Relaxed).is_null());

            // Create a dummy event
            let dummy_event: efi::Event = core::ptr::null_mut();

            // Ensure POST_RTB is false before the event
            assert!(!POST_RTB.load(Ordering::Relaxed));

            // Call the event wrapper
            core_install_memory_attributes_table_event_wrapper(dummy_event, core::ptr::null_mut());

            // Check if POST_RTB is set after the event
            assert!(POST_RTB.load(Ordering::Relaxed));

            // Check if MEMORY_ATTRIBUTES_TABLE is set after installation
            assert!(!MEMORY_ATTRIBUTES_TABLE.load(Ordering::Relaxed).is_null());
            let mat_ptr = MEMORY_ATTRIBUTES_TABLE.load(Ordering::Relaxed);
            unsafe {
                let mat = &*(mat_ptr as *const _ as *const efi::MemoryAttributesTable);

                assert_eq!(mat.version, efi::MEMORY_ATTRIBUTES_TABLE_VERSION);
                // we have one extra entry here because init_system_table allocates runtime pages
                // yes, this is annoying, but depending on which tests run first, the system table may or may not be
                // the first entry in the MAT
                assert!(mat.number_of_entries == entry_count as u32 + 1 || mat.number_of_entries == entry_count as u32);
                assert_eq!(mat.descriptor_size, size_of::<efi::MemoryDescriptor>() as u32);

                let mut entry_slice = slice::from_raw_parts(mat.entry.as_ptr(), mat.number_of_entries as usize);

                // ignore the first entry for the system table, we don't need to randomize this test
                // by checking it. Annoyingly, the system table is not guaranteed to be the first entry
                // if other tests run first, so we need to check for it.
                if entry_slice.len() == entry_count as usize + 1 {
                    entry_slice = &entry_slice[1..];
                }

                for (i, entry) in entry_slice.iter().enumerate() {
                    let expected_type = allocated_pages[i].1 .0;

                    let expected_physical_start = allocated_pages[i].0 as u64;
                    let expected_number_of_pages = allocated_pages[i].2;
                    let expected_attribute = allocated_pages[i].1 .1;

                    assert_eq!(entry.r#type, expected_type);
                    assert_eq!(entry.physical_start, expected_physical_start);
                    assert_eq!(entry.virtual_start, 0);
                    assert_eq!(entry.number_of_pages, expected_number_of_pages);
                    assert_eq!(entry.attribute, expected_attribute);
                }
            }
        });
    }
}
