//! DXE Core Firmware Volume (FV)
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{
    ffi::c_void,
    mem::{self, size_of},
    slice,
};

use alloc::{boxed::Box, collections::BTreeMap};
use patina::pi::{
    self,
    fw_fs::{ffs, fv, fvb},
    hob,
};

use patina::{component::service::Service, error::EfiError};
use patina_ffs::{section::SectionExtractor, volume::VolumeRef};
use patina_internal_device_path::concat_device_path_to_boxed_slice;
use r_efi::efi;

use crate::{
    allocator::core_allocate_pool,
    decompress::CoreExtractor,
    protocols::{PROTOCOL_DB, core_install_protocol_interface},
    tpl_mutex,
};

struct PrivateFvbData {
    _interface: Box<pi::protocols::firmware_volume_block::Protocol>,
    physical_address: u64,
}

struct PrivateFvData {
    _interface: Box<pi::protocols::firmware_volume::Protocol>,
    physical_address: u64,
}

enum PrivateDataItem {
    FvbData(PrivateFvbData),
    FvData(PrivateFvData),
}

struct PrivateGlobalData {
    fv_information: BTreeMap<*mut c_void, PrivateDataItem>,
    section_extractor: CoreExtractor,
}

// Safety: access to private global data is only through mutex guard, so safe to mark sync/send.
unsafe impl Sync for PrivateGlobalData {}
unsafe impl Send for PrivateGlobalData {}

static PRIVATE_FV_DATA: tpl_mutex::TplMutex<PrivateGlobalData> = tpl_mutex::TplMutex::new(
    efi::TPL_NOTIFY,
    PrivateGlobalData { fv_information: BTreeMap::new(), section_extractor: CoreExtractor::new() },
    "FvLock",
);

// FVB Protocol Functions
extern "efiapi" fn fvb_get_attributes(
    this: *mut pi::protocols::firmware_volume_block::Protocol,
    attributes: *mut fvb::attributes::EfiFvbAttributes2,
) -> efi::Status {
    if attributes.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    match core_fvb_get_attributes(this) {
        Err(err) => return err.into(),
        // Safety: caller must provide a valid pointer to receive the attributes. It is null-checked above.
        Ok(fvb_attributes) => unsafe { attributes.write_unaligned(fvb_attributes) },
    };

    efi::Status::SUCCESS
}

fn core_fvb_get_attributes(
    this: *mut pi::protocols::firmware_volume_block::Protocol,
) -> Result<fvb::attributes::EfiFvbAttributes2, EfiError> {
    let private_data = PRIVATE_FV_DATA.lock();

    let Some(PrivateDataItem::FvbData(fvb_data)) = private_data.fv_information.get(&(this as *mut c_void)) else {
        return Err(EfiError::NotFound);
    };

    // Safety: fvb_data.physical_address must point to a valid FV (i.e. private_data is correctly constructed and
    // its invariants - like not removing fv once installed - are upheld).
    let fv = unsafe { VolumeRef::new_from_address(fvb_data.physical_address)? };

    Ok(fv.attributes())
}

