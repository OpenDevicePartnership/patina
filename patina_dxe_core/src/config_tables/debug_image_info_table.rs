//! EFI_DEBUG_IMAGE_INFO_TABLE Support
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;
use alloc::{boxed::Box, vec, vec::Vec};
use patina::base::UEFI_PAGE_SIZE;

use core::{
    ffi::c_void,
    fmt::Debug,
    mem::size_of,
    ptr,
    sync::atomic::{AtomicPtr, AtomicU64, Ordering},
};

use crate::{
    GCD, config_tables::core_install_configuration_table, gcd::AllocateType, protocol_db, systemtables::EfiSystemTable,
};

use patina_pi::dxe_services::GcdMemoryType;

use r_efi::efi;

// to be sent upstream to r_efi

/// GUID for the EFI_DEBUG_IMAGE_INFO_TABLE per section 18.4.3 of UEFI Spec 2.11
pub const EFI_DEBUG_IMAGE_INFO_TABLE_GUID: efi::Guid =
    efi::Guid::from_fields(0x49152e77, 0x1ada, 0x4764, 0xb7, 0xa2, &[0x7a, 0xfe, 0xfe, 0xd9, 0x5e, 0x8b]);

/// Structure for EFI_DEBUG_IMAGE_INFO_NORMAL, per section 18.4.3 of UEFI Spec 2.11
/// This structure is used to store information about a loaded image for debugging purposes.
#[repr(C)]
#[derive(Debug)]
pub struct EfiDebugImageInfoNormal {
    pub image_info_type: u32,
    pub loaded_image_protocol_instance: *const efi::protocols::loaded_image::Protocol,
    pub image_handle: efi::Handle,
}

impl EfiDebugImageInfoNormal {
    /// UEFI spec defined constant for the image info type field in the EfiDebugImageInfoNormal structure
    pub const EFI_DEBUG_IMAGE_INFO_TYPE_NORMAL: u32 = 0x1;
}

/// Union for EFI_DEBUG_IMAGE_INFO, per section 18.4.3 of Uefi Spec 2.11
#[repr(C)]
#[derive(Copy, Clone)]
pub union EfiDebugImageInfo {
    image_info_type: *const u32,
    normal_image: *const EfiDebugImageInfoNormal,
}

/// Structure for the EFI_DEBUG_IMAGE_INFO_TABLE, per section 18.4.3 of UEFI Spec 2.11
#[repr(C)]
#[derive(Debug)]
pub struct DebugImageInfoTableHeader {
    update_status: u32, // This is made not pub to force volatile access to the field, per UEFI spec
    pub table_size: u32,
    pub efi_debug_image_info_table: *const EfiDebugImageInfo,
}

/// The update status field in the DebugImageInfoTableHeader is used to indicate the status of the table and per
/// UEFI spec, it should be accessed using volatile reads and writes to ensure that the debugger can read it.
/// The only way to guarantee this in Rust is to force volatile reads and writes; the member cannot be made volatile
impl DebugImageInfoTableHeader {
    /// UEFI spec defined constants for the update status field in the DebugImageInfoTableHeader
    pub const EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS: u32 = 0x1;
    pub const EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED: u32 = 0x2;

    /// Returns the current update status of the DebugImageInfoTableHeader.
    pub unsafe fn get_update_status(&self) -> u32 {
        unsafe { ptr::read_volatile(&self.update_status) }
    }

    /// Sets the update status of the DebugImageInfoTableHeader.
    pub unsafe fn set_update_status(&mut self, status: u32) {
        unsafe { ptr::write_volatile(&mut self.update_status as *mut u32, status) }
    }
}

/// Structure for the EFI_SYSTEM_TABLE_POINTER, per section 18.4.2 of UEFI Spec 2.11.
#[allow(unused)]
pub struct EfiSystemTablePointer {
    pub signature: u64,
    pub efi_system_table_base: efi::PhysicalAddress,
    pub crc32: u32,
}

// end to be sent upstream to r_efi

const IMAGE_INFO_TABLE_SIZE: usize = 128; // initial size of the table

/// Metadata structure for the DebugImageInfoTable, which contains the actual table and its size. It is only used
/// internally to manage the table and is not part of the UEFI spec.
struct DebugImageInfoTableMetadata<'a> {
    actual_table_size: u32,
    table: &'a mut DebugImageInfoTableHeader,
    slice: Box<[EfiDebugImageInfo]>,
}

static METADATA_TABLE: AtomicPtr<DebugImageInfoTableMetadata> = AtomicPtr::new(core::ptr::null_mut());

const ALIGNMENT_SHIFT_4MB: usize = 22;

static DBG_SYSTEM_TABLE_POINTER_ADDRESS: AtomicU64 = AtomicU64::new(0);

