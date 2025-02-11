//! DXE Core Firmware Volume (FV)
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use core::{
    ffi::c_void,
    mem::{self, size_of},
    slice,
};

use alloc::{boxed::Box, collections::BTreeMap};
use mu_pi::{
    fw_fs::{self, EfiFvbAttributes2, FirmwareVolume, SectionExtractor},
    hob,
};

use r_efi::efi;
use uefi_device_path::concat_device_path_to_boxed_slice;

use crate::{
    allocator::core_allocate_pool,
    protocols::{core_install_protocol_interface, PROTOCOL_DB},
    tpl_lock,
};

struct PrivateFvbData {
    _interface: Box<mu_pi::protocols::firmware_volume_block::Protocol>,
    physical_address: u64,
}

struct PrivateFvData {
    _interface: Box<mu_pi::protocols::firmware_volume::Protocol>,
    physical_address: u64,
}

enum PrivateDataItem {
    FvbData(PrivateFvbData),
    FvData(PrivateFvData),
}

struct PrivateGlobalData {
    fv_information: BTreeMap<*mut c_void, PrivateDataItem>,
    section_extractor: Option<Box<dyn SectionExtractor>>,
}

//access to private global data is only through mutex guard, so safe to mark sync/send.
unsafe impl Sync for PrivateGlobalData {}
unsafe impl Send for PrivateGlobalData {}

static PRIVATE_FV_DATA: tpl_lock::TplMutex<PrivateGlobalData> = tpl_lock::TplMutex::new(
    efi::TPL_NOTIFY,
    PrivateGlobalData { fv_information: BTreeMap::new(), section_extractor: None },
    "FvLock",
);

