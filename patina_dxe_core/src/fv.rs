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
    fw_fs::{ffs, fv, fvb},
    hob,
};

use patina_ffs::{section::SectionExtractor, volume::VolumeRef};
use patina_internal_device_path::concat_device_path_to_boxed_slice;
use patina_sdk::error::EfiError;
use r_efi::efi;

use crate::{
    allocator::core_allocate_pool,
    protocols::{PROTOCOL_DB, core_install_protocol_interface},
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
    attributes: *mut fvb::attributes::EfiFvbAttributes2,
) -> efi::Status {
    if attributes.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let private_data = PRIVATE_FV_DATA.lock();

    let fvb_data = match private_data.fv_information.get(&(this as *mut c_void)) {
        Some(PrivateDataItem::FvbData(fvb_data)) => fvb_data,
        Some(_) | None => return efi::Status::NOT_FOUND,
    };

    let fv = match unsafe { VolumeRef::new_from_address(fvb_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err.into(),
    };

    unsafe { attributes.write(fv.attributes()) };

    efi::Status::SUCCESS
}

extern "efiapi" fn fvb_set_attributes(
    _this: *mut mu_pi::protocols::firmware_volume_block::Protocol,
    _attributes: *mut fvb::attributes::EfiFvbAttributes2,
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

    let fv = match unsafe { VolumeRef::new_from_address(fvb_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err.into(),
    };

    let lba: u32 = match lba.try_into() {
        Ok(lba) => lba,
        _ => return efi::Status::INVALID_PARAMETER,
    };

    let (size, remaining_blocks) = match fv.lba_info(lba) {
        Err(err) => return err.into(),
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

    let fv = match unsafe { VolumeRef::new_from_address(fvb_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err.into(),
    };

    let lba: u32 = match lba.try_into() {
        Ok(lba) => lba,
        _ => return efi::Status::INVALID_PARAMETER,
    };

    let (lba_base_addr, block_size) = match fv.lba_info(lba) {
        Err(err) => return err.into(),
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
) -> Result<efi::Handle, EfiError> {
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
    fv_attributes: *mut fv::attributes::EfiFvAttributes,
) -> efi::Status {
    if fv_attributes.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let private_data = PRIVATE_FV_DATA.lock();

    let fv_data = match private_data.fv_information.get(&(this as *mut c_void)) {
        Some(PrivateDataItem::FvData(fv_data)) => fv_data,
        Some(_) | None => return efi::Status::NOT_FOUND,
    };

    let fv = match unsafe { VolumeRef::new_from_address(fv_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err.into(),
    };

    unsafe { fv_attributes.write(fv.attributes() as fv::attributes::EfiFvAttributes) };

    efi::Status::SUCCESS
}

extern "efiapi" fn fv_set_volume_attributes(
    _this: *const mu_pi::protocols::firmware_volume::Protocol,
    _fv_attributes: *mut fv::attributes::EfiFvAttributes,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

extern "efiapi" fn fv_read_file(
    this: *const mu_pi::protocols::firmware_volume::Protocol,
    name_guid: *const efi::Guid,
    buffer: *mut *mut c_void,
    buffer_size: *mut usize,
    found_type: *mut fv::EfiFvFileType,
    file_attributes: *mut fv::file::EfiFvFileAttributes,
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

    let fv = match unsafe { VolumeRef::new_from_address(fv_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err.into(),
    };

    if (fv.attributes() & fvb::attributes::raw::fvb2::READ_STATUS) == 0 {
        return efi::Status::ACCESS_DENIED;
    }

    let file = match fv.files().find(|f| f.as_ref().is_ok_and(|f| f.name() == local_name_guid) || f.is_err()) {
        Some(Ok(result)) => result,
        Some(Err(err)) => return err.into(),
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
            Err(err) => return err.into(),
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
    section_type: ffs::section::EfiSectionType,
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

    let fv = match unsafe { VolumeRef::new_from_address(fv_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err.into(),
    };

    if (fv.attributes() & fvb::attributes::raw::fvb2::READ_STATUS) == 0 {
        return efi::Status::ACCESS_DENIED;
    }

    let file = match fv.files().find(|f| f.as_ref().is_ok_and(|f| f.name() == local_name_guid) || f.is_err()) {
        Some(Ok(result)) => result,
        Some(Err(err)) => return err.into(),
        _ => return efi::Status::NOT_FOUND,
    };

    let sections; //ensure that section data lifetime is long enough by assigning to section outside match scope.
    let section_data = match section_type {
        ffs::section::raw_type::ALL => file.data(),
        x => {
            let extractor = private_data.section_extractor.as_ref().expect("fv support uninitialized");
            sections = match file.sections_with_extractor(extractor.as_ref()) {
                Ok(sections) => sections,
                Err(err) => return err.into(),
            };

            match sections.iter().filter(|sec| sec.section_type_raw() == x).nth(section_instance) {
                Some(sec) => match sec.try_content_as_slice() {
                    Ok(data) => data,
                    Err(err) => return err.into(),
                },
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
            Err(err) => return err.into(),
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

    if dest_buffer.len() < section_data.len() { efi::Status::WARN_BUFFER_TOO_SMALL } else { efi::Status::SUCCESS }
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
    file_type: *mut fv::EfiFvFileType,
    name_guid: *mut efi::Guid,
    attributes: *mut fv::file::EfiFvFileAttributes,
    size: *mut usize,
) -> efi::Status {
    if key.is_null() || file_type.is_null() || name_guid.is_null() || attributes.is_null() || size.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let local_key = unsafe { *(key as *mut usize) };
    let local_file_type = unsafe { *(file_type) };

    if local_file_type >= ffs::file::raw::r#type::FFS_MIN {
        return efi::Status::NOT_FOUND;
    }

    let private_data = PRIVATE_FV_DATA.lock();

    let fv_data = match private_data.fv_information.get(&(this as *mut c_void)) {
        Some(PrivateDataItem::FvData(fv_data)) => fv_data,
        Some(_) | None => return efi::Status::NOT_FOUND,
    };

    let fv = match unsafe { VolumeRef::new_from_address(fv_data.physical_address) } {
        Ok(fv) => fv,
        Err(err) => return err.into(),
    };

    let fv_attributes = fv.attributes();

    if (fv_attributes & fvb::attributes::raw::fvb2::READ_STATUS) == 0 {
        return efi::Status::ACCESS_DENIED;
    }

    let file_candidate = fv
        .files()
        .filter(|f| {
            f.is_err()
                || local_file_type == ffs::file::raw::r#type::ALL
                || f.as_ref().is_ok_and(|f| f.file_type_raw() == local_file_type)
        })
        .nth(local_key);

    let file = match file_candidate {
        Some(Err(err)) => return err.into(),
        Some(Ok(file)) => file,
        _ => return efi::Status::NOT_FOUND,
    };

    // found matching file. Update the key and outputs.
    unsafe {
        (key as *mut usize).write(local_key + 1);
        name_guid.write(file.name());
        if (fv_attributes & fvb::attributes::raw::fvb2::MEMORY_MAPPED) == fvb::attributes::raw::fvb2::MEMORY_MAPPED {
            attributes.write(file.fv_attributes() | fv::file::raw::attribute::MEMORY_MAPPED);
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
) -> Result<efi::Handle, EfiError> {
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

// Safety: caller must ensure that base_address points to a valid firmware volume.
unsafe fn install_fv_device_path_protocol(
    handle: Option<efi::Handle>,
    base_address: u64,
) -> Result<efi::Handle, EfiError> {
    let fv = unsafe { VolumeRef::new_from_address(base_address) }?;

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

pub unsafe fn core_install_firmware_volume(
    base_address: u64,
    parent_handle: Option<efi::Handle>,
) -> Result<efi::Handle, EfiError> {
    let handle = unsafe { install_fv_device_path_protocol(None, base_address)? };
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
    let fv_hobs = hob_list.iter().filter_map(|h| if let hob::Hob::FirmwareVolume(fv) = h { Some(*fv) } else { None });

    for fv in fv_hobs {
        // construct a FirmwareVolume struct to verify sanity.
        let fv_slice = unsafe { slice::from_raw_parts(fv.base_address as *const u8, fv.length as usize) };
        VolumeRef::new(fv_slice)?;
        // Safety: base addresses of FirmwareVolume HOBs are assumed to be valid and accessible.
        unsafe { core_install_firmware_volume(fv.base_address, None) }?;
    }
    Ok(())
}

/// Initializes FV services for the DXE core.
pub fn init_fv_support(hob_list: &hob::HobList, extractor: Box<dyn SectionExtractor>) {
    PRIVATE_FV_DATA.lock().section_extractor = Some(extractor);
    initialize_hob_fvs(hob_list).expect("Unexpected error initializing FVs from hob_list");
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::test_support;
    use mu_pi::hob::Hob;
    extern crate alloc;
    use crate::test_collateral;
    use mu_pi::hob::HobList;
    use mu_pi::{BootMode, hob};
    use std::alloc::{Layout, alloc, dealloc};
    use std::ffi::c_void;
    use std::ptr;
    use std::{fs::File, io::Read};

    //Populate Null References for error cases
    const BUFFER_SIZE_EMPTY: usize = 0;
    const LBA: u64 = 0;
    const SECTION_TYPE: ffs::section::EfiSectionType = 0;
    const SECTION_INSTANCE: usize = 0;

    pub unsafe fn fv_private_data_reset() {
        // Clear inserted elements
        PRIVATE_FV_DATA.lock().fv_information.clear();
    }

    #[test]
    fn test_fv_init_core() {
        test_support::with_global_lock(|| {
            /* Start with Clearing Private Global Data, Please note that this is to be done only once
             * for test_fv_functionality.
             * In case other functions/modules are written, clear the private global data again.
             */
            unsafe {
                fv_private_data_reset();
            }
            assert!(PRIVATE_FV_DATA.lock().fv_information.is_empty());
            fn gen_firmware_volume2() -> hob::FirmwareVolume2 {
                let header =
                    hob::header::Hob { r#type: hob::FV, length: size_of::<hob::FirmwareVolume2>() as u16, reserved: 0 };

                hob::FirmwareVolume2 {
                    header,
                    base_address: 0,
                    length: 0x8000,
                    fv_name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
                    file_name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
                }
            }
            fn gen_firmware_volume() -> hob::FirmwareVolume {
                let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
                let mut fv: Vec<u8> = Vec::new();
                file.read_to_end(&mut fv).expect("failed to read test file");
                let len: u64 = fv.len() as u64;
                let base: u64 = fv.as_ptr() as u64;

                let header =
                    hob::header::Hob { r#type: hob::FV, length: size_of::<hob::FirmwareVolume>() as u16, reserved: 0 };

                hob::FirmwareVolume { header, base_address: base, length: len }
            }

            fn gen_end_of_hoblist() -> hob::PhaseHandoffInformationTable {
                let header = hob::header::Hob {
                    r#type: hob::END_OF_HOB_LIST,
                    length: size_of::<hob::PhaseHandoffInformationTable>() as u16,
                    reserved: 0,
                };

                hob::PhaseHandoffInformationTable {
                    header,
                    version: 0x00010000,
                    boot_mode: BootMode::BootWithFullConfiguration,
                    memory_top: 0xdeadbeef,
                    memory_bottom: 0xdeadc0de,
                    free_memory_top: 104,
                    free_memory_bottom: 255,
                    end_of_hob_list: 0xdeaddeadc0dec0de,
                }
            }

            // Generate some example HOBs

            let _firmware_volume2 = gen_firmware_volume2();
            let _firmware_volume0 = gen_firmware_volume();
            let end_of_hob_list = gen_end_of_hoblist();

            // Create a new empty HOB list
            let mut hoblist = HobList::new();

            // Push the example HOBs onto the HOB l
            hoblist.push(Hob::FirmwareVolume2(&_firmware_volume2));
            hoblist.push(Hob::Handoff(&end_of_hob_list));
            init_fv_support(&hoblist, Box::new(patina_ffs_extractors::BrotliSectionExtractor));
        })
        .expect("Unexpected Error Initalising hob fvs ");
    }

    #[test]
    fn test_fv_functionality() {
        test_support::with_global_lock(|| {
            let mut fv_att: u64 = 0x1;
            let fv_attributes: *mut fv::attributes::EfiFvAttributes = &mut fv_att;
            let guid_invalid: efi::Guid = efi::Guid::from_fields(0, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 0]);
            let guid_ref_invalid_ref: *const efi::Guid = &guid_invalid;
            let mut auth_valid_status: u32 = 1;
            let auth_valid_p: *mut u32 = &mut auth_valid_status;
            let mut guid_valid: efi::Guid =
                efi::Guid::from_fields(0x1fa1f39e, 0xfeff, 0x4aae, 0xbd, 0x7b, &[0x38, 0xa0, 0x70, 0xa3, 0xb6, 0x09]);
            let guid_valid_ref: *mut efi::Guid = &mut guid_valid;
            let mut file_rd_attr: u32 = fvb::attributes::raw::fvb2::READ_STATUS;
            let file_attributes: *mut fv::file::EfiFvFileAttributes = &mut file_rd_attr;

            let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
            let mut fv: Vec<u8> = Vec::new();
            file.read_to_end(&mut fv).expect("failed to read test file");

            let fv = fv.leak();
            let base_address: u64 = fv.as_ptr() as u64;
            let parent_handle: Option<efi::Handle> = None;

            // Safety: fv was leaked above to ensure that the buffer is valid and immutable for the rest of the test.
            let _handle = unsafe { install_fv_device_path_protocol(None, base_address) };

            /* Start with Clearing Private Global Data, Please note that this is to be done only once
             * for test_fv_functionality.
             * In case other functions/modules are written, clear the private global data again.
             */
            unsafe {
                fv_private_data_reset();
            }
            assert!(PRIVATE_FV_DATA.lock().fv_information.is_empty());

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
                    Some(_handle) => _handle,
                    None => core::ptr::null_mut(),
                },
                get_info: fv_get_info,
                set_info: fv_set_info,
            });

            let fv_ptr = fv_interface.as_mut() as *mut mu_pi::protocols::firmware_volume::Protocol as *mut c_void;

            let private_data = PrivateFvData { _interface: fv_interface, physical_address: base_address };
            // save the protocol structure we're about to install in the private data.
            PRIVATE_FV_DATA.lock().fv_information.insert(fv_ptr, PrivateDataItem::FvData(private_data));
            let fv_ptr1: *const mu_pi::protocols::firmware_volume::Protocol =
                fv_ptr as *const mu_pi::protocols::firmware_volume::Protocol;

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
            let fvb_ptr =
                fvb_interface.as_mut() as *mut mu_pi::protocols::firmware_volume_block::Protocol as *mut c_void;
            let fvb_ptr_mut_prot = fvb_interface.as_mut() as *mut mu_pi::protocols::firmware_volume_block::Protocol;

            /* Build Private Data */
            let private_data = PrivateFvbData { _interface: fvb_interface, physical_address: base_address };
            // save the protocol structure we're about to install in the private data.
            PRIVATE_FV_DATA.lock().fv_information.insert(fvb_ptr, PrivateDataItem::FvbData(private_data));

            //let fv_attributes3: *mut fw_fs::EfiFvAttributes = &mut fv_att;

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
            let base_no2: u64 = fv.as_ptr() as u64 + 0x1000;
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
            let fvb_intf_invalid_void =
                fvb_intf_invalid.as_mut() as *mut mu_pi::protocols::firmware_volume_block::Protocol as *mut c_void;
            let fvb_intf_invalid_mutpro =
                fvb_intf_invalid.as_mut() as *mut mu_pi::protocols::firmware_volume_block::Protocol;
            let base_no: u64 = fv.as_ptr() as u64 + 0x1000;

            let private_data4 = PrivateFvbData { _interface: fvb_intf_invalid, physical_address: base_no };
            // save the protocol structure we're about to install in the private data.
            PRIVATE_FV_DATA
                .lock()
                .fv_information
                .insert(fvb_intf_invalid_void, PrivateDataItem::FvbData(private_data4));

            /* Create a Firmware Volume Block Interface without Physical address populated  */
            let mut fvb_intf_data_n = Box::from(mu_pi::protocols::firmware_volume_block::Protocol {
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
            let fvb_intf_data_n_mut =
                fvb_intf_data_n.as_mut() as *mut mu_pi::protocols::firmware_volume_block::Protocol;

            unsafe {
                let fv_test_set_info = || {
                    fv_set_info(ptr::null(), ptr::null(), BUFFER_SIZE_EMPTY, ptr::null());
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
                    fv_get_volume_attributes(fv_ptr1, std::ptr::null_mut());

                    /* Handle bad firmware volume data - return efi::Status::NOT_FOUND */
                    fv_get_volume_attributes(fv_ptr_no_data, fv_attributes);

                    /* Handle Invalid Physical address case */
                    fv_get_volume_attributes(fv_ptr3_const, fv_attributes);

                    /* Non Null Case, success case */
                    fv_get_volume_attributes(fv_ptr1, fv_attributes);
                };

                let fv_test_fvb_read = || {
                    /* Mutable Reference cannot be borrowed more than once,
                     * hence delcare and free up after use immediately
                     */
                    let mut len3 = 1000;
                    let buffer_valid_size3: *mut usize = &mut len3;
                    let layout3 = Layout::from_size_align(1001, 8).unwrap();
                    let buffer_valid3 = alloc(layout3) as *mut c_void;

                    if buffer_valid3.is_null() {
                        panic!("Memory allocation failed!");
                    }
                    /* Handle various cases for different conditions to hit */
                    fvb_read(fvb_ptr_mut_prot, LBA, 0, std::ptr::null_mut(), std::ptr::null_mut());
                    fvb_read(fvb_ptr_mut_prot, LBA, 0, buffer_valid_size3, buffer_valid3);
                    fvb_read(fvb_ptr_mut_prot, 0xfffffffff, 0, buffer_valid_size3, buffer_valid3);
                    fvb_read(fvb_intf_invalid_mutpro, LBA, 0, buffer_valid_size3, buffer_valid3);
                    fvb_read(fvb_ptr_mut_prot, u64::MAX, 0, buffer_valid_size3, buffer_valid3);
                    fvb_read(fvb_ptr_mut_prot, 0x22299222, 0x999999, buffer_valid_size3, buffer_valid3);
                    fvb_read(fvb_intf_data_n_mut, LBA, 0, buffer_valid_size3, buffer_valid3);

                    /* Free Memory */
                    dealloc(buffer_valid3 as *mut u8, layout3);
                };

                let fv_test_get_block_size = || {
                    /* Mutable Reference cannot be borrowed more than once,
                     * hence delcare and free up after use immediately
                     */
                    let mut len3 = 1000;
                    let buffer_valid_size3: *mut usize = &mut len3;
                    let layout3 = Layout::from_size_align(1001, 8).unwrap();
                    let buffer_valid3 = alloc(layout3) as *mut c_void;

                    if buffer_valid3.is_null() {
                        panic!("Memory allocation failed!");
                    }

                    let mut buffer_size_random: usize = 99;
                    let buffer_size_random_ref: *mut usize = &mut buffer_size_random;
                    let mut num_buffer_empty: usize = 0;
                    let num_buffer_empty_ref: *mut usize = &mut num_buffer_empty;

                    /* Handle the Null Case */
                    fvb_get_block_size(fvb_ptr_mut_prot, LBA, std::ptr::null_mut(), std::ptr::null_mut());
                    fvb_get_block_size(fvb_ptr_mut_prot, LBA, buffer_valid_size3, buffer_valid_size3);
                    fvb_get_block_size(fvb_intf_invalid_mutpro, LBA, buffer_valid_size3, buffer_valid_size3);
                    fvb_get_block_size(fvb_intf_data_n_mut, LBA, buffer_valid_size3, buffer_valid_size3);
                    fvb_get_block_size(fvb_ptr_mut_prot, u64::MAX, buffer_valid_size3, buffer_valid_size3);
                    fvb_get_block_size(fvb_ptr_mut_prot, 222222, buffer_size_random_ref, num_buffer_empty_ref);
                    /* Free Memory */
                    dealloc(buffer_valid3 as *mut u8, layout3);
                };

                let fvb_test_erase_block = || {
                    fvb_erase_blocks(fvb_ptr_mut_prot);
                };

                let fvb_test_get_physical_address = || {
                    /* Handling Not Found Case */
                    let mut p_address: efi::PhysicalAddress = 0x12345;

                    fvb_get_physical_address(fvb_intf_data_n_mut, &mut p_address as *mut u64);
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
                    let buffer_valid3 = alloc(layout3) as *mut c_void;

                    if buffer_valid3.is_null() {
                        panic!("Memory allocation failed!");
                    }

                    fvb_write(fvb_ptr_mut_prot, LBA, 0, std::ptr::null_mut(), std::ptr::null_mut());
                    fvb_write(fvb_ptr_mut_prot, LBA, 0, buffer_valid_size3, buffer_valid3);
                    fvb_write(fvb_intf_invalid_mutpro, LBA, 0, buffer_valid_size3, buffer_valid3);
                    fvb_write(fvb_intf_data_n_mut, LBA, 0, buffer_valid_size3, buffer_valid3);
                    /* Free Memory */
                    dealloc(buffer_valid3 as *mut u8, layout3);
                };

                let fvb_test_get_attributes = || {
                    let mut fvb_attributes: fvb::attributes::EfiFvbAttributes2 = 0x123456;
                    let fvb_attributes_ref: *mut fvb::attributes::EfiFvbAttributes2 = &mut fvb_attributes;

                    fvb_get_attributes(fvb_ptr_mut_prot, std::ptr::null_mut());
                    fvb_get_attributes(fvb_ptr_mut_prot, fvb_attributes_ref);
                    fvb_get_attributes(fvb_intf_invalid_mutpro, fvb_attributes_ref);
                    fvb_get_attributes(fvb_intf_data_n_mut, fvb_attributes_ref);
                };

                let fvb_test_get_next_file = || {
                    /* Mutable Reference cannot be borrowed more than once,
                     * hence delcare and free up after use immediately
                     */
                    let mut len3 = 1000;
                    let buffer_valid_size3: *mut usize = &mut len3;
                    let layout3 = Layout::from_size_align(1001, 8).unwrap();
                    let buffer_valid3 = alloc(layout3) as *mut c_void;
                    let mut file_type_read: fv::EfiFvFileType = 1;
                    let file_type_read_ref: *mut fv::EfiFvFileType = &mut file_type_read;
                    let mut n_guid_mut: efi::Guid = efi::Guid::from_fields(0, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 0]);
                    let n_guid_ref_mut: *mut efi::Guid = &mut n_guid_mut;

                    if buffer_valid3.is_null() {
                        panic!("Memory allocation failed!");
                    }
                    fv_get_next_file(
                        ptr::null(),
                        std::ptr::null_mut(),
                        file_type_read_ref,
                        std::ptr::null_mut(),
                        file_attributes,
                        buffer_valid_size3,
                    );
                    fv_get_next_file(
                        ptr::null(),
                        buffer_valid3,
                        file_type_read_ref,
                        n_guid_ref_mut,
                        file_attributes,
                        buffer_valid_size3,
                    );
                    fv_get_next_file(
                        fv_ptr1,
                        buffer_valid3,
                        file_type_read_ref,
                        n_guid_ref_mut,
                        file_attributes,
                        buffer_valid_size3,
                    );
                    fv_get_next_file(
                        fv_ptr3_const,
                        buffer_valid3,
                        file_type_read_ref,
                        n_guid_ref_mut,
                        file_attributes,
                        buffer_valid_size3,
                    );
                    fv_get_next_file(
                        fv_ptr_no_data,
                        buffer_valid3,
                        file_type_read_ref,
                        n_guid_ref_mut,
                        file_attributes,
                        buffer_valid_size3,
                    );
                    /*handle  fw_fs::FfsFileRawType::FFS_MIN case */
                    let mut file_type_read: fv::EfiFvFileType = ffs::file::raw::r#type::FFS_MIN;
                    let file_type_read_ref1: *mut fv::EfiFvFileType = &mut file_type_read;

                    fv_get_next_file(
                        fv_ptr1,
                        buffer_valid3,
                        file_type_read_ref1,
                        n_guid_ref_mut,
                        file_attributes,
                        buffer_valid_size3,
                    );
                    /* Null BUffer Case*/
                    fv_get_next_file(
                        fv_ptr1,
                        std::ptr::null_mut(),
                        file_type_read_ref,
                        n_guid_ref_mut,
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
                    let mut len3 = 1000;
                    let buffer_valid_size3: *mut usize = &mut len3;
                    let layout3 = Layout::from_size_align(1001, 8).unwrap();
                    let mut buffer_valid3 = alloc(layout3) as *mut c_void;

                    if buffer_valid3.is_null() {
                        panic!("Memory allocation failed!");
                    }

                    let mut gd2: efi::Guid = efi::Guid::from_fields(
                        0x434f695c,
                        0xef26,
                        0x4a12,
                        0x9e,
                        0xba,
                        &[0xdd, 0xef, 0x00, 0x97, 0x49, 0x7c],
                    );
                    let name_guid2: *mut efi::Guid = &mut gd2;

                    /* Cover the NULL Case, User Passing Invalid Parameter Case  */
                    fv_read_section(
                        ptr::null(),
                        ptr::null(),
                        SECTION_TYPE,
                        SECTION_INSTANCE,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    );

                    fv_read_section(
                        fv_ptr1,
                        guid_ref_invalid_ref,
                        6,
                        10,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        auth_valid_p,
                    );

                    /* Valid guid case - panicing, debug this further, for now comment*/
                    /*fv_read_section(
                        fv_ptr1,
                        guid_valid_ref,
                        6,
                        10,
                       &mut buffer_valid3 as *mut *mut c_void,
                       buffer_valid_size3,
                       auth_valid_p,
                    );*/

                    fv_read_section(
                        fv_ptr1,
                        name_guid2,
                        6,
                        10,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        auth_valid_p,
                    );

                    /* Handle Invalid Physical address case */
                    fv_read_section(
                        fv_ptr3_const,
                        guid_ref_invalid_ref,
                        1,
                        1,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        auth_valid_p,
                    );

                    /* Handle bad firmware volume data - return efi::Status::NOT_FOUND */
                    fv_read_section(
                        fv_ptr_no_data,
                        guid_ref_invalid_ref,
                        1,
                        1,
                        &mut buffer_valid3 as *mut *mut c_void,
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
                    let mut len3 = 1000;
                    let buffer_valid_size3: *mut usize = &mut len3;
                    let layout3 = Layout::from_size_align(1001, 8).unwrap();
                    let mut buffer_valid3 = alloc(layout3) as *mut c_void;
                    let mut found_type: u8 = ffs::file::raw::r#type::DRIVER;
                    let found_type_ref: *mut fv::EfiFvFileType = &mut found_type;

                    if buffer_valid3.is_null() {
                        panic!("Memory allocation failed!");
                    }

                    fv_read_file(
                        ptr::null(),
                        ptr::null(),
                        &mut buffer_valid3 as *mut *mut c_void,
                        std::ptr::null_mut(),
                        found_type_ref,
                        file_attributes,
                        std::ptr::null_mut(),
                    );

                    fv_read_file(
                        fv_ptr1,
                        guid_ref_invalid_ref,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        found_type_ref,
                        file_attributes,
                        auth_valid_p,
                    );
                    fv_read_file(
                        fv_ptr1,
                        guid_valid_ref,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        found_type_ref,
                        file_attributes,
                        auth_valid_p,
                    );
                    fv_read_file(
                        fv_ptr3_const,
                        guid_valid_ref,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        found_type_ref,
                        file_attributes,
                        auth_valid_p,
                    );
                    fv_read_file(
                        fv_ptr_no_data,
                        guid_valid_ref,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        found_type_ref,
                        file_attributes,
                        auth_valid_p,
                    );
                    fv_read_file(
                        fv_ptr1,
                        guid_valid_ref,
                        std::ptr::null_mut(),
                        buffer_valid_size3,
                        found_type_ref,
                        file_attributes,
                        auth_valid_p,
                    );
                    /* Raise Bug for this case , case when Buffer size is 0 and buffer not NULL. last block*/
                    /*fv_read_file(fv_ptr1 , guid_valid_ref, (&mut buffer_valid as *mut *mut c_void),
                    buffer_equal_0p, found_type_ref, file_attributes,
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
        })
        .unwrap();
    }

    #[test]
    fn test_fv_special_section_read() {
        test_support::with_global_lock(|| {
            let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
            let mut fv: Vec<u8> = Vec::new();
            file.read_to_end(&mut fv).expect("failed to read test file");
            let base_address: u64 = fv.as_ptr() as u64;
            let parent_handle: Option<efi::Handle> = None;
            /* Start with Clearing Private Global Data, Please note that this is to be done only once
             * for test_fv_functionality.
             * In case other functions/modules are written, clear the private global data again.
             */
            unsafe {
                fv_private_data_reset();
            }
            assert!(PRIVATE_FV_DATA.lock().fv_information.is_empty());

            PRIVATE_FV_DATA.lock().section_extractor = Some(Box::new(patina_ffs_extractors::BrotliSectionExtractor));

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
            let fv_ptr1: *const mu_pi::protocols::firmware_volume::Protocol =
                fv_ptr as *const mu_pi::protocols::firmware_volume::Protocol;

            unsafe {
                let layout = Layout::from_size_align(1000, 8).unwrap();
                let mut buffer = alloc(layout) as *mut c_void;

                if buffer.is_null() {
                    panic!("Memory allocation failed!");
                }

                let mut len = 1000;
                let buffer_size: *mut usize = &mut len;
                let mut authentication_status: u32 = 1;
                let authentication_statusp: *mut u32 = &mut authentication_status;
                let mut guid1: efi::Guid = efi::Guid::from_fields(
                    0x1fa1f39e,
                    0xfeff,
                    0x4aae,
                    0xbd,
                    0x7b,
                    &[0x38, 0xa0, 0x70, 0xa3, 0xb6, 0x09],
                );
                let name_guid3: *mut efi::Guid = &mut guid1;

                fv_read_section(
                    fv_ptr1,
                    name_guid3,
                    6,
                    10,
                    &mut buffer as *mut *mut c_void,
                    buffer_size,
                    authentication_statusp,
                );

                // Deallocate the memory
                dealloc(buffer as *mut u8, layout);
            }
        })
        .expect("Failed to read Firmware Volume Section");
    }
}