/// Initializes the EFI_DEBUG_IMAGE_INFO_TABLE_GUID configuration table in the UEFI system table with an empty table.
pub(crate) fn initialize_debug_image_info_table(system_table: &mut EfiSystemTable) {
    let initial_table =
        vec![EfiDebugImageInfo { normal_image: core::ptr::null() }; IMAGE_INFO_TABLE_SIZE].into_boxed_slice();

    let debug_image_info_table_header = Box::new(DebugImageInfoTableHeader {
        update_status: 0,
        table_size: 0,
        efi_debug_image_info_table: initial_table.as_ptr(),
    });

    let table_ptr = Box::into_raw(debug_image_info_table_header) as *mut c_void;
    if core_install_configuration_table(EFI_DEBUG_IMAGE_INFO_TABLE_GUID, table_ptr, system_table).is_err() {
        log::error!("Failed to install configuration table for EFI_DEBUG_IMAGE_INFO_TABLE_GUID");
        return;
    };

    // SAFETY: This is safe because we just allocated the table and we are going to use it immediately
    let table = Box::new(DebugImageInfoTableMetadata {
        actual_table_size: IMAGE_INFO_TABLE_SIZE as u32,
        table: unsafe { &mut *table_ptr.cast::<DebugImageInfoTableHeader>() },
        slice: initial_table,
    });
    METADATA_TABLE.store(Box::into_raw(table), Ordering::SeqCst);

    // Now create the EFI_SYSTEM_TABLE_POINTER structure
    let system_table_pointer = system_table.system_table() as *const _ as u64;

    // we need to align the the pointer to 4MB and near the top of memory
    let address = match GCD.allocate_memory_space(
        AllocateType::TopDown(None),
        GcdMemoryType::SystemMemory,
        ALIGNMENT_SHIFT_4MB,
        UEFI_PAGE_SIZE,
        protocol_db::DXE_CORE_HANDLE,
        None,
    ) {
        Ok(address) => address,
        Err(_) => return,
    };

    let ptr = address as *mut EfiSystemTablePointer;

    // SAFETY: This is safe because we just allocated this. We have to do a volatile write because we don't use this
    // pointer, an external debugger does
    unsafe {
        ptr::write_volatile(
            ptr,
            EfiSystemTablePointer {
                signature: efi::SYSTEM_TABLE_SIGNATURE,
                efi_system_table_base: system_table_pointer,
                crc32: 0,
            },
        );

        let crc32 = crc32fast::hash(alloc::slice::from_raw_parts(ptr as *const u8, size_of::<EfiSystemTablePointer>()));

        ptr::write_volatile(&mut (*ptr).crc32, crc32);
    }

    // Set the system table address for the debugger.
    DBG_SYSTEM_TABLE_POINTER_ADDRESS.store(address as u64, Ordering::Relaxed);

    patina_debugger::add_monitor_command("system_table_ptr", "Prints the system table pointer", |_, out| {
        let address = DBG_SYSTEM_TABLE_POINTER_ADDRESS.load(Ordering::Relaxed);
        let _ = write!(out, "{address:x}");
    });
}

/// This function is called upon image load to create a new entry in the EFI_DEBUG_IMAGE_INFO_TABLE_GUID table.
pub(crate) fn core_new_debug_image_info_entry(
    image_info_type: u32,
    loaded_image_protocol_instance: *const efi::protocols::loaded_image::Protocol,
    image_handle: efi::Handle,
) {
    // This is a very funny check for null because it is working around an LLVM bug where checking is_null() or variations
    // of that on a load of an atomic pointer causes improper code generation and LLVM to crash. So, this check is a workaround
    // to check if the pointer is in the first page of memory, which is a valid check for null in this case, as we mark
    // that entire page as invalid. LLVM issue: https://github.com/llvm/llvm-project/issues/137152.
    let metadata_table = METADATA_TABLE.load(Ordering::SeqCst);
    if metadata_table < UEFI_PAGE_SIZE as *mut DebugImageInfoTableMetadata {
        log::error!("EFI_DEBUG_IMAGE_INFO_TABLE_GUID table not initialized");
        return;
    }

    // SAFETY: This is safe because we check that the table is initialized above
    let metadata_table = unsafe { &mut *(metadata_table) };

    // per UEFI spec, need to mark the table is being updated and preserve the modified bit if set
    // SAFETY: This is safe because we are accessing the table header and we ensure that it is initialized
    let update_status = unsafe { metadata_table.table.get_update_status() };
    unsafe {
        metadata_table
            .table
            .set_update_status(update_status | DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS)
    };

    // create our new table
    if metadata_table.table.table_size >= metadata_table.actual_table_size {
        // We need to allocate more space for the table
        let new_table_size = metadata_table.table.table_size + IMAGE_INFO_TABLE_SIZE as u32;
        let old_table_size = metadata_table.table.table_size;

        let mut new_vec = Vec::with_capacity(new_table_size as usize);
        new_vec.extend_from_slice(&metadata_table.slice[..old_table_size as usize]);
        new_vec.extend(core::iter::repeat_n(
            EfiDebugImageInfo { normal_image: core::ptr::null() },
            (new_table_size - old_table_size) as usize,
        ));
        let new_boxed_slice = new_vec.into_boxed_slice();
        metadata_table.slice = new_boxed_slice;

        metadata_table.actual_table_size = new_table_size;
        metadata_table.table.efi_debug_image_info_table = metadata_table.slice.as_ptr();
    }

    // size here is last_index + 1
    // SAFETY: This is safe because we are accessing the table header and we ensure that it is initialized
    let debug_image_info = &mut metadata_table.slice[metadata_table.table.table_size as usize];
    let debug_image_info_table =
        Box::new(EfiDebugImageInfoNormal { image_info_type, loaded_image_protocol_instance, image_handle });

    debug_image_info.normal_image = Box::leak(debug_image_info_table);
    metadata_table.table.table_size += 1;

    // SAFETY: This is safe because we are accessing the table header and we ensure that it is initialized
    unsafe {
        let update_status = metadata_table.table.get_update_status();
        metadata_table.table.set_update_status(
            (update_status & !DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS)
                | DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED,
        )
    };
}