// FVB Protocol Functions
extern "efiapi" fn fvb_get_attributes(
    this: *mut mu_pi::protocols::firmware_volume_block::Protocol,
    attributes: *mut fw_fs::EfiFvbAttributes2,
) -> efi::Status {
    if attributes.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let private_data = PRIVATE_FV_DATA.lock();

    let fvb_data = match private_data.fv_information.get(&(this as *mut c_void)) {
        Some(PrivateDataItem::FvbData(fvb_data)) => fvb_data,
        Some(_) | None => return efi::Status::NOT_FOUND,
    };

    let fv = match unsafe { FirmwareVolume::new_from_address(fvb_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err,
    };

    unsafe { attributes.write(fv.attributes()) };

    efi::Status::SUCCESS
}

extern "efiapi" fn fvb_set_attributes(
    _this: *mut mu_pi::protocols::firmware_volume_block::Protocol,
    _attributes: *mut EfiFvbAttributes2,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

extern "efiapi" fn fvb_get_physical_address(
    this: *mut mu_pi::protocols::firmware_volume_block::Protocol,
    address: *mut efi::PhysicalAddress,
) -> efi::Status {
    if address.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let private_data = PRIVATE_FV_DATA.lock();

    let fvb_data = match private_data.fv_information.get(&(this as *mut c_void)) {
        Some(PrivateDataItem::FvbData(fvb_data)) => fvb_data,
        Some(_) | None => return efi::Status::NOT_FOUND,
    };

    unsafe { address.write(fvb_data.physical_address) };

    efi::Status::SUCCESS
}

extern "efiapi" fn fvb_get_block_size(
    this: *mut mu_pi::protocols::firmware_volume_block::Protocol,
    lba: efi::Lba,
    block_size: *mut usize,
    number_of_blocks: *mut usize,
) -> efi::Status {
    if block_size.is_null() || number_of_blocks.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let private_data = PRIVATE_FV_DATA.lock();

    let fvb_data = match private_data.fv_information.get(&(this as *mut c_void)) {
        Some(PrivateDataItem::FvbData(fvb_data)) => fvb_data,
        Some(_) | None => return efi::Status::NOT_FOUND,
    };

    let fv = match unsafe { FirmwareVolume::new_from_address(fvb_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err,
    };

    let lba: u32 = match lba.try_into() {
        Ok(lba) => lba,
        _ => return efi::Status::INVALID_PARAMETER,
    };

    let (size, remaining_blocks) = match fv.lba_info(lba) {
        Err(err) => return err,
        Ok((_, size, remaining_blocks)) => (size, remaining_blocks),
    };

    unsafe {
        block_size.write(size as usize);
        number_of_blocks.write(remaining_blocks as usize);
    }

    efi::Status::SUCCESS
}

extern "efiapi" fn fvb_read(
    this: *mut mu_pi::protocols::firmware_volume_block::Protocol,
    lba: efi::Lba,
    offset: usize,
    num_bytes: *mut usize,
    buffer: *mut core::ffi::c_void,
) -> efi::Status {
    if num_bytes.is_null() || buffer.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let private_data = PRIVATE_FV_DATA.lock();

    let fvb_data = match private_data.fv_information.get(&(this as *mut c_void)) {
        Some(PrivateDataItem::FvbData(fvb_data)) => fvb_data,
        Some(_) | None => return efi::Status::NOT_FOUND,
    };

    let fv = match unsafe { FirmwareVolume::new_from_address(fvb_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err,
    };

    let lba: u32 = match lba.try_into() {
        Ok(lba) => lba,
        _ => return efi::Status::INVALID_PARAMETER,
    };

    let (lba_base_addr, block_size) = match fv.lba_info(lba) {
        Err(err) => return err,
        Ok((base, block, _)) => (base as usize, block as usize),
    };

    let mut status = efi::Status::SUCCESS;

    let mut bytes_to_read = unsafe { *num_bytes };
    if offset + bytes_to_read > block_size {
        bytes_to_read = block_size - offset;
        status = efi::Status::BAD_BUFFER_SIZE;
    }

    let lba_start = (fvb_data.physical_address as usize + lba_base_addr + offset) as *mut u8;

    // copy from memory into the destination buffer to do the read.
    unsafe {
        let source_buffer = slice::from_raw_parts(lba_start, bytes_to_read);
        let dest_buffer = slice::from_raw_parts_mut(buffer as *mut u8, bytes_to_read);
        dest_buffer.copy_from_slice(source_buffer);

        num_bytes.write(bytes_to_read);
    }

    status
}

extern "efiapi" fn fvb_write(
    _this: *mut mu_pi::protocols::firmware_volume_block::Protocol,
    _lba: efi::Lba,
    _offset: usize,
    _num_bytes: *mut usize,
    _buffer: *mut core::ffi::c_void,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

extern "efiapi" fn fvb_erase_blocks(
    _this: *mut mu_pi::protocols::firmware_volume_block::Protocol,
    //... TODO: this should be variadic; however, variadic and eficall don't mix well presently.
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

fn install_fvb_protocol(
    handle: Option<efi::Handle>,
    parent_handle: Option<efi::Handle>,
    base_address: u64,
) -> Result<efi::Handle, efi::Status> {
    let mut fvb_interface = Box::from(mu_pi::protocols::firmware_volume_block::Protocol {
        get_attributes: fvb_get_attributes,
        set_attributes: fvb_set_attributes,
        get_physical_address: fvb_get_physical_address,
        get_block_size: fvb_get_block_size,
        read: fvb_read,
        write: fvb_write,
        erase_blocks: fvb_erase_blocks,
        parent_handle: match parent_handle {
            Some(handle) => handle,
            None => core::ptr::null_mut(),
        },
    });

    let fvb_ptr = fvb_interface.as_mut() as *mut mu_pi::protocols::firmware_volume_block::Protocol as *mut c_void;

    let private_data = PrivateFvbData { _interface: fvb_interface, physical_address: base_address };

    // save the protocol structure we're about to install in the private data.
    PRIVATE_FV_DATA.lock().fv_information.insert(fvb_ptr, PrivateDataItem::FvbData(private_data));

    // install the protocol and return status
    core_install_protocol_interface(handle, mu_pi::protocols::firmware_volume_block::PROTOCOL_GUID, fvb_ptr)
}

// Firmware Volume protocol functions
extern "efiapi" fn fv_get_volume_attributes(
    this: *const mu_pi::protocols::firmware_volume::Protocol,
    fv_attributes: *mut fw_fs::EfiFvAttributes,
) -> efi::Status {
    if fv_attributes.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let private_data = PRIVATE_FV_DATA.lock();

    let fv_data = match private_data.fv_information.get(&(this as *mut c_void)) {
        Some(PrivateDataItem::FvData(fv_data)) => fv_data,
        Some(_) | None => return efi::Status::NOT_FOUND,
    };

    let fv = match unsafe { FirmwareVolume::new_from_address(fv_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err,
    };

    unsafe { fv_attributes.write(fv.attributes() as fw_fs::EfiFvAttributes) };

    efi::Status::SUCCESS
}

extern "efiapi" fn fv_set_volume_attributes(
    _this: *const mu_pi::protocols::firmware_volume::Protocol,
    _fv_attributes: *mut fw_fs::EfiFvAttributes,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

extern "efiapi" fn fv_read_file(
    this: *const mu_pi::protocols::firmware_volume::Protocol,
    name_guid: *const efi::Guid,
    buffer: *mut *mut c_void,
    buffer_size: *mut usize,
    found_type: *mut fw_fs::EfiFvFileType,
    file_attributes: *mut fw_fs::EfiFvFileAttributes,
    authentication_status: *mut u32,
) -> efi::Status {
    if name_guid.is_null()
        || buffer_size.is_null()
        || found_type.is_null()
        || file_attributes.is_null()
        || authentication_status.is_null()
    {
        return efi::Status::INVALID_PARAMETER;
    }

    let local_buffer_size = unsafe { *buffer_size };
    let local_name_guid = unsafe { *name_guid };

    let private_data = PRIVATE_FV_DATA.lock();

    let fv_data = match private_data.fv_information.get(&(this as *mut c_void)) {
        Some(PrivateDataItem::FvData(fv_data)) => fv_data,
        Some(_) | None => return efi::Status::NOT_FOUND,
    };

    let fv = match unsafe { FirmwareVolume::new_from_address(fv_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err,
    };

    if (fv.attributes() & fw_fs::Fvb2RawAttributes::READ_STATUS) == 0 {
        return efi::Status::ACCESS_DENIED;
    }

    let file = match fv.file_iter().find(|f| f.as_ref().is_ok_and(|f| f.name() == local_name_guid) || f.is_err()) {
        Some(Ok(result)) => result,
        Some(Err(err)) => return err,
        _ => return efi::Status::NOT_FOUND,
    };

    // update file metadata output pointers.
    unsafe {
        found_type.write(file.file_type_raw());
        file_attributes.write(file.fv_attributes());
        //TODO: Authentication status is not yet supported.
        buffer_size.write(file.content().len());
    }

    if buffer.is_null() {
        //caller just wants file meta data, no need to read file data.
        return efi::Status::SUCCESS;
    }

    let mut local_buffer_ptr = unsafe { *buffer };

    if local_buffer_size > 0 {
        //caller indicates they have allocated a buffer to receive the file data.
        if local_buffer_size < file.content().len() {
            return efi::Status::BUFFER_TOO_SMALL;
        }
        if local_buffer_ptr.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }
    } else {
        //caller indicates that they wish to receive file data, but that this
        //routine should allocate a buffer of appropriate size. Since the caller
        //is expected to free this buffer via free_pool, we need to manually
        //allocate it via allocate_pool.
        match core_allocate_pool(efi::BOOT_SERVICES_DATA, file.content().len()) {
            Err(err) => return err,
            Ok(allocation) => unsafe {
                local_buffer_ptr = allocation;
                buffer.write(local_buffer_ptr);
            },
        }
    }

    //convert pointer+size into a slice and copy the file data.
    let out_buffer = unsafe { slice::from_raw_parts_mut(local_buffer_ptr as *mut u8, file.content().len()) };
    out_buffer.copy_from_slice(file.content());

    efi::Status::SUCCESS
}

extern "efiapi" fn fv_read_section(
    this: *const mu_pi::protocols::firmware_volume::Protocol,
    name_guid: *const efi::Guid,
    section_type: fw_fs::EfiSectionType,
    section_instance: usize,
    buffer: *mut *mut c_void,
    buffer_size: *mut usize,
    authentication_status: *mut u32,
) -> efi::Status {
    if name_guid.is_null() || buffer.is_null() || buffer_size.is_null() || authentication_status.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let local_name_guid = unsafe { *name_guid };

    let private_data = PRIVATE_FV_DATA.lock();

    let fv_data = match private_data.fv_information.get(&(this as *mut c_void)) {
        Some(PrivateDataItem::FvData(fv_data)) => fv_data,
        Some(_) | None => return efi::Status::NOT_FOUND,
    };

    let fv = match unsafe { fw_fs::FirmwareVolume::new_from_address(fv_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err,
    };

    if (fv.attributes() & fw_fs::Fvb2RawAttributes::READ_STATUS) == 0 {
        return efi::Status::ACCESS_DENIED;
    }

    let file = match fv.file_iter().find(|f| f.as_ref().is_ok_and(|f| f.name() == local_name_guid) || f.is_err()) {
        Some(Ok(result)) => result,
        Some(Err(err)) => return err,
        _ => return efi::Status::NOT_FOUND,
    };

    let section; //ensure that section data lifetime is long enough by assigning to section outside match scope.
    let section_data = match section_type {
        fw_fs::FfsSectionRawType::ALL => file.data(),
        x => {
            let extractor = private_data.section_extractor.as_ref().expect("fv support uninitialized");
            match file
                .section_iter_with_extractor(extractor.as_ref())
                .filter(|sec| sec.as_ref().is_ok_and(|sec| sec.section_type_raw() == x))
                .nth(section_instance)
            {
                Some(Ok(sec)) => {
                    section = sec;
                    section.section_data()
                }
                Some(Err(err)) => return err,
                _ => return efi::Status::NOT_FOUND,
            }
        }
    };

    // get the buffer_size and buffer parameters from caller.
    // Safety: null-checks are at the start of the routine, but caller is required to guarantee that buffer_size and
    // buffer are valid.
    let mut local_buffer_size = unsafe { *buffer_size };
    let mut local_buffer_ptr = unsafe { *buffer };

    if local_buffer_ptr.is_null() {
        //caller indicates that they wish to receive section data, but that this
        //routine should allocate a buffer of appropriate size. Since the caller
        //is expected to free this buffer via free_pool, we need to manually
        //allocate it via allocate_pool.
        match core_allocate_pool(efi::BOOT_SERVICES_DATA, section_data.len()) {
            Err(err) => return err,
            Ok(allocation) => unsafe {
                local_buffer_size = section_data.len();
                local_buffer_ptr = allocation;
                buffer_size.write(local_buffer_size);
                buffer.write(local_buffer_ptr);
            },
        }
    } else {
        // update buffer size output for the caller
        // Safety: null-checked at the start of the routine, but caller is required to guarantee buffer_size is valid.
        unsafe {
            buffer_size.write(section_data.len());
        }
    }

    //copy bytes to output. Caller-provided buffer may be shorter than section
    //data. If so, copy to fill the destination buffer, and return
    //WARN_BUFFER_TOO_SMALL.
    let dest_buffer = unsafe { slice::from_raw_parts_mut(local_buffer_ptr as *mut u8, local_buffer_size) };
    dest_buffer.copy_from_slice(&section_data[0..dest_buffer.len()]);

    //TODO: authentication status not yet supported.

    if dest_buffer.len() < section_data.len() {
        efi::Status::WARN_BUFFER_TOO_SMALL
    } else {
        efi::Status::SUCCESS
    }
}

extern "efiapi" fn fv_write_file(
    _this: *const mu_pi::protocols::firmware_volume::Protocol,
    _number_of_files: u32,
    _write_policy: mu_pi::protocols::firmware_volume::EfiFvWritePolicy,
    _file_data: *mut mu_pi::protocols::firmware_volume::EfiFvWriteFileData,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

extern "efiapi" fn fv_get_next_file(
    this: *const mu_pi::protocols::firmware_volume::Protocol,
    key: *mut c_void,
    file_type: *mut fw_fs::EfiFvFileType,
    name_guid: *mut efi::Guid,
    attributes: *mut fw_fs::EfiFvFileAttributes,
    size: *mut usize,
) -> efi::Status {
    if key.is_null() || file_type.is_null() || name_guid.is_null() || attributes.is_null() || size.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let local_key = unsafe { *(key as *mut usize) };
    let local_file_type = unsafe { *(file_type) };

    if local_file_type >= fw_fs::FfsFileRawType::FFS_MIN {
        return efi::Status::NOT_FOUND;
    }

    let private_data = PRIVATE_FV_DATA.lock();

    let fv_data = match private_data.fv_information.get(&(this as *mut c_void)) {
        Some(PrivateDataItem::FvData(fv_data)) => fv_data,
        Some(_) | None => return efi::Status::NOT_FOUND,
    };

    let fv = match unsafe { fw_fs::FirmwareVolume::new_from_address(fv_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err,
    };

    let fv_attributes = fv.attributes();

    if (fv_attributes & fw_fs::Fvb2RawAttributes::READ_STATUS) == 0 {
        return efi::Status::ACCESS_DENIED;
    }

    let file_candidate = fv
        .file_iter()
        .filter(|f| {
            f.is_err()
                || local_file_type == fw_fs::FfsFileRawType::ALL
                || f.as_ref().is_ok_and(|f| f.file_type_raw() == local_file_type)
        })
        .nth(local_key);

    let file = match file_candidate {
        Some(Err(err)) => return err,
        Some(Ok(file)) => file,
        _ => return efi::Status::NOT_FOUND,
    };

    // found matching file. Update the key and outputs.
    unsafe {
        (key as *mut usize).write(local_key + 1);
        name_guid.write(file.name());
        if (fv_attributes & fw_fs::Fvb2RawAttributes::MEMORY_MAPPED) == fw_fs::Fvb2RawAttributes::MEMORY_MAPPED {
            attributes.write(file.fv_attributes() | fw_fs::FvFileRawAttribute::MEMORY_MAPPED);
        } else {
            attributes.write(file.fv_attributes());
        }
        size.write(file.data().len());
        file_type.write(file.file_type_raw());
    }

    efi::Status::SUCCESS
}

extern "efiapi" fn fv_get_info(
    _this: *const mu_pi::protocols::firmware_volume::Protocol,
    _information_type: *const efi::Guid,
    _buffer_size: *mut usize,
    _buffer: *mut c_void,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

extern "efiapi" fn fv_set_info(
    _this: *const mu_pi::protocols::firmware_volume::Protocol,
    _information_type: *const efi::Guid,
    _buffer_size: usize,
    _buffer: *const c_void,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

fn install_fv_protocol(
    handle: Option<efi::Handle>,
    parent_handle: Option<efi::Handle>,
    base_address: u64,
) -> Result<efi::Handle, efi::Status> {
    let mut fv_interface = Box::from(mu_pi::protocols::firmware_volume::Protocol {
        get_volume_attributes: fv_get_volume_attributes,
        set_volume_attributes: fv_set_volume_attributes,
        read_file: fv_read_file,
        read_section: fv_read_section,
        write_file: fv_write_file,
        get_next_file: fv_get_next_file,
        key_size: size_of::<usize>() as u32,
        parent_handle: match parent_handle {
            Some(handle) => handle,
            None => core::ptr::null_mut(),
        },
        get_info: fv_get_info,
        set_info: fv_set_info,
    });

    let fv_ptr = fv_interface.as_mut() as *mut mu_pi::protocols::firmware_volume::Protocol as *mut c_void;

    let private_data = PrivateFvData { _interface: fv_interface, physical_address: base_address };

    // save the protocol structure we're about to install in the private data.
    PRIVATE_FV_DATA.lock().fv_information.insert(fv_ptr, PrivateDataItem::FvData(private_data));

    // install the protocol and return status
    core_install_protocol_interface(handle, mu_pi::protocols::firmware_volume::PROTOCOL_GUID, fv_ptr)
}

//Firmware Volume device path structures and functions
#[repr(C)]
struct MemMapDevicePath {
    header: efi::protocols::device_path::Protocol,
    memory_type: u32,
    starting_address: u64,
    ending_address: u64,
}

#[repr(C)]
struct FvMemMapDevicePath {
    mem_map_device_path: MemMapDevicePath,
    end_dev_path: efi::protocols::device_path::End,
}

#[repr(C)]
struct MediaFwVolDevicePath {
    header: efi::protocols::device_path::Protocol,
    name: efi::Guid,
}

#[repr(C)]
struct FvPiWgDevicePath {
    fv_dev_path: MediaFwVolDevicePath,
    end_dev_path: efi::protocols::device_path::End,
}

impl FvPiWgDevicePath {
    // instantiate a new FvPiWgDevicePath for a Firmware Volume
    fn new_fv(fv_name: efi::Guid) -> Self {
        Self::new_worker(fv_name, efi::protocols::device_path::Media::SUBTYPE_PIWG_FIRMWARE_VOLUME)
    }
    // instantiate a new FvPiWgDevicePath for a Firmware File
    fn new_file(file_name: efi::Guid) -> Self {
        Self::new_worker(file_name, efi::protocols::device_path::Media::SUBTYPE_PIWG_FIRMWARE_FILE)
    }
    // instantiate a new FvPiWgDevicePath with the given sub-type
    fn new_worker(name: efi::Guid, sub_type: u8) -> Self {
        FvPiWgDevicePath {
            fv_dev_path: MediaFwVolDevicePath {
                header: efi::protocols::device_path::Protocol {
                    r#type: efi::protocols::device_path::TYPE_MEDIA,
                    sub_type,
                    length: [
                        (mem::size_of::<MediaFwVolDevicePath>() & 0xff) as u8,
                        ((mem::size_of::<MediaFwVolDevicePath>() >> 8) & 0xff) as u8,
                    ],
                },
                name,
            },
            end_dev_path: efi::protocols::device_path::End {
                header: efi::protocols::device_path::Protocol {
                    r#type: efi::protocols::device_path::TYPE_END,
                    sub_type: efi::protocols::device_path::End::SUBTYPE_ENTIRE,
                    length: [
                        (mem::size_of::<efi::protocols::device_path::End>() & 0xff) as u8,
                        ((mem::size_of::<efi::protocols::device_path::End>() >> 8) & 0xff) as u8,
                    ],
                },
            },
        }
    }
}

fn install_fv_device_path_protocol(handle: Option<efi::Handle>, base_address: u64) -> Result<efi::Handle, efi::Status> {
    let fv = unsafe { fw_fs::FirmwareVolume::new_from_address(base_address) }?;

    let device_path_ptr = match fv.fv_name() {
        Some(fv_name) => {
            //Construct FvPiWgDevicePath
            let device_path = FvPiWgDevicePath::new_fv(fv_name);
            Box::into_raw(Box::new(device_path)) as *mut c_void
        }
        None => {
            //Construct FvMemMapDevicePath
            let device_path = FvMemMapDevicePath {
                mem_map_device_path: MemMapDevicePath {
                    header: efi::protocols::device_path::Protocol {
                        r#type: efi::protocols::device_path::TYPE_HARDWARE,
                        sub_type: efi::protocols::device_path::Hardware::SUBTYPE_MMAP,
                        length: [
                            (mem::size_of::<MemMapDevicePath>() & 0xff) as u8,
                            ((mem::size_of::<MemMapDevicePath>() >> 8) & 0xff) as u8,
                        ],
                    },
                    memory_type: 11, //EfiMemoryMappedIo not defined in r_efi
                    starting_address: base_address,
                    ending_address: base_address + fv.size(),
                },
                end_dev_path: efi::protocols::device_path::End {
                    header: efi::protocols::device_path::Protocol {
                        r#type: efi::protocols::device_path::TYPE_END,
                        sub_type: efi::protocols::device_path::End::SUBTYPE_ENTIRE,
                        length: [
                            (mem::size_of::<efi::protocols::device_path::End>() & 0xff) as u8,
                            ((mem::size_of::<efi::protocols::device_path::End>() >> 8) & 0xff) as u8,
                        ],
                    },
                },
            };
            Box::into_raw(Box::new(device_path)) as *mut c_void
        }
    };

    // install the protocol and return status
    core_install_protocol_interface(handle, efi::protocols::device_path::PROTOCOL_GUID, device_path_ptr)
}

pub fn core_install_firmware_volume(
    base_address: u64,
    parent_handle: Option<efi::Handle>,
) -> Result<efi::Handle, efi::Status> {
    let handle = install_fv_device_path_protocol(None, base_address)?;
    install_fvb_protocol(Some(handle), parent_handle, base_address)?;
    install_fv_protocol(Some(handle), parent_handle, base_address)?;
    Ok(handle)
}

/// Returns a device path for the file specified by the given fv_handle and filename GUID.
pub fn device_path_bytes_for_fv_file(fv_handle: efi::Handle, file_name: efi::Guid) -> Result<Box<[u8]>, efi::Status> {
    let fv_device_path = PROTOCOL_DB.get_interface_for_handle(fv_handle, efi::protocols::device_path::PROTOCOL_GUID)?;
    let file_node = &FvPiWgDevicePath::new_file(file_name);
    concat_device_path_to_boxed_slice(
        fv_device_path as *mut _ as *const efi::protocols::device_path::Protocol,
        file_node as *const _ as *const efi::protocols::device_path::Protocol,
    )
}

fn initialize_hob_fvs(hob_list: &hob::HobList) -> Result<(), efi::Status> {
    let fv_hobs = hob_list.iter().filter_map(|h| if let hob::Hob::FirmwareVolume(&fv) = h { Some(fv) } else { None });

    for fv in fv_hobs {
        // construct a FirmwareVolume struct to verify sanity.
        let fv_slice = unsafe { slice::from_raw_parts(fv.base_address as *const u8, fv.length as usize) };
        FirmwareVolume::new(fv_slice)?;
        core_install_firmware_volume(fv.base_address, None)?;
    }
    Ok(())
}

/// Initializes FV services for the DXE core.
pub fn init_fv_support(hob_list: &hob::HobList, extractor: Box<dyn SectionExtractor>) {
    PRIVATE_FV_DATA.lock().section_extractor = Some(extractor);
    initialize_hob_fvs(hob_list).expect("Unexpected error initializing FVs from hob_list");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;
    use mu_pi::hob::MEMORY_TYPE_INFO_HOB_GUID;
    use mu_pi::hob::{header, GuidHob, Hob, GUID_EXTENSION};
    extern crate alloc;
    use crate::allocator;
    use crate::GCD;
    const MEM_SIZE: u64 = 0x200000;
    use crate::test_collateral;
    use mu_pi::fw_fs::FfsFileRawType;
    use mu_pi::hob::{get_c_hob_list_size, HobList};
    use r_efi::efi::{Handle, Lba, Status};
    use std::alloc::{alloc, dealloc, Layout};
    use std::ffi::c_void;
    use std::fs;
    use std::io::Write;
    use std::ptr;
    use std::{fs::File, io::Read, vec};
    use uefi_sdk::guid::EVENT_GROUP_END_OF_DXE;
    use uefi_sdk::error::{self, Result};
    use crate::{gcd, test_support::build_test_hob_list};
    // Populate Interfaces which all functions can use.

    //Populate Null References for error cases
    const buffer_size_empty: usize = 0;
    const lba: u64 = 0;
    const offset: usize = 0;
    const section_type: fw_fs::EfiSectionType = 0;
    const section_instance: usize = 0;

    /* FV Init is crashing because of memory protection, debug with team. */
    /*
     /* Returns the length of the HOB list.
      * Clippy gets unhappy if we call get_c_hob_list_size directly, because it gets confused, thinking
      * get_c_hob_list_size is not marked unsafe, but it is
      */
    fn get_hob_list_len1(hob_list: *const c_void) -> usize {
        unsafe { get_c_hob_list_size(hob_list) }
    }

    #[test]
    fn fv_hob_init() {
        test_support::with_global_lock(|| {
            let physical_hob_list = build_test_hob_list(MEM_SIZE);
            if physical_hob_list.is_null() {
               panic!("HOB list pointer is null!");
            }

            unsafe {
                GCD.reset();
                gcd::init_gcd(physical_hob_list);
                test_support::init_test_protocol_db();
            }

            let mut hob_list = HobList::default();
            hob_list.discover_hobs(physical_hob_list);
            hob_list.push(Hob::GuidHob(
                &GuidHob {
                   header: header::Hob { r#type: GUID_EXTENSION, length: 48, reserved: 0 },
                   name: MEMORY_TYPE_INFO_HOB_GUID,
                },
                &[
                      // for test, pick dynamic allocators, since state is easier to clean up for those.
                      0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, //0x0100 pages of LOADER_DATA
                ],
            ));

            // Initialize full allocation support.
            allocator::init_memory_support(&hob_list);
            /* we have to relocate HOBs after memory services are initialized as we are going to allocate memory and
             * the initial free memory may not be enough to contain the HOB list. We need to relocate the HOBs because
             * the initial HOB list is not in mapped memory as passed from pre-DXE.
             */
            hob_list.relocate_hobs();
            let hob_list_slice = unsafe {
                core::slice::from_raw_parts(physical_hob_list as *const u8, get_hob_list_len1(physical_hob_list))
            };
            let relocated_c_hob_list = hob_list_slice.to_vec().into_boxed_slice();

            init_fv_support(&hob_list, Box::new(section_extractor::BrotliSectionExtractor));

        })
        .unwrap();

    } */

    #[test]
    fn evaluate_fv_validation() {
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();

        file.read_to_end(&mut fv).expect("failed to read test file");
        let mut base_address: u64;
        let parent_handle: Option<efi::Handle>;
        let mut fv_att: u64 = 0x0000000000000001;
        let mut fv_attributes: *mut fw_fs::EfiFvAttributes = &mut fv_att;
        let fv_attributes_null: *mut fw_fs::EfiFvAttributes = std::ptr::null_mut();
        let guid_invalid: efi::Guid            = efi::Guid::from_fields(0, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 0]);
        let guidp_invalidp: *const efi::Guid   = &guid_invalid;
        let mut auth_valid_status: u32         = 1;
        let mut auth_valid_p: *mut u32         = &mut auth_valid_status;
        let mut guid_valid_for_file: efi::Guid =
            efi::Guid::from_fields(0x1fa1f39e, 0xfeff, 0x4aae, 0xbd, 0x7b, &[0x38, 0xa0, 0x70, 0xa3, 0xb6, 0x09]);
        let mut guid_valid_for_filep: *mut efi::Guid = &mut guid_valid_for_file;
        let mut file_type_read: fw_fs::EfiFvFileType = 1;
        let mut file_type_readp: *mut fw_fs::EfiFvFileType = &mut file_type_read;
        let mut file_rd_attr: u32 = fw_fs::Fvb2RawAttributes::READ_STATUS;
        let file_attributes: *mut fw_fs::EfiFvFileAttributes = &mut file_rd_attr;
        let mut n_guid_mut: efi::Guid = efi::Guid::from_fields(0, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 0]);
        let mut n_guidp_mut: *mut efi::Guid = &mut n_guid_mut;
        let mut found_type: u8 = FfsFileRawType::DRIVER;
        let found_typep: *mut fw_fs::EfiFvFileType = &mut found_type;
        let mut fvb_attributes: fw_fs::EfiFvbAttributes2 = 0x123456;
        let fvb_attributesp: *mut fw_fs::EfiFvbAttributes2 = &mut fvb_attributes;

        base_address = fv.as_ptr() as u64;
        parent_handle = None;
        let handle = install_fv_device_path_protocol(None, base_address);

        /* Create Firmware Interface, this will be used by the whole test module */
        let mut fv_interface = Box::from(mu_pi::protocols::firmware_volume::Protocol {
            get_volume_attributes: fv_get_volume_attributes,
            set_volume_attributes: fv_set_volume_attributes,
            read_file: fv_read_file,
            read_section: fv_read_section,
            write_file: fv_write_file,
            get_next_file: fv_get_next_file,
            key_size: size_of::<usize>() as u32,
            parent_handle: match parent_handle {
                Some(handle) => handle,
                None => core::ptr::null_mut(),
            },
            get_info: fv_get_info,
            set_info: fv_set_info,
        });

        let fv_ptr = fv_interface.as_mut() as *mut mu_pi::protocols::firmware_volume::Protocol as *mut c_void;
        let fv_ptr1: *const mu_pi::protocols::firmware_volume::Protocol =
            fv_ptr as *const mu_pi::protocols::firmware_volume::Protocol;
        /* Build Privte Data */
        let private_data = PrivateFvData { _interface: fv_interface, physical_address: base_address };
        // save the protocol structure we're about to install in the private data.
        PRIVATE_FV_DATA.lock().fv_information.insert(fv_ptr, PrivateDataItem::FvData(private_data));

        /* Build Firmware Volume Block Interface*/
        let mut fvb_interface = Box::from(mu_pi::protocols::firmware_volume_block::Protocol {
            get_attributes: fvb_get_attributes,
            set_attributes: fvb_set_attributes,
            get_physical_address: fvb_get_physical_address,
            get_block_size: fvb_get_block_size,
            read: fvb_read,
            write: fvb_write,
            erase_blocks: fvb_erase_blocks,
            parent_handle: match parent_handle {
                Some(handle) => handle,
                None => core::ptr::null_mut(),
            },
        });
        let fvb_ptr = fvb_interface.as_mut() as *mut mu_pi::protocols::firmware_volume_block::Protocol as *mut c_void;
        let fvb_ptr_mut_prot = fvb_interface.as_mut() as *mut mu_pi::protocols::firmware_volume_block::Protocol;

        /* Build Privte Data */
        let private_data = PrivateFvbData { _interface: fvb_interface, physical_address: base_address };
        // save the protocol structure we're about to install in the private data.
        PRIVATE_FV_DATA.lock().fv_information.insert(fvb_ptr, PrivateDataItem::FvbData(private_data));

        let fv_attributes3: *mut fw_fs::EfiFvAttributes = &mut fv_att;

        /* Instance 2 - Create a FV  interface with Bad physical address to handle Error cases. */
        let mut fv_interface3 = Box::from(mu_pi::protocols::firmware_volume::Protocol {
            get_volume_attributes: fv_get_volume_attributes,
            set_volume_attributes: fv_set_volume_attributes,
            read_file: fv_read_file,
            read_section: fv_read_section,
            write_file: fv_write_file,
            get_next_file: fv_get_next_file,
            key_size: size_of::<usize>() as u32,
            parent_handle: match parent_handle {
                Some(handle) => handle,
                None => core::ptr::null_mut(),
            },
            get_info: fv_get_info,
            set_info: fv_set_info,
        });

        let fv_ptr3 = fv_interface3.as_mut() as *mut mu_pi::protocols::firmware_volume::Protocol as *mut c_void;
        let fv_ptr3_const: *const mu_pi::protocols::firmware_volume::Protocol =
            fv_ptr3 as *const mu_pi::protocols::firmware_volume::Protocol;

        /* Corrupt the base address to cover error conditions  */
        let base_no2: u64 = (fv.as_ptr() as u64 + 0x1000);
        let private_data2 = PrivateFvData { _interface: fv_interface3, physical_address: base_no2 };
        //save the protocol structure we're about to install in the private data.
        PRIVATE_FV_DATA.lock().fv_information.insert(fv_ptr3, PrivateDataItem::FvData(private_data2));

        /* Create an interface with No physical address and no private data - cover Error Conditions */
        let fv_interface_no_data = mu_pi::protocols::firmware_volume::Protocol {
            get_volume_attributes: fv_get_volume_attributes,
            set_volume_attributes: fv_set_volume_attributes,
            read_file: fv_read_file,
            read_section: fv_read_section,
            write_file: fv_write_file,
            get_next_file: fv_get_next_file,
            key_size: size_of::<usize>() as u32,
            parent_handle: core::ptr::null_mut(),

            get_info: fv_get_info,
            set_info: fv_set_info,
        };

        let fv_ptr_no_data = &fv_interface_no_data as *const mu_pi::protocols::firmware_volume::Protocol;

        /* Create a Firmware Volume Block Interface with Invalid Physical Address */
        let mut fvb_intf_invalid = Box::from(mu_pi::protocols::firmware_volume_block::Protocol {
            get_attributes: fvb_get_attributes,
            set_attributes: fvb_set_attributes,
            get_physical_address: fvb_get_physical_address,
            get_block_size: fvb_get_block_size,
            read: fvb_read,
            write: fvb_write,
            erase_blocks: fvb_erase_blocks,
            parent_handle: match parent_handle {
                Some(handle) => handle,
                None => core::ptr::null_mut(),
            },
        });
        let fvb_intf_invalid_cvoid =
            fvb_intf_invalid.as_mut() as *mut mu_pi::protocols::firmware_volume_block::Protocol as *mut c_void;
        let fvb_intf_invalid_mutpro =
            fvb_intf_invalid.as_mut() as *mut mu_pi::protocols::firmware_volume_block::Protocol;
        let base_no: u64 = (fv.as_ptr() as u64 + 0x1000);

        let private_data4 = PrivateFvbData { _interface: fvb_intf_invalid, physical_address: base_no };
        // save the protocol structure we're about to install in the private data.
        PRIVATE_FV_DATA.lock().fv_information.insert(fvb_intf_invalid_cvoid, PrivateDataItem::FvbData(private_data4));

        /* Create a Firmware Volume Block Interface without Physical address populated  */
        let mut fvb_intf_ndata = Box::from(mu_pi::protocols::firmware_volume_block::Protocol {
            get_attributes: fvb_get_attributes,
            set_attributes: fvb_set_attributes,
            get_physical_address: fvb_get_physical_address,
            get_block_size: fvb_get_block_size,
            read: fvb_read,
            write: fvb_write,
            erase_blocks: fvb_erase_blocks,
            parent_handle: match parent_handle {
                Some(handle) => handle,
                None => core::ptr::null_mut(),
            },
        });
        let fvb_intf_ndata_cvoid =
            fvb_intf_ndata.as_mut() as *mut mu_pi::protocols::firmware_volume_block::Protocol as *mut c_void;
        let fvb_intf_ndata_mut = fvb_intf_ndata.as_mut() as *mut mu_pi::protocols::firmware_volume_block::Protocol;

        unsafe {

            let fv_test_set_info = || {
                fv_set_info(ptr::null(), ptr::null(), buffer_size_empty, ptr::null());
            };

            let fv_test_get_info = || {
                fv_get_info(ptr::null(), ptr::null(), ptr::null_mut(), ptr::null_mut());
            };

            let fv_test_set_volume_attributes = || {
                /* Cover the NULL Case */
                fv_set_volume_attributes(ptr::null(), fv_attributes);

                /* Non Null Case*/
            };

            let fv_test_get_volume_attributes = || {
                /* Cover the NULL Case, User Passing Invalid Parameter Case  */
                fv_get_volume_attributes(fv_ptr1, fv_attributes_null);

                /* Handle bad firmware volume data - return efi::Status::NOT_FOUND */
                fv_get_volume_attributes(fv_ptr_no_data, fv_attributes);

                /* Handle Invalid Physical address case */
                fv_get_volume_attributes(fv_ptr3_const, fv_attributes);

                /* Non Null Case, success case */
                fv_get_volume_attributes(fv_ptr1, fv_attributes3);

            };

            let fv_test_fvb_read = || {
                /* Mutable Reference cannot be borrowed more than once,
                 * hence delcare and free up after use immediately
                 */
                 let mut len3                       = 1000;
                 let buffer_valid_size3: *mut usize = &mut len3;
                 let layout3                        = Layout::from_size_align(1001, 8).unwrap();
                 let mut buffer_valid3              = alloc(layout3) as *mut c_void;

                if buffer_valid3.is_null() {
                    panic!("Memory allocation failed!");
                }

                fvb_read(fvb_ptr_mut_prot, lba, 0, std::ptr::null_mut(), std::ptr::null_mut());
                fvb_read(fvb_ptr_mut_prot, lba, 0, buffer_valid_size3, buffer_valid3 as *mut c_void);
                fvb_read(fvb_ptr_mut_prot, 0xfffffffff, 0, buffer_valid_size3, buffer_valid3 as *mut c_void);
                fvb_read(fvb_intf_invalid_mutpro, lba, 0, buffer_valid_size3, buffer_valid3 as *mut c_void);
                fvb_read(fvb_ptr_mut_prot, u64::MAX, 0, buffer_valid_size3, buffer_valid3 as *mut c_void);
                fvb_read(fvb_intf_ndata_mut, lba, 0, buffer_valid_size3, buffer_valid3 as *mut c_void);

                /* Free Memory */
                dealloc(buffer_valid3 as *mut u8, layout3);
            };

            let fv_test_get_block_size = || {
                /* Mutable Reference cannot be borrowed more than once,
                 * hence delcare and free up after use immediately
                 */
                 let mut len3                       = 1000;
                 let buffer_valid_size3: *mut usize = &mut len3;
                 let layout3                        = Layout::from_size_align(1001, 8).unwrap();
                 let mut buffer_valid3              = alloc(layout3) as *mut c_void;

                if buffer_valid3.is_null() {
                    panic!("Memory allocation failed!");
                }

                /* Handle the Null Case */
                fvb_get_block_size(fvb_ptr_mut_prot, lba, std::ptr::null_mut(), std::ptr::null_mut());
                fvb_get_block_size(fvb_ptr_mut_prot, lba, buffer_valid_size3, buffer_valid_size3);
                fvb_get_block_size(fvb_intf_invalid_mutpro, lba, buffer_valid_size3, buffer_valid_size3);
                fvb_get_block_size(fvb_intf_ndata_mut, lba, buffer_valid_size3, buffer_valid_size3);
                //fvb_get_block_size(fvb_intf_ndata_mut, lba, buffer_valid_size3, buffer_valid_size3);
                //fvb_get_block_size(fvb_ptr_mut_prot, 0xfffffffff, buffer_valid_size3, buffer_valid_size3);
                fvb_get_block_size(fvb_ptr_mut_prot, u64::MAX, buffer_valid_size3, buffer_valid_size3);
                /* Free Memory */
                dealloc(buffer_valid3 as *mut u8, layout3);
            };


            let fvb_test_erase_block = || {
                fvb_erase_blocks(fvb_ptr_mut_prot);
            };

            let fvb_test_get_physical_address = || {
                /* Handling Not Found Case */
                let mut p_address: efi::PhysicalAddress = 0x12345;

                fvb_get_physical_address(fvb_intf_ndata_mut, &mut p_address as *mut u64);
                fvb_get_physical_address(fvb_intf_invalid_mutpro, &mut p_address as *mut u64);
                fvb_get_physical_address(fvb_ptr_mut_prot, &mut p_address as *mut u64);
                fvb_get_physical_address(fvb_ptr_mut_prot, std::ptr::null_mut());
            };
            let fvb_test_write_file = || {
 
                let number_of_files: u32 = 0;
                let write_policy: mu_pi::protocols::firmware_volume::EfiFvWritePolicy = 0;
                fv_write_file(fv_ptr1, number_of_files, write_policy, std::ptr::null_mut());
            };

            let fvb_test_set_attributes = || {
                fvb_set_attributes(fvb_ptr_mut_prot, std::ptr::null_mut());
            };

            let fvb_test_write = || {
                let mut len3 = 1000;
                let buffer_valid_size3: *mut usize = &mut len3;
                let layout3 = Layout::from_size_align(1001, 8).unwrap();
                let mut buffer_valid3 = alloc(layout3) as *mut c_void;

                if buffer_valid3.is_null() {
                    panic!("Memory allocation failed!");
                }

                fvb_write(fvb_ptr_mut_prot, lba, 0, std::ptr::null_mut(), std::ptr::null_mut());
                fvb_write(fvb_ptr_mut_prot, lba, 0, buffer_valid_size3, buffer_valid3 as *mut c_void);
                fvb_write(fvb_intf_invalid_mutpro, lba, 0, buffer_valid_size3, buffer_valid3 as *mut c_void);
                fvb_write(fvb_intf_ndata_mut, lba, 0, buffer_valid_size3, buffer_valid3 as *mut c_void);
                /* Free Memory */
                dealloc(buffer_valid3 as *mut u8, layout3);
            };

            let fvb_test_get_attributes = || {
                fvb_get_attributes(fvb_ptr_mut_prot, std::ptr::null_mut());
                fvb_get_attributes(fvb_ptr_mut_prot, fvb_attributesp);
                fvb_get_attributes(fvb_intf_invalid_mutpro, fvb_attributesp);
                fvb_get_attributes(fvb_intf_ndata_mut, fvb_attributesp);
            };


            let mut fvb_test_get_next_file = || {
                /* Mutable Reference cannot be borrowed more than once,
                 * hence delcare and free up after use immediately
                 */
                let mut len3                       = 1000;
                let buffer_valid_size3: *mut usize = &mut len3;
                let layout3                        = Layout::from_size_align(1001, 8).unwrap();
                let mut buffer_valid3              = alloc(layout3) as *mut c_void;

                if buffer_valid3.is_null() {
                    panic!("Memory allocation failed!");
                }
                fv_get_next_file(
                    ptr::null(),
                    std::ptr::null_mut(),
                    file_type_readp,
                    std::ptr::null_mut(),
                    file_attributes,
                    buffer_valid_size3,
                );
                fv_get_next_file(
                    ptr::null(),
                    buffer_valid3 as *mut c_void,
                    file_type_readp,
                    n_guidp_mut,
                    file_attributes,
                    buffer_valid_size3,
                );
                fv_get_next_file(
                    fv_ptr1,
                    buffer_valid3 as *mut c_void,
                    file_type_readp,
                    n_guidp_mut,
                    file_attributes,
                    buffer_valid_size3,
                );
                fv_get_next_file(
                    fv_ptr3_const,
                    buffer_valid3 as *mut c_void,
                    file_type_readp,
                    n_guidp_mut,
                    file_attributes,
                    buffer_valid_size3,
                );
                fv_get_next_file(
                    fv_ptr_no_data,
                    buffer_valid3 as *mut c_void,
                    file_type_readp,
                    n_guidp_mut,
                    file_attributes,
                    buffer_valid_size3,
                );
                /*handle  fw_fs::FfsFileRawType::FFS_MIN case */
                file_type_read = fw_fs::FfsFileRawType::FFS_MIN;

                fv_get_next_file(
                    fv_ptr1,
                    buffer_valid3 as *mut c_void,
                    file_type_readp,
                    n_guidp_mut,
                    file_attributes,
                    buffer_valid_size3,
                );
                /* Null BUffer Case*/
                fv_get_next_file(
                    fv_ptr1,
                    std::ptr::null_mut(),
                    file_type_readp,
                    n_guidp_mut,
                    file_attributes,
                    buffer_valid_size3,
                );
                // Deallocate the memory
                dealloc(buffer_valid3 as *mut u8, layout3);
            };

            let fvb_test_read_section = || {
                /* Mutable Reference cannot be borrowed more than once,
                 * hence delcare and free up after use immediately
                 */
                let mut len3                       = 1000;
                let buffer_valid_size3: *mut usize = &mut len3;
                let layout3                        = Layout::from_size_align(1001, 8).unwrap();
                let mut buffer_valid3              = alloc(layout3) as *mut c_void;

                if buffer_valid3.is_null() {
                    panic!("Memory allocation failed!");
                }

                let mut gd1: efi::Guid = mu_pi::protocols::firmware_volume_block::PROTOCOL_GUID; //EVENT_GROUP_END_OF_DXE;
                let mut gd2: efi::Guid = efi::Guid::from_fields(
                    0x434f695c,
                    0xef26,
                    0x4a12,
                    0x9e,
                    0xba,
                    &[0xdd, 0xef, 0x00, 0x97, 0x49, 0x7c],
                );
                let name_guid1: *mut efi::Guid = &mut gd1;
                let name_guid2: *mut efi::Guid = &mut gd2;

                /* Cover the NULL Case, User Passing Invalid Parameter Case  */
                fv_read_section(
                    ptr::null(),
                    ptr::null(),
                    section_type,
                    section_instance,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );

                fv_read_section(
                    fv_ptr1,
                    guidp_invalidp,
                    6,
                    10,
                    (&mut buffer_valid3 as *mut *mut c_void),
                    buffer_valid_size3,
                    auth_valid_p,
                );
                /* Valid guid case - panicing, debug this further, for now comment*/
                /*fv_read_section(
                    fv_ptr1,
                    guid_valid_for_filep,
                    1,
                    1,
                   (&mut buffer_valid as *mut *mut c_void),
                   buffer_valid_size,
                   auth_valid_p,
                  ); */

                fv_read_section(
                    fv_ptr1,
                    name_guid2,
                    6,
                    10,
                    (&mut buffer_valid3 as *mut *mut c_void),
                    buffer_valid_size3,
                    auth_valid_p,
                );

                /* Handle Invalid Physical address case */
                fv_read_section(
                    fv_ptr3_const,
                    guidp_invalidp,
                    1,
                    1,
                    (&mut buffer_valid3 as *mut *mut c_void),
                    buffer_valid_size3,
                    auth_valid_p,
                );

                /* Handle bad firmware volume data - return efi::Status::NOT_FOUND */
                fv_read_section(
                    fv_ptr_no_data,
                    guidp_invalidp,
                    1,
                    1,
                    (&mut buffer_valid3 as *mut *mut c_void),
                    buffer_valid_size3,
                    auth_valid_p,
                );
                /* Free Memory */
                dealloc(buffer_valid3 as *mut u8, layout3);
            };

            let fvb_test_read_file = || {
                /* Mutable Reference cannot be borrowed more than once,
                 * hence delcare and free up after use immediately
                 */
                 let mut len3                       = 1000;
                 let buffer_valid_size3: *mut usize = &mut len3;
                 let layout3                        = Layout::from_size_align(1001, 8).unwrap();
                 let mut buffer_valid3              = alloc(layout3) as *mut c_void;

                if buffer_valid3.is_null() {
                    panic!("Memory allocation failed!");
                }

                fv_read_file(
                    ptr::null(),
                    ptr::null(),
                    (&mut buffer_valid3 as *mut *mut c_void),
                    std::ptr::null_mut(),
                    found_typep,
                    file_attributes,
                    std::ptr::null_mut(),
                );

                fv_read_file(
                    fv_ptr1,
                    guidp_invalidp,
                    (&mut buffer_valid3 as *mut *mut c_void),
                    buffer_valid_size3,
                    found_typep,
                    file_attributes,
                    auth_valid_p,
                );
                fv_read_file(
                    fv_ptr1,
                    guid_valid_for_filep,
                    (&mut buffer_valid3 as *mut *mut c_void),
                    buffer_valid_size3,
                    found_typep,
                    file_attributes,
                    auth_valid_p,
                );
                fv_read_file(
                    fv_ptr3_const,
                    guid_valid_for_filep,
                    (&mut buffer_valid3 as *mut *mut c_void),
                    buffer_valid_size3,
                    found_typep,
                    file_attributes,
                    auth_valid_p,
                );
                fv_read_file(
                    fv_ptr_no_data,
                    guid_valid_for_filep,
                    (&mut buffer_valid3 as *mut *mut c_void),
                    buffer_valid_size3,
                    found_typep,
                    file_attributes,
                    auth_valid_p,
                );
                fv_read_file(
                    fv_ptr1,
                    guid_valid_for_filep,
                    std::ptr::null_mut(),
                    buffer_valid_size3,
                    found_typep,
                    file_attributes,
                    auth_valid_p,
                );
                /* Raise Bug for this case , case when Buffer size is 0 and buffer not NULL. last block*/
                /*fv_read_file(fv_ptr1 , guid_valid_for_filep, (&mut buffer_valid as *mut *mut c_void), 
                  buffer_equal_0p, found_typep, file_attributes,
                  auth_valid_p ); */
                /* Free Memory */
                dealloc(buffer_valid3 as *mut u8, layout3);
            };

            fv_test_set_info();
            fv_test_get_info();
            fv_test_set_volume_attributes();
            fv_test_get_volume_attributes();
            fv_test_fvb_read();
            fv_test_get_block_size();
            fvb_test_erase_block();
            fvb_test_get_physical_address();
            fvb_test_set_attributes();
            fvb_test_get_attributes();
            fvb_test_write();
            fvb_test_read_section();
            fvb_test_get_next_file();
            fvb_test_read_file();
            fvb_test_write_file();
        }
    }
}