extern "efiapi" fn fvb_set_attributes(
    _this: *mut pi::protocols::firmware_volume_block::Protocol,
    _attributes: *mut fvb::attributes::EfiFvbAttributes2,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

extern "efiapi" fn fvb_get_physical_address(
    this: *mut pi::protocols::firmware_volume_block::Protocol,
    address: *mut efi::PhysicalAddress,
) -> efi::Status {
    if address.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let private_data = PRIVATE_FV_DATA.lock();

    let Some(PrivateDataItem::FvbData(fvb_data)) = private_data.fv_information.get(&(this as *mut c_void)) else {
        return efi::Status::NOT_FOUND;
    };

    // Safety: caller must provide a valid pointer to receive the address. It is null-checked above.
    unsafe { address.write_unaligned(fvb_data.physical_address) };

    efi::Status::SUCCESS
}

extern "efiapi" fn fvb_get_block_size(
    this: *mut pi::protocols::firmware_volume_block::Protocol,
    lba: efi::Lba,
    block_size: *mut usize,
    number_of_blocks: *mut usize,
) -> efi::Status {
    if block_size.is_null() || number_of_blocks.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let (size, remaining_blocks) = match core_fvb_get_block_size(this, lba) {
        Err(err) => return err.into(),
        Ok((size, remaining_blocks)) => (size, remaining_blocks),
    };

    // Safety: caller must provide valid pointers to receive the block size and number of blocks. They are null-checked above.
    unsafe {
        block_size.write_unaligned(size);
        number_of_blocks.write_unaligned(remaining_blocks);
    }

    efi::Status::SUCCESS
}

fn core_fvb_get_block_size(
    this: *mut pi::protocols::firmware_volume_block::Protocol,
    lba: efi::Lba,
) -> Result<(usize, usize), EfiError> {
    let private_data = PRIVATE_FV_DATA.lock();

    let Some(PrivateDataItem::FvbData(fvb_data)) = private_data.fv_information.get(&(this as *mut c_void)) else {
        return Err(EfiError::NotFound);
    };

    // Safety: fvb_data.physical_address must point to a valid FV (i.e. private_data is correctly constructed and
    // its invariants - like not removing fv once installed - are upheld).
    let fv = unsafe { VolumeRef::new_from_address(fvb_data.physical_address)? };

    let lba: u32 = lba.try_into().map_err(|_| EfiError::InvalidParameter)?;

    let (block_size, remaining_blocks, _) = fv.lba_info(lba)?;

    Ok((block_size as usize, remaining_blocks as usize))
}

extern "efiapi" fn fvb_read(
    this: *mut pi::protocols::firmware_volume_block::Protocol,
    lba: efi::Lba,
    offset: usize,
    num_bytes: *mut usize,
    buffer: *mut core::ffi::c_void,
) -> efi::Status {
    if num_bytes.is_null() || buffer.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // Safety: caller must provide valid pointers for num_bytes and buffer. They are null-checked above.
    let bytes_to_read = unsafe { *num_bytes };

    let data = match core_fvb_read(this, lba, offset, bytes_to_read) {
        Err(err) => return err.into(),
        Ok(data) => data,
    };

    if data.len() > bytes_to_read {
        // Safety: caller must provide a valid pointer for num_bytes. It is null-checked above.
        unsafe { num_bytes.write_unaligned(data.len()) };
        return efi::Status::BUFFER_TOO_SMALL;
    }

    // copy from memory into the destination buffer to do the read.
    // Safety: buffer must be valid for writes of at least bytes_to_read length. It is null-checked above, and
    // the caller must ensure that the buffer is large enough to hold the data being read.
    unsafe {
        let dest_buffer = slice::from_raw_parts_mut(buffer as *mut u8, data.len());
        dest_buffer.copy_from_slice(data);
        num_bytes.write_unaligned(data.len());
    }

    if data.len() != bytes_to_read { efi::Status::BAD_BUFFER_SIZE } else { efi::Status::SUCCESS }
}

fn core_fvb_read(
    this: *mut pi::protocols::firmware_volume_block::Protocol,
    lba: efi::Lba,
    offset: usize,
    num_bytes: usize,
) -> Result<&'static [u8], EfiError> {
    let private_data = PRIVATE_FV_DATA.lock();

    let Some(PrivateDataItem::FvbData(fvb_data)) = private_data.fv_information.get(&(this as *mut c_void)) else {
        return Err(EfiError::NotFound);
    };

    // Safety: fvb_data.physical_address must point to a valid FV (i.e. private_data is correctly constructed and
    // its invariants - like not removing fv once installed - are upheld).
    let fv = unsafe { VolumeRef::new_from_address(fvb_data.physical_address) }?;

    let Ok(lba) = lba.try_into() else {
        return Err(EfiError::InvalidParameter);
    };

    let (lba_base_addr, block_size) = fv.lba_info(lba).map(|(addr, size, _)| (addr as usize, size as usize))?;

    let mut bytes_to_read = num_bytes;
    if offset + bytes_to_read > block_size {
        debug_assert!(offset + bytes_to_read <= block_size); // caller should not request to read beyond the block.
        bytes_to_read = block_size - offset;
    }

    let lba_start = (fvb_data.physical_address as usize + lba_base_addr + offset) as *mut u8;
    // Safety: lba_start is calculated from the base address of a valid FV, plus an offset and offset+num_bytes.
    // consistency of this data is guaranteed by checks on instantiation of the VolumeRef.
    // The FV data is expected to be 'static (i.e. permanently mapped) for the lifetime of the system.
    unsafe { Ok(slice::from_raw_parts(lba_start, bytes_to_read)) }
}