/// This function is called on image unload to remove an entry from the EFI_DEBUG_IMAGE_INFO_TABLE_GUID table.
pub(crate) fn core_remove_debug_image_info_entry(image_handle: efi::Handle) {
    // This is a very funny check for null because it is working around an LLVM bug where checking is_null() or variations
    // of that on a load of an atomic pointer causes improper code generation and LLVM to crash. So, this check is a workaround
    // to check if the pointer is in the first page of memory, which is a valid check for null in this case, as we mark
    // that entire page as invalid. LLVM issue: https://github.com/llvm/llvm-project/issues/137152.
    let metadata_table = METADATA_TABLE.load(Ordering::SeqCst);
    if metadata_table < UEFI_PAGE_SIZE as *mut DebugImageInfoTableMetadata {
        log::error!("EFI_DEBUG_IMAGE_INFO_TABLE_GUID table not initialized");
        return;
    }

    // SAFETY: This is safe because we check that the table is initialized above
    let metadata_table = unsafe { &mut *(metadata_table) };

    // per UEFI spec, need to mark the table is being updated and preserve the modified bit if set
    // SAFETY: This is safe because we are accessing the table header and we ensure that it is initialized
    let update_status = unsafe { metadata_table.table.get_update_status() };
    unsafe {
        metadata_table
            .table
            .set_update_status(update_status | DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS)
    };

    let table_size = metadata_table.table.table_size as usize;

    // Take the pointer from the last entry before the loop to avoid double mutable borrow
    let mut last_normal_image_ptr: *const EfiDebugImageInfoNormal = core::ptr::null();
    if table_size > 0 {
        last_normal_image_ptr = unsafe { metadata_table.slice[table_size - 1].normal_image };
    }

    // find the entry to remove
    for i in 0..table_size {
        let debug_image_info = &mut metadata_table.slice[i];

        // SAFETY: This is safe because we are accessing the table and we ensure that it is initialized
        let debug_image_info_table = unsafe { &*debug_image_info.normal_image };
        if debug_image_info_table.image_handle == image_handle {
            // free the entry by reclaiming it and dropping the Box. The box should go out of scope if we didn't
            // manually call drop, but let's be explicit since this is the operation we are attempting to do.
            // SAFETY: This is safe because we are accessing the table and we ensure that it is initialized
            let boxed_debug_image_info_table =
                unsafe { Box::from_raw(debug_image_info.normal_image as *mut EfiDebugImageInfoNormal) };
            drop(boxed_debug_image_info_table);

            if i != table_size - 1 {
                // if this is not the last entry, we need to move the last entry to this position
                // SAFETY: This is safe because we are accessing the table and we ensure that it is initialized
                debug_image_info.normal_image = last_normal_image_ptr;
            }

            // we either have moved the last entry to this position or we are removing the last entry, in either case
            // we need to update the table size and the last entry
            metadata_table.slice[table_size - 1].normal_image = core::ptr::null_mut();
            metadata_table.table.table_size -= 1;
            break;
        }
    }

    // SAFETY: This is safe because we are accessing the table header and we ensure that it is initialized
    unsafe {
        let update_status = metadata_table.table.get_update_status();
        metadata_table.table.set_update_status(
            (update_status & !DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS)
                | DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED,
        )
    };
}
