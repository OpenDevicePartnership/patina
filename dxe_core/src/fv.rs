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

use crate::{allocator::core_allocate_pool, protocols::core_install_protocol_interface};

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
        buffer_size.write(file.data().len());
    }

    if buffer.is_null() {
        //caller just wants file meta data, no need to read file data.
        return efi::Status::SUCCESS;
    }

    let mut local_buffer_ptr = unsafe { *buffer };

    if local_buffer_size > 0 {
        //caller indicates they have allocated a buffer to receive the file data.
        if local_buffer_size < file.data().len() {
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
        match core_allocate_pool(efi::BOOT_SERVICES_DATA, file.data().len()) {
            Err(err) => return err,
            Ok(allocation) => unsafe {
                local_buffer_ptr = allocation;
                buffer.write(local_buffer_ptr);
            },
        }
    }

    //convert pointer+size into a slice and copy the file data.
    let out_buffer = unsafe { slice::from_raw_parts_mut(local_buffer_ptr as *mut u8, file.data().len()) };
    out_buffer.copy_from_slice(file.data());

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
    fv_name: efi::Guid,
}

#[repr(C)]
struct FvPiWgDevicePath {
    fv_dev_path: MediaFwVolDevicePath,
    end_dev_path: efi::protocols::device_path::End,
}

impl FvPiWgDevicePath {
    fn new(fv_name: efi::Guid) -> Self {
        FvPiWgDevicePath {
            fv_dev_path: MediaFwVolDevicePath {
                header: efi::protocols::device_path::Protocol {
                    r#type: efi::protocols::device_path::TYPE_MEDIA,
                    sub_type: efi::protocols::device_path::Media::SUBTYPE_PIWG_FIRMWARE_VOLUME,
                    length: [
                        (mem::size_of::<MediaFwVolDevicePath>() & 0xff) as u8,
                        ((mem::size_of::<MediaFwVolDevicePath>() >> 8) & 0xff) as u8,
                    ],
                },
                fv_name,
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
            let device_path = FvPiWgDevicePath::new(fv_name);
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