extern "efiapi" fn fvb_write(
    _this: *mut pi::protocols::firmware_volume_block::Protocol,
    _lba: efi::Lba,
    _offset: usize,
    _num_bytes: *mut usize,
    _buffer: *mut core::ffi::c_void,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

extern "efiapi" fn fvb_erase_blocks(
    _this: *mut pi::protocols::firmware_volume_block::Protocol,
    //... TODO: this should be variadic; however, variadic and eficall don't mix well presently.
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

fn install_fvb_protocol(
    handle: Option<efi::Handle>,
    parent_handle: Option<efi::Handle>,
    base_address: u64,
) -> Result<efi::Handle, EfiError> {
    let mut fvb_interface = Box::from(pi::protocols::firmware_volume_block::Protocol {
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

    let fvb_ptr = fvb_interface.as_mut() as *mut pi::protocols::firmware_volume_block::Protocol as *mut c_void;

    let private_data = PrivateFvbData { _interface: fvb_interface, physical_address: base_address };

    // save the protocol structure we're about to install in the private data.
    PRIVATE_FV_DATA.lock().fv_information.insert(fvb_ptr, PrivateDataItem::FvbData(private_data));

    // install the protocol and return status
    core_install_protocol_interface(handle, pi::protocols::firmware_volume_block::PROTOCOL_GUID, fvb_ptr)
}

// Firmware Volume protocol functions
extern "efiapi" fn fv_get_volume_attributes(
    this: *const pi::protocols::firmware_volume::Protocol,
    fv_attributes: *mut fv::attributes::EfiFvAttributes,
) -> efi::Status {
    if fv_attributes.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let fv_attributes_data = match core_fv_get_volume_attributes(this) {
        Err(err) => return err.into(),
        Ok(attrs) => attrs,
    };

    // Safety: caller must provide a valid pointer to receive the attributes. It is null-checked above.
    unsafe { fv_attributes.write_unaligned(fv_attributes_data) };

    efi::Status::SUCCESS
}

fn core_fv_get_volume_attributes(
    this: *const pi::protocols::firmware_volume::Protocol,
) -> Result<fv::attributes::EfiFvAttributes, EfiError> {
    let private_data = PRIVATE_FV_DATA.lock();

    let Some(PrivateDataItem::FvData(fv_data)) = private_data.fv_information.get(&(this as *mut c_void)) else {
        return Err(EfiError::NotFound);
    };

    // Safety: fvb_data.physical_address must point to a valid FV (i.e. private_data is correctly constructed and
    // its invariants - like not removing fv once installed - are upheld).
    let fv = unsafe { VolumeRef::new_from_address(fv_data.physical_address)? };

    Ok(fv.attributes() as fv::attributes::EfiFvAttributes)
}

extern "efiapi" fn fv_set_volume_attributes(
    _this: *const pi::protocols::firmware_volume::Protocol,
    _fv_attributes: *mut fv::attributes::EfiFvAttributes,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

extern "efiapi" fn fv_read_file(
    this: *const pi::protocols::firmware_volume::Protocol,
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

    // Safety: caller must provide valid pointers for buffer_size and name_guid. They are null-checked above.
    let local_buffer_size = unsafe { buffer_size.read_unaligned() };
    let local_name_guid = unsafe { name_guid.read_unaligned() };

    // for this routine, the file data should be copied into the output buffer directly from the FileRef
    // constructed here. If this logic was moved into a `core_fv_read_file()` routine as with other functions
    // in this file, the FileRef would be local to that routine and the data slice could not be returned without
    // making a copy of the data (or otherwise working around the lifetime issues with e.g. unpalatable raw ptr
    // shenanigans).
    let private_data = PRIVATE_FV_DATA.lock();

    let Some(PrivateDataItem::FvData(fv_data)) = private_data.fv_information.get(&(this as *mut c_void)) else {
        return efi::Status::NOT_FOUND;
    };

    // Safety: fvb_data.physical_address must point to a valid FV (i.e. private_data is correctly constructed and
    // its invariants - like not removing fv once installed - are upheld).
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
    // Safety: caller must provide valid pointers for found_type, file_attributes, and buffer_size. They are null-checked above.
    unsafe {
        found_type.write_unaligned(file.file_type_raw());
        file_attributes.write_unaligned(file.fv_attributes());
        //TODO: Authentication status is not yet supported.
        buffer_size.write_unaligned(file.content().len());
    }

    if buffer.is_null() {
        //caller just wants file meta data, no need to read file data.
        return efi::Status::SUCCESS;
    }

    // Safety: caller must provide a valid pointer for buffer. It is null-checked above.
    let mut local_buffer_ptr = unsafe { buffer.read_unaligned() };

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
            // Safety: caller must provide a valid pointer for buffer. It is null-checked above.
            Ok(allocation) => unsafe {
                local_buffer_ptr = allocation;
                buffer.write_unaligned(local_buffer_ptr);
            },
        }
    }

    // convert pointer+size into a slice and copy the file data.
    // Safety: local_buffer_ptr is either provided by the caller (and null-checked above), or allocated via allocate pool
    // and is of sufficient size to contian the data.
    let out_buffer = unsafe { slice::from_raw_parts_mut(local_buffer_ptr as *mut u8, file.content().len()) };
    out_buffer.copy_from_slice(file.content());

    efi::Status::SUCCESS
}

extern "efiapi" fn fv_read_section(
    this: *const pi::protocols::firmware_volume::Protocol,
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

    // Safety: caller must provide valid pointer for name_guid. It is null-checked above.
    let local_name_guid = unsafe { name_guid.read_unaligned() };

    let section = match core_fv_read_section(this, local_name_guid, section_type, section_instance) {
        Ok(section) => section,
        Err(err) => return err.into(),
    };

    let section_data = match section.try_content_as_slice() {
        Ok(data) => data,
        Err(err) => return err.into(),
    };

    // get the buffer_size and buffer parameters from caller.
    // Safety: null-checks are at the start of the routine, but caller is required to guarantee that buffer_size and
    // buffer are valid.
    let mut local_buffer_size = unsafe { buffer_size.read_unaligned() };
    let mut local_buffer_ptr = unsafe { buffer.read_unaligned() };

    if local_buffer_ptr.is_null() {
        //caller indicates that they wish to receive section data, but that this
        //routine should allocate a buffer of appropriate size. Since the caller
        //is expected to free this buffer via free_pool, we need to manually
        //allocate it via allocate_pool.
        match core_allocate_pool(efi::BOOT_SERVICES_DATA, section_data.len()) {
            Err(err) => return err.into(),
            // Safety: caller is required to guarantee that buffer_size and buffer are valid.
            Ok(allocation) => unsafe {
                local_buffer_size = section_data.len();
                local_buffer_ptr = allocation;
                buffer_size.write_unaligned(local_buffer_size);
                buffer.write_unaligned(local_buffer_ptr);
            },
        }
    } else {
        // update buffer size output for the caller
        // Safety: null-checked at the start of the routine, but caller is required to guarantee buffer_size is valid.
        unsafe {
            buffer_size.write_unaligned(section_data.len());
        }
    }

    //copy bytes to output. Caller-provided buffer may be shorter than section
    //data. If so, copy to fill the destination buffer, and return
    //WARN_BUFFER_TOO_SMALL.

    // Safety: local_buffer_ptr is either provided by the caller (and null-checked above), or allocated via allocate pool and
    // is of sufficient size to contain the data.
    let dest_buffer = unsafe { slice::from_raw_parts_mut(local_buffer_ptr as *mut u8, local_buffer_size) };
    dest_buffer.copy_from_slice(&section_data[0..dest_buffer.len()]);

    //TODO: authentication status not yet supported.

    if dest_buffer.len() < section_data.len() { efi::Status::WARN_BUFFER_TOO_SMALL } else { efi::Status::SUCCESS }
}

fn core_fv_read_section(
    this: *const pi::protocols::firmware_volume::Protocol,
    name_guid: efi::Guid,
    section_type: ffs::section::EfiSectionType,
    section_instance: usize,
) -> Result<patina_ffs::section::Section, EfiError> {
    let private_data = PRIVATE_FV_DATA.lock();

    let Some(PrivateDataItem::FvData(fv_data)) = private_data.fv_information.get(&(this as *mut c_void)) else {
        return Err(EfiError::NotFound);
    };

    // Safety: fvb_data.physical_address must point to a valid FV (i.e. private_data is correctly constructed and
    // its invariants - like not removing fv once installed - are upheld).
    let fv = unsafe { VolumeRef::new_from_address(fv_data.physical_address) }?;

    if (fv.attributes() & fvb::attributes::raw::fvb2::READ_STATUS) == 0 {
        return Err(EfiError::AccessDenied);
    }

    let file = match fv.files().find(|f| f.as_ref().is_ok_and(|f| f.name() == name_guid) || f.is_err()) {
        Some(Ok(result)) => result,
        Some(Err(err)) => return Err(err.into()),
        _ => return Err(EfiError::NotFound),
    };

    let extractor = &private_data.section_extractor;
    let sections = file.sections_with_extractor(extractor)?;

    sections
        .iter()
        .filter(|sec| sec.section_type_raw() == section_type)
        .nth(section_instance)
        .cloned()
        .ok_or(EfiError::NotFound)
}

extern "efiapi" fn fv_write_file(
    _this: *const pi::protocols::firmware_volume::Protocol,
    _number_of_files: u32,
    _write_policy: pi::protocols::firmware_volume::EfiFvWritePolicy,
    _file_data: *mut pi::protocols::firmware_volume::EfiFvWriteFileData,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

extern "efiapi" fn fv_get_next_file(
    this: *const pi::protocols::firmware_volume::Protocol,
    key: *mut c_void,
    file_type: *mut fv::EfiFvFileType,
    name_guid: *mut efi::Guid,
    attributes: *mut fv::file::EfiFvFileAttributes,
    size: *mut usize,
) -> efi::Status {
    if key.is_null() || file_type.is_null() || name_guid.is_null() || attributes.is_null() || size.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // Safety: caller must provide valid pointers for key and file_type. They are null-checked above.
    let local_key = unsafe { (key as *mut usize).read_unaligned() };
    let local_file_type = unsafe { file_type.read_unaligned() };

    if local_file_type >= ffs::file::raw::r#type::FFS_MIN {
        return efi::Status::NOT_FOUND;
    }

    let (file_name, fv_attributes, file_size, found_file_type) =
        match core_fv_get_next_file(this, local_file_type, local_key) {
            Err(err) => return err.into(),
            Ok((name, attrs, size, file_type)) => (name, attrs, size, file_type),
        };
    // found matching file. Update the key and outputs.
    // Safety: caller must provide valid pointers for key, file_type, name_guid, attributes, and size. They are null-checked above.
    unsafe {
        (key as *mut usize).write_unaligned(local_key + 1);
        name_guid.write_unaligned(file_name);
        if (fv_attributes & fvb::attributes::raw::fvb2::MEMORY_MAPPED) == fvb::attributes::raw::fvb2::MEMORY_MAPPED {
            attributes.write_unaligned(fv_attributes | fv::file::raw::attribute::MEMORY_MAPPED);
        } else {
            attributes.write_unaligned(fv_attributes);
        }
        size.write_unaligned(file_size);
        file_type.write_unaligned(found_file_type);
    }

    efi::Status::SUCCESS
}

fn core_fv_get_next_file(
    this: *const pi::protocols::firmware_volume::Protocol,
    file_type: fv::EfiFvFileType,
    key: usize,
) -> Result<(efi::Guid, fv::file::EfiFvFileAttributes, usize, fv::EfiFvFileType), EfiError> {
    let private_data = PRIVATE_FV_DATA.lock();

    let Some(PrivateDataItem::FvData(fv_data)) = private_data.fv_information.get(&(this as *mut c_void)) else {
        return Err(EfiError::NotFound);
    };

    // Safety: fvb_data.physical_address must point to a valid FV (i.e. private_data is correctly constructed and
    // its invariants - like not removing fv once installed - are upheld).
    let fv = unsafe { VolumeRef::new_from_address(fv_data.physical_address) }?;

    let fv_attributes = fv.attributes();

    if (fv_attributes & fvb::attributes::raw::fvb2::READ_STATUS) == 0 {
        return Err(EfiError::AccessDenied);
    }

    let file_candidate = fv
        .files()
        .filter(|f| {
            f.is_err()
                || file_type == ffs::file::raw::r#type::ALL
                || f.as_ref().is_ok_and(|f| f.file_type_raw() == file_type)
        })
        .nth(key);

    let file = match file_candidate {
        Some(Err(err)) => return Err(err.into()),
        Some(Ok(file)) => file,
        _ => return Err(EfiError::NotFound),
    };

    let attributes =
        if (fv_attributes & fvb::attributes::raw::fvb2::MEMORY_MAPPED) == fvb::attributes::raw::fvb2::MEMORY_MAPPED {
            file.fv_attributes() | fv::file::raw::attribute::MEMORY_MAPPED
        } else {
            file.fv_attributes()
        };

    Ok((file.name(), attributes, file.data().len(), file.file_type_raw()))
}

extern "efiapi" fn fv_get_info(
    _this: *const pi::protocols::firmware_volume::Protocol,
    _information_type: *const efi::Guid,
    _buffer_size: *mut usize,
    _buffer: *mut c_void,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

extern "efiapi" fn fv_set_info(
    _this: *const pi::protocols::firmware_volume::Protocol,
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
    let mut fv_interface = Box::from(pi::protocols::firmware_volume::Protocol {
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

    let fv_ptr = fv_interface.as_mut() as *mut pi::protocols::firmware_volume::Protocol as *mut c_void;

    let private_data = PrivateFvData { _interface: fv_interface, physical_address: base_address };

    // save the protocol structure we're about to install in the private data.
    PRIVATE_FV_DATA.lock().fv_information.insert(fv_ptr, PrivateDataItem::FvData(private_data));

    // install the protocol and return status
    core_install_protocol_interface(handle, pi::protocols::firmware_volume::PROTOCOL_GUID, fv_ptr)
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
    // Safety: caller must ensure that base_address is valid.
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

// Safety: base_address must point to a valid firmware volume.
pub unsafe fn core_install_firmware_volume(
    base_address: u64,
    parent_handle: Option<efi::Handle>,
) -> Result<efi::Handle, EfiError> {
    // Safety: caller must ensure that base_address is valid.
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

/// Parse the FVs defined in the HOB list.
pub fn parse_hob_fvs(hob_list: &hob::HobList) -> Result<(), efi::Status> {
    let fv_hobs = hob_list.iter().filter_map(|h| if let hob::Hob::FirmwareVolume(fv) = h { Some(*fv) } else { None });

    for fv in fv_hobs {
        // construct a FirmwareVolume struct to verify sanity.
        // Safety: base addresses of FirmwareVolume HOBs are assumed to be valid and accessible.
        let fv_slice = unsafe { slice::from_raw_parts(fv.base_address as *const u8, fv.length as usize) };
        VolumeRef::new(fv_slice)?;
        // Safety: base addresses of FirmwareVolume HOBs are assumed to be valid and accessible.
        unsafe { core_install_firmware_volume(fv.base_address, None) }?;
    }
    Ok(())
}

/// Registers a section extractor to be used when reading sections from files in firmware volumes.
pub fn register_section_extractor(extractor: Service<dyn SectionExtractor>) {
    PRIVATE_FV_DATA.lock().section_extractor.set_extractor(extractor);
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::test_support;
    use patina::pi::{
        BootMode,
        hob::{self, Hob, HobList},
    };
    use patina_ffs_extractors::CompositeSectionExtractor;
    extern crate alloc;
    use crate::test_collateral;
    use std::{
        alloc::{Layout, alloc, dealloc},
        ffi::c_void,
        fs::File,
        io::Read,
        ptr,
    };

    //Populate Null References for error cases
    const BUFFER_SIZE_EMPTY: usize = 0;
    const LBA: u64 = 0;
    const SECTION_TYPE: ffs::section::EfiSectionType = 0;
    const SECTION_INSTANCE: usize = 0;

    // Safety: resets all the private data; so caller must ensure that no code exists that
    // assumes the private data is valid (i.e. that FVs that it describes still exist).
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
            // Safety: global lock ensures exclusive access to the private data.
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
            parse_hob_fvs(&hoblist).unwrap();
            register_section_extractor(Service::mock(Box::new(CompositeSectionExtractor::default())));
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
            // Safety: global lock ensures exclusive access to the private data.
            unsafe {
                fv_private_data_reset();
            }
            assert!(PRIVATE_FV_DATA.lock().fv_information.is_empty());

            /* Create Firmware Interface, this will be used by the whole test module */
            let mut fv_interface = Box::from(pi::protocols::firmware_volume::Protocol {
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

            let fv_ptr = fv_interface.as_mut() as *mut pi::protocols::firmware_volume::Protocol as *mut c_void;

            let private_data = PrivateFvData { _interface: fv_interface, physical_address: base_address };
            // save the protocol structure we're about to install in the private data.
            PRIVATE_FV_DATA.lock().fv_information.insert(fv_ptr, PrivateDataItem::FvData(private_data));
            let fv_ptr1: *const pi::protocols::firmware_volume::Protocol =
                fv_ptr as *const pi::protocols::firmware_volume::Protocol;

            /* Build Firmware Volume Block Interface*/
            let mut fvb_interface = Box::from(pi::protocols::firmware_volume_block::Protocol {
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
            let fvb_ptr = fvb_interface.as_mut() as *mut pi::protocols::firmware_volume_block::Protocol as *mut c_void;
            let fvb_ptr_mut_prot = fvb_interface.as_mut() as *mut pi::protocols::firmware_volume_block::Protocol;

            /* Build Private Data */
            let private_data = PrivateFvbData { _interface: fvb_interface, physical_address: base_address };
            // save the protocol structure we're about to install in the private data.
            PRIVATE_FV_DATA.lock().fv_information.insert(fvb_ptr, PrivateDataItem::FvbData(private_data));

            //let fv_attributes3: *mut fw_fs::EfiFvAttributes = &mut fv_att;

            /* Instance 2 - Create a FV  interface with Bad physical address to handle Error cases. */
            let mut fv_interface3 = Box::from(pi::protocols::firmware_volume::Protocol {
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

            let fv_ptr3 = fv_interface3.as_mut() as *mut pi::protocols::firmware_volume::Protocol as *mut c_void;
            let fv_ptr3_const: *const pi::protocols::firmware_volume::Protocol =
                fv_ptr3 as *const pi::protocols::firmware_volume::Protocol;

            /* Corrupt the base address to cover error conditions  */
            let base_no2: u64 = fv.as_ptr() as u64 + 0x1000;
            let private_data2 = PrivateFvData { _interface: fv_interface3, physical_address: base_no2 };
            //save the protocol structure we're about to install in the private data.
            PRIVATE_FV_DATA.lock().fv_information.insert(fv_ptr3, PrivateDataItem::FvData(private_data2));

            /* Create an interface with No physical address and no private data - cover Error Conditions */
            let fv_interface_no_data = pi::protocols::firmware_volume::Protocol {
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

            let fv_ptr_no_data = &fv_interface_no_data as *const pi::protocols::firmware_volume::Protocol;

            /* Create a Firmware Volume Block Interface with Invalid Physical Address */
            let mut fvb_intf_invalid = Box::from(pi::protocols::firmware_volume_block::Protocol {
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
                fvb_intf_invalid.as_mut() as *mut pi::protocols::firmware_volume_block::Protocol as *mut c_void;
            let fvb_intf_invalid_mutpro =
                fvb_intf_invalid.as_mut() as *mut pi::protocols::firmware_volume_block::Protocol;
            let base_no: u64 = fv.as_ptr() as u64 + 0x1000;

            let private_data4 = PrivateFvbData { _interface: fvb_intf_invalid, physical_address: base_no };
            // save the protocol structure we're about to install in the private data.
            PRIVATE_FV_DATA
                .lock()
                .fv_information
                .insert(fvb_intf_invalid_void, PrivateDataItem::FvbData(private_data4));

            /* Create a Firmware Volume Block Interface without Physical address populated  */
            let mut fvb_intf_data_n = Box::from(pi::protocols::firmware_volume_block::Protocol {
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
            let fvb_intf_data_n_mut = fvb_intf_data_n.as_mut() as *mut pi::protocols::firmware_volume_block::Protocol;

            // Safety: the following test code must uphold the safety expectations of the unsafe
            // functions it calls. It uses direct memory allocations to create buffers for testing FFI
            // functions.
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
                    let write_policy: pi::protocols::firmware_volume::EfiFvWritePolicy = 0;
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
            // Safety: global lock ensures exclusive access to the private data.
            unsafe {
                fv_private_data_reset();
            }
            assert!(PRIVATE_FV_DATA.lock().fv_information.is_empty());

            PRIVATE_FV_DATA
                .lock()
                .section_extractor
                .set_extractor(Service::mock(Box::new(patina_ffs_extractors::BrotliSectionExtractor)));

            let mut fv_interface = Box::from(pi::protocols::firmware_volume::Protocol {
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

            let fv_ptr = fv_interface.as_mut() as *mut pi::protocols::firmware_volume::Protocol as *mut c_void;

            let private_data = PrivateFvData { _interface: fv_interface, physical_address: base_address };
            // save the protocol structure we're about to install in the private data.
            PRIVATE_FV_DATA.lock().fv_information.insert(fv_ptr, PrivateDataItem::FvData(private_data));
            let fv_ptr1: *const pi::protocols::firmware_volume::Protocol =
                fv_ptr as *const pi::protocols::firmware_volume::Protocol;

            // Safety: the following test code must uphold the safety expectations of the unsafe
            // functions it calls. It uses direct memory management to test fv FFI primitives.
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
