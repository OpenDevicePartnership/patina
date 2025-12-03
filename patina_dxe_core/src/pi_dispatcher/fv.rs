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
    num::NonZeroUsize,
    ptr::NonNull,
    slice,
};

use alloc::{boxed::Box, collections::BTreeMap};
use patina::pi::{
    self,
    fw_fs::{ffs, fv, fvb},
    hob,
};

use patina::error::EfiError;
use patina_ffs::{file::FileRef, section::SectionExtractor, volume::VolumeRef};
use patina_internal_device_path::concat_device_path_to_boxed_slice;
use r_efi::efi::{self, MEMORY_MAPPED_IO};

use crate::{
    Core, PlatformInfo,
    allocator::core_allocate_pool,
    protocols::{PROTOCOL_DB, core_install_protocol_interface},
    tpl_mutex,
};

/// A container for a FV or FVB protocol instance.
///
/// The protocol instances themselves are not used directly by rust code, but this container is used to manage the
/// lifetime of the protocols and to validate accesses to them.
#[allow(dead_code)]
enum Protocol {
    /// A heap-allocated FV protocol instance.
    Fv(&'static pi::protocols::firmware_volume::Protocol),
    /// A heap-allocated FVB protocol instance.
    Fvb(&'static pi::protocols::firmware_volume_block::Protocol),
}

/// The metadata associated with a given FV / FVB protocol installation.
struct Metadata {
    /// The installed protocol instance.
    protocol: Protocol,
    /// The physical address of the FV / FVB associated with the protocol.
    physical_address: u64,
}

impl Metadata {
    /// Creates a new Metadata instance for a FVB protocol.
    fn new_fvb(protocol: Box<pi::protocols::firmware_volume_block::Protocol>, physical_address: u64) -> Self {
        Self { protocol: Protocol::Fvb(Box::leak(protocol)), physical_address }
    }

    /// Creates a new Metadata instance for a FV protocol.
    fn new_fv(protocol: Box<pi::protocols::firmware_volume::Protocol>, physical_address: u64) -> Self {
        Self { protocol: Protocol::Fv(Box::leak(protocol)), physical_address }
    }
}

/// Stored protocol data for any FV/FVB protocols installed by the DXE core.
pub(super) struct FvProtocolData<P: PlatformInfo> {
    /// A map of installed FV/FVB protocol pointers (key) and the corresponding metadata (value).
    fv_metadata: BTreeMap<NonZeroUsize, Metadata>,
    /// A marker for accessing the singleton `Core` instance in a UEFI protocol method.
    _platform_info: core::marker::PhantomData<P>,
}

impl<P: PlatformInfo> FvProtocolData<P> {
    /// Returns the FV's physical address for the given protocol pointer, if it is in-fact a FV protocol.
    #[inline(always)]
    fn get_fv_address(&self, protocol: NonNull<pi::protocols::firmware_volume::Protocol>) -> Option<u64> {
        if let Some(Metadata { protocol: Protocol::Fv(_), physical_address }) = self.fv_metadata.get(&protocol.addr()) {
            Some(*physical_address)
        } else {
            None
        }
    }

    /// Returns the FVB's physical address for the given protocol pointer, if it is in-fact a FVB protocol.
    #[inline(always)]
    fn get_fvb_address(&self, protocol: NonNull<pi::protocols::firmware_volume_block::Protocol>) -> Option<u64> {
        if let Some(Metadata { protocol: Protocol::Fvb(_), physical_address }) = self.fv_metadata.get(&protocol.addr())
        {
            Some(*physical_address)
        } else {
            None
        }
    }
}

impl<P: PlatformInfo> FvProtocolData<P> {
    /// Creates a new [FvProtocolData] instance.
    pub const fn new() -> Self {
        Self { fv_metadata: BTreeMap::new(), _platform_info: core::marker::PhantomData }
    }

    /// Creates a new [TplMutex] wrapping a new [FvProtocolData] instance.
    pub const fn new_locked() -> tpl_mutex::TplMutex<Self> {
        tpl_mutex::TplMutex::new(efi::TPL_NOTIFY, Self::new(), "FvData")
    }

    /// Returns a locked instance of the global [FvProtocolData].
    fn instance<'a>() -> tpl_mutex::TplGuard<'a, Self> {
        Core::<P>::instance().pi_dispatcher.fv_data.lock()
    }

    /// Rust implementation of the FVB protocol's get_attributes method.
    fn fvb_get_attributes(
        &self,
        protocol: NonNull<pi::protocols::firmware_volume_block::Protocol>,
    ) -> Result<fvb::attributes::EfiFvbAttributes2, EfiError> {
        let physical_address = self.get_fvb_address(protocol).ok_or(EfiError::NotFound)?;

        // Safety: physical_address must point to a valid FV (i.e. private_data is correctly constructed and
        // its invariants - like not removing fv once installed - are upheld).
        let fv = unsafe { VolumeRef::new_from_address(physical_address)? };
        Ok(fv.attributes())
    }

    /// Rust implementation of the FVB protocol's get_physical_address method.
    fn fvb_get_physical_address(
        &self,
        protocol: NonNull<pi::protocols::firmware_volume_block::Protocol>,
    ) -> Result<efi::PhysicalAddress, EfiError> {
        let physical_address = self.get_fvb_address(protocol).ok_or(EfiError::NotFound)?;

        Ok(physical_address as efi::PhysicalAddress)
    }

    /// Rust implementation of the FVB protocol's get_block_size method.
    fn fvb_get_block_size(
        &self,
        protocol: NonNull<pi::protocols::firmware_volume_block::Protocol>,
        lba: efi::Lba,
    ) -> Result<(usize, usize), EfiError> {
        let physical_address = self.get_fvb_address(protocol).ok_or(EfiError::NotFound)?;

        // Safety: physical_address must point to a valid FV (i.e. private_data is correctly constructed and
        // its invariants - like not removing fv once installed - are upheld).
        let fv = unsafe { VolumeRef::new_from_address(physical_address)? };

        let lba: u32 = lba.try_into().map_err(|_| EfiError::InvalidParameter)?;

        let (block_size, remaining_blocks, _) = fv.lba_info(lba)?;

        Ok((block_size as usize, remaining_blocks as usize))
    }

    /// Rust implementation of the FVB protocol's read method.
    fn fvb_read(
        &self,
        protocol: NonNull<pi::protocols::firmware_volume_block::Protocol>,
        lba: efi::Lba,
        offset: usize,
        num_bytes: usize,
    ) -> Result<&'static [u8], EfiError> {
        let physical_address = self.get_fvb_address(protocol).ok_or(EfiError::NotFound)?;

        // Safety: physical_address must point to a valid FV (i.e. private_data is correctly constructed and
        // its invariants - like not removing fv once installed - are upheld).
        let fv = unsafe { VolumeRef::new_from_address(physical_address) }?;
        let Ok(lba) = lba.try_into() else {
            return Err(EfiError::InvalidParameter);
        };

        let (lba_base_addr, block_size) = fv.lba_info(lba).map(|(addr, size, _)| (addr as usize, size as usize))?;

        let mut bytes_to_read = num_bytes;
        if offset.saturating_add(bytes_to_read) > block_size {
            debug_assert!(offset.saturating_add(bytes_to_read) <= block_size); // caller should not request to read beyond the block.
            bytes_to_read = block_size.saturating_sub(offset);
        }

        let lba_start = (physical_address as usize).saturating_add(lba_base_addr).saturating_add(offset) as *mut u8;
        // Safety: lba_start is calculated from the base address of a valid FV, plus an offset and offset+num_bytes.
        // consistency of this data is guaranteed by checks on instantiation of the VolumeRef.
        // The FV data is expected to be 'static (i.e. permanently mapped) for the lifetime of the system.
        unsafe { Ok(slice::from_raw_parts(lba_start, bytes_to_read)) }
    }

    /// Rust implementation of the FV protocol's get_volume_attributes method.
    fn fv_get_volume_attributes(
        &self,
        protocol: NonNull<pi::protocols::firmware_volume::Protocol>,
    ) -> Result<fv::attributes::EfiFvAttributes, EfiError> {
        let physical_address = self.get_fv_address(protocol).ok_or(EfiError::NotFound)?;

        // Safety: physical_address must point to a valid FV (i.e. private_data is correctly constructed and
        // its invariants - like not removing fv once installed - are upheld).
        let fv = unsafe { VolumeRef::new_from_address(physical_address)? };

        Ok(fv.attributes() as fv::attributes::EfiFvAttributes)
    }

    /// Rust implementation of the FV protocol's read_file method.
    fn fv_read_file(
        &self,
        protocol: NonNull<pi::protocols::firmware_volume::Protocol>,
        name: efi::Guid,
    ) -> Result<FileRef<'_>, EfiError> {
        let physical_address = self.get_fv_address(protocol).ok_or(EfiError::NotFound)?;

        // Safety: physical_address must point to a valid FV (i.e. private_data is correctly constructed and
        // its invariants - like not removing fv once installed - are upheld).
        let fv = unsafe { VolumeRef::new_from_address(physical_address) }?;

        if (fv.attributes() & fvb::attributes::raw::fvb2::READ_STATUS) == 0 {
            return Err(EfiError::AccessDenied);
        }

        let file = match fv.files().find(|f| f.as_ref().is_ok_and(|f| f.name() == name) || f.is_err()) {
            Some(Ok(file)) => file,
            Some(Err(err)) => return Err(err.into()),
            _ => return Err(EfiError::NotFound),
        };

        Ok(file)
    }

    /// Helper function to extract a section from a FV.
    fn fv_read_section<E: SectionExtractor>(
        &self,
        protocol: NonNull<pi::protocols::firmware_volume::Protocol>,
        name: efi::Guid,
        section_type: ffs::section::EfiSectionType,
        section_instance: usize,
        section_extractor: &E,
    ) -> Result<patina_ffs::section::Section, EfiError> {
        let file = self.fv_read_file(protocol, name)?;
        let sections = file.sections_with_extractor(section_extractor)?;

        sections
            .iter()
            .filter(|s| s.section_type_raw() == section_type)
            .nth(section_instance)
            .cloned()
            .ok_or(EfiError::NotFound)
    }

    /// Rust implementation of the FV protocol's GetNextFile method.
    fn fv_get_next_file(
        &self,
        protocol: NonNull<pi::protocols::firmware_volume::Protocol>,
        file_type: fv::EfiFvFileType,
        key: usize,
    ) -> Result<(efi::Guid, fv::file::EfiFvFileAttributes, usize, fv::EfiFvFileType), EfiError> {
        let physical_address = self.get_fv_address(protocol).ok_or(EfiError::NotFound)?;

        // Safety: physical_address must point to a valid FV (i.e. private_data is correctly constructed and
        // its invariants - like not removing fv once installed - are upheld).
        let fv = unsafe { VolumeRef::new_from_address(physical_address) }?;

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

        let attributes = if (fv_attributes & fvb::attributes::raw::fvb2::MEMORY_MAPPED)
            == fvb::attributes::raw::fvb2::MEMORY_MAPPED
        {
            file.fv_attributes() | fv::file::raw::attribute::MEMORY_MAPPED
        } else {
            file.fv_attributes()
        };

        Ok((file.name(), attributes, file.data().len(), file.file_type_raw()))
    }

    fn new_fvb_protocol(parent_handle: Option<efi::Handle>) -> Box<pi::protocols::firmware_volume_block::Protocol> {
        Box::new(pi::protocols::firmware_volume_block::Protocol {
            get_attributes: Self::fvb_get_attributes_efiapi,
            set_attributes: Self::fvb_set_attributes_efiapi,
            get_physical_address: Self::fvb_get_physical_address_efiapi,
            get_block_size: Self::fvb_get_block_size_efiapi,
            read: Self::fvb_read_efiapi,
            write: Self::fvb_write_efiapi,
            erase_blocks: Self::fvb_erase_blocks_efiapi,
            parent_handle: parent_handle.unwrap_or(core::ptr::null_mut()),
        })
    }

    fn new_fv_protocol(parent_handle: Option<efi::Handle>) -> Box<pi::protocols::firmware_volume::Protocol> {
        Box::from(pi::protocols::firmware_volume::Protocol {
            get_volume_attributes: Self::fv_get_volume_attributes_efiapi,
            set_volume_attributes: Self::fv_set_volume_attributes_efiapi,
            read_file: Self::fv_read_file_efiapi,
            read_section: Self::fv_read_section_efiapi,
            write_file: Self::fv_write_file_efiapi,
            get_next_file: Self::fv_get_next_file_efiapi,
            key_size: size_of::<usize>() as u32,
            parent_handle: parent_handle.unwrap_or(core::ptr::null_mut()),
            get_info: Self::fv_get_info_efiapi,
            set_info: Self::fv_set_info_efiapi,
        })
    }

    /// A helper function to generate a firmware volume block protocol instance and install it on the provided handle.
    fn install_fvb_protocol(
        &mut self,
        handle: Option<efi::Handle>,
        parent_handle: Option<efi::Handle>,
        base_address: u64,
    ) -> Result<efi::Handle, EfiError> {
        let protocol = Self::new_fvb_protocol(parent_handle);

        let protocol_ptr = NonNull::from(&*protocol).cast::<c_void>();

        let metadata = Metadata::new_fvb(protocol, base_address);

        // save the protocol structure we're about to install in the private data.
        self.fv_metadata.insert(protocol_ptr.addr(), metadata);

        // install the protocol and return status
        core_install_protocol_interface(
            handle,
            pi::protocols::firmware_volume_block::PROTOCOL_GUID,
            protocol_ptr.as_ptr(),
        )
    }

    /// A helper function to generate a firmware volume protocol instance and install it on the provided handle.
    fn install_fv_protocol(
        &mut self,
        handle: Option<efi::Handle>,
        parent_handle: Option<efi::Handle>,
        base_address: u64,
    ) -> Result<efi::Handle, EfiError> {
        let protocol = Self::new_fv_protocol(parent_handle);

        let protocol_ptr = NonNull::from(&*protocol).cast::<c_void>();

        let metadata = Metadata::new_fv(protocol, base_address);

        // save the protocol structure we're about to install in the private data.
        self.fv_metadata.insert(protocol_ptr.addr(), metadata);

        // install the protocol and return status
        core_install_protocol_interface(handle, pi::protocols::firmware_volume::PROTOCOL_GUID, protocol_ptr.as_ptr())
    }

    /// Installs both the FVB and FV protocols for a firmware volume at the specified base address.
    ///
    /// ## Safety
    ///
    /// Caller must ensure that base_address points to a valid firmware volume.
    pub unsafe fn install_firmware_volume(
        &mut self,
        base_address: u64,
        parent_handle: Option<efi::Handle>,
    ) -> Result<efi::Handle, EfiError> {
        // Safety: Caller must meet the safety requirements of this function.
        let handle = unsafe { self.install_fv_device_path_protocol(None, base_address)? };
        self.install_fvb_protocol(Some(handle), parent_handle, base_address)?;
        self.install_fv_protocol(Some(handle), parent_handle, base_address)?;
        Ok(handle)
    }

    /// Installs any firmware volumes from FV HOBs in the hob list.
    pub(super) fn install_firmware_volumes_from_hoblist(&mut self, hob_list: &hob::HobList) -> Result<(), efi::Status> {
        let fv_hobs =
            hob_list.iter().filter_map(|h| if let hob::Hob::FirmwareVolume(fv) = h { Some(*fv) } else { None });

        for fv in fv_hobs {
            // construct a FirmwareVolume struct to verify sanity.
            // Safety: base addresses of FirmwareVolume HOBs are assumed to be valid and accessible.
            let fv_slice = unsafe { slice::from_raw_parts(fv.base_address as *const u8, fv.length as usize) };
            VolumeRef::new(fv_slice)?;
            // Safety: base addresses of FirmwareVolume HOBs are assumed to be valid and accessible.
            unsafe { self.install_firmware_volume(fv.base_address, None) }?;
        }
        Ok(())
    }

    /// Installs the device path protocol for a firmware volume at the specified base address.
    ///
    /// ## Safety
    ///
    /// Caller must ensure that base_address points to a valid firmware volume.
    unsafe fn install_fv_device_path_protocol(
        &self,
        handle: Option<efi::Handle>,
        base_address: u64,
    ) -> Result<efi::Handle, EfiError> {
        // Safety: caller must ensure that base_address is valid.
        let fv = unsafe { VolumeRef::new_from_address(base_address) }?;

        let device_path_ptr = match fv.fv_name() {
            Some(fv_name) => {
                // Construct FvPiWgDevicePath
                let device_path = FvPiWgDevicePath::new_fv(fv_name);
                Box::into_raw(Box::new(device_path)) as *mut c_void
            }
            None => {
                // Construct FvMemMapDevicePath
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
                        memory_type: MEMORY_MAPPED_IO,
                        starting_address: base_address,
                        ending_address: base_address.saturating_add(fv.size()),
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
}

// FV / FVB EFIAPI compliant protocol method implementations.
#[coverage(off)]
impl<P: PlatformInfo> FvProtocolData<P> {
    /// EFIAPI compliant FVB protocol GetAttributes method.
    extern "efiapi" fn fvb_get_attributes_efiapi(
        this: *mut pi::protocols::firmware_volume_block::Protocol,
        attributes: *mut fvb::attributes::EfiFvbAttributes2,
    ) -> efi::Status {
        if attributes.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        let Some(protocol) = NonNull::new(this) else {
            return efi::Status::INVALID_PARAMETER;
        };

        match Self::instance().fvb_get_attributes(protocol) {
            Err(err) => return err.into(),
            // Safety: caller must provide a valid pointer to receive the attributes. It is null-checked above.
            Ok(fvb_attributes) => unsafe { attributes.write_unaligned(fvb_attributes) },
        };

        efi::Status::SUCCESS
    }

    /// EFIAPI compliant FVB protocol SetAttributes method.
    extern "efiapi" fn fvb_set_attributes_efiapi(
        _this: *mut pi::protocols::firmware_volume_block::Protocol,
        _attributes: *mut fvb::attributes::EfiFvbAttributes2,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    /// EFIAPI compliant FVB protocol GetPhysicalAddress method.
    extern "efiapi" fn fvb_get_physical_address_efiapi(
        this: *mut pi::protocols::firmware_volume_block::Protocol,
        address: *mut efi::PhysicalAddress,
    ) -> efi::Status {
        if address.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        let Some(protocol) = NonNull::new(this) else {
            return efi::Status::INVALID_PARAMETER;
        };

        match Self::instance().fvb_get_physical_address(protocol) {
            Err(err) => return err.into(),
            // Safety: caller must provide a valid pointer to receive the address. It is null-checked above.
            Ok(physical_address) => unsafe { address.write_unaligned(physical_address) },
        };

        efi::Status::SUCCESS
    }

    /// EFIAPI compliant FVB protocol GetBlockSize method.
    extern "efiapi" fn fvb_get_block_size_efiapi(
        this: *mut pi::protocols::firmware_volume_block::Protocol,
        lba: efi::Lba,
        block_size: *mut usize,
        number_of_blocks: *mut usize,
    ) -> efi::Status {
        if block_size.is_null() || number_of_blocks.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        let Some(protocol) = NonNull::new(this) else {
            return efi::Status::INVALID_PARAMETER;
        };

        let (size, remaining_blocks) = match Self::instance().fvb_get_block_size(protocol, lba) {
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

    /// EFIAPI compliant FVB protocol Read method.
    extern "efiapi" fn fvb_read_efiapi(
        this: *mut pi::protocols::firmware_volume_block::Protocol,
        lba: efi::Lba,
        offset: usize,
        num_bytes: *mut usize,
        buffer: *mut core::ffi::c_void,
    ) -> efi::Status {
        if num_bytes.is_null() || buffer.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        let Some(protocol) = NonNull::new(this) else {
            return efi::Status::INVALID_PARAMETER;
        };

        // Safety: caller must provide valid pointers for num_bytes and buffer. They are null-checked above.
        let bytes_to_read = unsafe { *num_bytes };

        let data = match Self::instance().fvb_read(protocol, lba, offset, bytes_to_read) {
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

    /// EFIAPI compliant FVB protocol Write method.
    extern "efiapi" fn fvb_write_efiapi(
        _this: *mut pi::protocols::firmware_volume_block::Protocol,
        _lba: efi::Lba,
        _offset: usize,
        _num_bytes: *mut usize,
        _buffer: *mut core::ffi::c_void,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    /// EFIAPI compliant FVB protocol EraseBlocks method.
    extern "efiapi" fn fvb_erase_blocks_efiapi(
        _this: *mut pi::protocols::firmware_volume_block::Protocol,
        //... TODO: this should be variadic; however, variadic and eficall don't mix well presently.
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    /// EFIAPI compliant FV protocol GetVolumeAttributes method.
    extern "efiapi" fn fv_get_volume_attributes_efiapi(
        this: *const pi::protocols::firmware_volume::Protocol,
        fv_attributes: *mut fv::attributes::EfiFvAttributes,
    ) -> efi::Status {
        if fv_attributes.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        let Some(protocol) = NonNull::new(this as *mut pi::protocols::firmware_volume::Protocol) else {
            return efi::Status::INVALID_PARAMETER;
        };

        let fv_attributes_data = match Self::instance().fv_get_volume_attributes(protocol) {
            Err(err) => return err.into(),
            Ok(attrs) => attrs,
        };

        // Safety: caller must provide a valid pointer to receive the attributes. It is null-checked above.
        unsafe { fv_attributes.write_unaligned(fv_attributes_data) };

        efi::Status::SUCCESS
    }

    /// EFIAPI compliant FV protocol SetVolumeAttributes method.
    extern "efiapi" fn fv_set_volume_attributes_efiapi(
        _this: *const pi::protocols::firmware_volume::Protocol,
        _fv_attributes: *mut fv::attributes::EfiFvAttributes,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    /// EFIAPI compliant FV protocol ReadFile method.
    extern "efiapi" fn fv_read_file_efiapi(
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

        let Some(protocol) = NonNull::new(this as *mut pi::protocols::firmware_volume::Protocol) else {
            return efi::Status::INVALID_PARAMETER;
        };

        // Safety: caller must provide valid pointers for buffer_size and name_guid. They are null-checked above.
        let local_buffer_size = unsafe { buffer_size.read_unaligned() };
        // Safety: caller must provide valid pointers for buffer_size and name_guid. They are null-checked above.
        let name = unsafe { name_guid.read_unaligned() };

        let this = Self::instance();
        let file = match this.fv_read_file(protocol, name) {
            Err(err) => return err.into(),
            Ok(file) => file,
        };

        // update file metadata output pointers (buffer_size is written later).
        // Safety: caller must provide valid pointers for found_type and file_attributes. They are null-checked above.
        unsafe {
            found_type.write_unaligned(file.file_type_raw());
            file_attributes.write_unaligned(file.fv_attributes());
            //TODO: Authentication status is not yet supported.
            buffer_size.write_unaligned(file.content().len());
        }

        if buffer.is_null() {
            // The caller just wants file meta data, no need to read file data.
            // Safety: The caller must provide a valid pointer for buffer_size. It is null-checked above.
            unsafe {
                buffer_size.write_unaligned(file.content().len());
            }
            return efi::Status::SUCCESS;
        }

        // Safety: caller must provide a valid pointer for buffer. It is null-checked above.
        let mut local_buffer_ptr = unsafe { buffer.read_unaligned() };

        // Determine the size to copy and the return status. For compatibility with existing callers to this function,
        // C code behavior (`FvReadFile()`) is retained that does the following based on inputs:
        //
        // 1. If the buffer pointer provided  is null, attempt to allocate a buffer of appropriate size via allocate_pool,
        //    set the copy size to the file size, write full file size to buffer_size output, and return SUCCESS.
        // 2. If the buffer pointer is non-null, but the provided buffer size is smaller than the file size,
        //    set the copy size to the provided buffer size, write this truncated size to buffer_size output,
        //    perform the truncated copy into the provided buffer, and return WARN_BUFFER_TOO_SMALL.
        // 3. If the buffer pointer is non-null, and the provided buffer size is sufficient to hold the file data,
        //    set the copy size to the file size, write full file size to buffer_size output,
        //    perform the copy into the provided buffer, and return SUCCESS.
        let (copy_size, status) = if local_buffer_ptr.is_null() {
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
            (file.content().len(), efi::Status::SUCCESS)
        } else if file.content().len() > local_buffer_size {
            // The buffer is too small, a truncated copy should be performed
            (local_buffer_size, efi::Status::WARN_BUFFER_TOO_SMALL)
        } else {
            (file.content().len(), efi::Status::SUCCESS)
        };

        // Safety: The caller must provide a valid pointer for buffer_size. It is null-checked above.
        unsafe {
            buffer_size.write_unaligned(copy_size);
        }

        // convert pointer+size into a slice and copy the file data (truncated if necessary).
        // Safety: local_buffer_ptr is either provided by the caller (and null-checked above), or allocated via allocate pool
        // and is of sufficient size to contain the data.
        let out_buffer = unsafe { slice::from_raw_parts_mut(local_buffer_ptr as *mut u8, copy_size) };
        out_buffer.copy_from_slice(&file.content()[..copy_size]);

        status
    }

    /// EFIAPI compliant FV protocol ReadSection method.
    extern "efiapi" fn fv_read_section_efiapi(
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

        let Some(protocol) = NonNull::new(this as *mut pi::protocols::firmware_volume::Protocol) else {
            return efi::Status::INVALID_PARAMETER;
        };

        // Safety: caller must provide valid pointer for name_guid. It is null-checked above.
        let name = unsafe { name_guid.read_unaligned() };

        let extractor = &Core::<P>::instance().pi_dispatcher.section_extractor;
        let section = match Self::instance().fv_read_section(protocol, name, section_type, section_instance, extractor)
        {
            Err(err) => return err.into(),
            Ok(section) => section,
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

        // We only want to copy the min(section_data.len(), local_buffer_size) bytes. If the local_buffer_size is
        // larger than the section_data, we could inadvertently copy beyond the section_data slice.
        let copy_size = core::cmp::min(section_data.len(), local_buffer_size);

        // Safety: local_buffer_ptr is either provided by the caller (and null-checked above), or allocated via allocate pool and
        // is of sufficient size to contain the data. We copy the minimum of section_data.len() and local_buffer_size to ensure we do not
        // copy beyond the bounds of either buffer.
        let dest_buffer = unsafe { slice::from_raw_parts_mut(local_buffer_ptr as *mut u8, copy_size) };
        dest_buffer.copy_from_slice(&section_data[0..dest_buffer.len()]);

        //TODO: authentication status not yet supported.

        if dest_buffer.len() < section_data.len() { efi::Status::WARN_BUFFER_TOO_SMALL } else { efi::Status::SUCCESS }
    }

    /// EFIAPI compliant FV protocol WriteFile method.
    extern "efiapi" fn fv_write_file_efiapi(
        _this: *const pi::protocols::firmware_volume::Protocol,
        _number_of_files: u32,
        _write_policy: pi::protocols::firmware_volume::EfiFvWritePolicy,
        _file_data: *mut pi::protocols::firmware_volume::EfiFvWriteFileData,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    /// EFIAPI compliant FV protocol GetNextFile method.
    extern "efiapi" fn fv_get_next_file_efiapi(
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

        let Some(protocol) = NonNull::new(this as *mut pi::protocols::firmware_volume::Protocol) else {
            return efi::Status::INVALID_PARAMETER;
        };

        // Safety: caller must provide valid pointers for key and file_type. They are null-checked above.
        let local_key = unsafe { (key as *mut usize).read_unaligned() };
        // Safety: caller must provide valid pointers for key and file_type. They are null-checked above.
        let local_file_type = unsafe { file_type.read_unaligned() };

        if local_file_type >= ffs::file::raw::r#type::FFS_MIN {
            return efi::Status::NOT_FOUND;
        }

        let (file_name, fv_attributes, file_size, found_file_type) =
            match Self::instance().fv_get_next_file(protocol, local_file_type, local_key) {
                Err(err) => return err.into(),
                Ok((name, attrs, size, file_type)) => (name, attrs, size, file_type),
            };

        // Safety: caller must provide valid pointers for key, file_type, name_guid, attributes, and size. They are null-checked above.
        unsafe {
            (key as *mut usize).write_unaligned(local_key.saturating_add(1));
            name_guid.write_unaligned(file_name);
            if (fv_attributes & fvb::attributes::raw::fvb2::MEMORY_MAPPED) == fvb::attributes::raw::fvb2::MEMORY_MAPPED
            {
                attributes.write_unaligned(fv_attributes | fv::file::raw::attribute::MEMORY_MAPPED);
            } else {
                attributes.write_unaligned(fv_attributes);
            }
            size.write_unaligned(file_size);
            file_type.write_unaligned(found_file_type);
        }

        efi::Status::SUCCESS
    }

    /// EFIAPI compliant FV protocol GetInfo method.
    extern "efiapi" fn fv_get_info_efiapi(
        _this: *const pi::protocols::firmware_volume::Protocol,
        _information_type: *const efi::Guid,
        _buffer_size: *mut usize,
        _buffer: *mut c_void,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    /// EFIAPI compliant FV protocol SetInfo method.
    extern "efiapi" fn fv_set_info_efiapi(
        _this: *const pi::protocols::firmware_volume::Protocol,
        _information_type: *const efi::Guid,
        _buffer_size: usize,
        _buffer: *const c_void,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }
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

/// Returns a device path for the file specified by the given fv_handle and filename GUID.
pub fn device_path_bytes_for_fv_file(fv_handle: efi::Handle, file_name: efi::Guid) -> Result<Box<[u8]>, efi::Status> {
    let fv_device_path = PROTOCOL_DB.get_interface_for_handle(fv_handle, efi::protocols::device_path::PROTOCOL_GUID)?;
    let file_node = &FvPiWgDevicePath::new_file(file_name);
    concat_device_path_to_boxed_slice(
        fv_device_path as *mut _ as *const efi::protocols::device_path::Protocol,
        file_node as *const _ as *const efi::protocols::device_path::Protocol,
    )
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::{MockComponentInfo, MockCpuInfo, MockMemoryInfo, test_support};
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

    struct MockPlatformInfo;

    impl PlatformInfo for MockPlatformInfo {
        type MemoryInfo = MockMemoryInfo;
        type CpuInfo = MockCpuInfo;
        type ComponentInfo = MockComponentInfo;
        type Extractor = CompositeSectionExtractor;
    }
    type MockCore = Core<MockPlatformInfo>;
    type MockProtocolData = FvProtocolData<MockPlatformInfo>;

    #[test]
    fn test_fv_init_core() {
        test_support::with_global_lock(|| {
            // Safety: global lock ensures exclusive access to the private data.
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

            static CORE: MockCore = MockCore::new(CompositeSectionExtractor::new());
            CORE.override_instance();
            CORE.pi_dispatcher.install_firmware_volumes_from_hoblist(&hoblist).unwrap();
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

            static CORE: MockCore = MockCore::new(CompositeSectionExtractor::new());
            CORE.override_instance();
            // Safety: fv was leaked above to ensure that the buffer is valid and immutable for the rest of the test.
            let _handle =
                unsafe { CORE.pi_dispatcher.fv_data.lock().install_fv_device_path_protocol(None, base_address) };

            /* Start with Clearing Private Global Data, Please note that this is to be done only once
             * for test_fv_functionality.
             * In case other functions/modules are written, clear the private global data again.
             */
            // Safety: global lock ensures exclusive access to the private data.
            CORE.pi_dispatcher.fv_data.lock().fv_metadata.clear();
            assert!(CORE.pi_dispatcher.fv_data.lock().fv_metadata.is_empty());

            /* Create Firmware Interface, this will be used by the whole test module */
            let fv_interface = MockProtocolData::new_fv_protocol(parent_handle);

            let fv_ptr = NonNull::from(&*fv_interface).cast::<c_void>();

            let metadata = Metadata::new_fv(fv_interface, base_address);
            // save the protocol structure we're about to install in the private data.
            CORE.pi_dispatcher.fv_data.lock().fv_metadata.insert(fv_ptr.addr(), metadata);

            let fv_ptr1 = fv_ptr.cast::<pi::protocols::firmware_volume::Protocol>().as_ptr();

            /* Build Firmware Volume Block Interface*/
            let fvb_interface = MockProtocolData::new_fvb_protocol(parent_handle);

            let fvb_ptr = NonNull::from(&*fvb_interface).cast::<c_void>();
            let fvb_ptr_mut_prot = fvb_ptr.cast::<pi::protocols::firmware_volume_block::Protocol>().as_ptr();

            /* Build Private Data */
            let metadata = Metadata::new_fvb(fvb_interface, base_address);
            // save the protocol structure we're about to install in the private data.
            CORE.pi_dispatcher.fv_data.lock().fv_metadata.insert(fvb_ptr.addr(), metadata);

            //let fv_attributes3: *mut fw_fs::EfiFvAttributes = &mut fv_att;

            /* Instance 2 - Create a FV  interface with Bad physical address to handle Error cases. */
            let mut fv_interface3 = MockProtocolData::new_fv_protocol(parent_handle);

            let fv_ptr3 = NonNull::from(&*fv_interface3).cast::<c_void>();
            let fv_ptr3_const = fv_ptr3.cast::<pi::protocols::firmware_volume::Protocol>().as_ptr();

            /* Corrupt the base address to cover error conditions  */
            let base_no2: u64 = fv_interface3.as_mut() as *mut _ as u64 + 0x1000;
            let metadata2 = Metadata::new_fv(fv_interface3, base_no2);
            //save the protocol structure we're about to install in the private data.
            CORE.pi_dispatcher.fv_data.lock().fv_metadata.insert(fv_ptr3.addr(), metadata2);

            /* Create an interface with No physical address and no private data - cover Error Conditions */
            let fv_interface_no_data = MockProtocolData::new_fv_protocol(None);

            let fv_ptr_no_data = fv_interface_no_data.as_ref() as *const pi::protocols::firmware_volume::Protocol;

            /* Create a Firmware Volume Block Interface with Invalid Physical Address */
            let fvb_intf_invalid = MockProtocolData::new_fvb_protocol(parent_handle);
            let fvb_intf_invalid_void = NonNull::from(&*fvb_intf_invalid).cast::<c_void>();
            let fvb_intf_invalid_mutpro =
                fvb_intf_invalid_void.cast::<pi::protocols::firmware_volume_block::Protocol>().as_ptr();
            let base_no: u64 = fv.as_ptr() as u64 + 0x1000;

            let private_data4 = Metadata::new_fvb(fvb_intf_invalid, base_no);
            // save the protocol structure we're about to install in the private data.
            CORE.pi_dispatcher.fv_data.lock().fv_metadata.insert(fvb_intf_invalid_void.addr(), private_data4);

            /* Create a Firmware Volume Block Interface without Physical address populated  */
            let mut fvb_intf_data_n = Box::from(pi::protocols::firmware_volume_block::Protocol {
                get_attributes: MockProtocolData::fvb_get_attributes_efiapi,
                set_attributes: MockProtocolData::fvb_set_attributes_efiapi,
                get_physical_address: MockProtocolData::fvb_get_physical_address_efiapi,
                get_block_size: MockProtocolData::fvb_get_block_size_efiapi,
                read: MockProtocolData::fvb_read_efiapi,
                write: MockProtocolData::fvb_write_efiapi,
                erase_blocks: MockProtocolData::fvb_erase_blocks_efiapi,
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
                    MockProtocolData::fv_set_info_efiapi(ptr::null(), ptr::null(), BUFFER_SIZE_EMPTY, ptr::null());
                };

                let fv_test_get_info = || {
                    MockProtocolData::fv_get_info_efiapi(ptr::null(), ptr::null(), ptr::null_mut(), ptr::null_mut());
                };

                let fv_test_set_volume_attributes = || {
                    /* Cover the NULL Case */
                    MockProtocolData::fv_set_volume_attributes_efiapi(ptr::null(), fv_attributes);

                    /* Non Null Case*/
                };

                let fv_test_get_volume_attributes = || {
                    /* Cover the NULL Case, User Passing Invalid Parameter Case  */
                    MockProtocolData::fv_get_volume_attributes_efiapi(fv_ptr1, std::ptr::null_mut());

                    /* Handle bad firmware volume data - return efi::Status::NOT_FOUND */
                    MockProtocolData::fv_get_volume_attributes_efiapi(fv_ptr_no_data, fv_attributes);

                    /* Handle Invalid Physical address case */
                    MockProtocolData::fv_get_volume_attributes_efiapi(fv_ptr3_const, fv_attributes);

                    /* Non Null Case, success case */
                    MockProtocolData::fv_get_volume_attributes_efiapi(fv_ptr1, fv_attributes);
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
                    MockProtocolData::fvb_read_efiapi(
                        fvb_ptr_mut_prot,
                        LBA,
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    );
                    MockProtocolData::fvb_read_efiapi(fvb_ptr_mut_prot, LBA, 0, buffer_valid_size3, buffer_valid3);
                    MockProtocolData::fvb_read_efiapi(
                        fvb_ptr_mut_prot,
                        0xfffffffff,
                        0,
                        buffer_valid_size3,
                        buffer_valid3,
                    );
                    MockProtocolData::fvb_read_efiapi(
                        fvb_intf_invalid_mutpro,
                        LBA,
                        0,
                        buffer_valid_size3,
                        buffer_valid3,
                    );
                    MockProtocolData::fvb_read_efiapi(fvb_ptr_mut_prot, u64::MAX, 0, buffer_valid_size3, buffer_valid3);
                    MockProtocolData::fvb_read_efiapi(
                        fvb_ptr_mut_prot,
                        0x22299222,
                        0x999999,
                        buffer_valid_size3,
                        buffer_valid3,
                    );
                    MockProtocolData::fvb_read_efiapi(fvb_intf_data_n_mut, LBA, 0, buffer_valid_size3, buffer_valid3);

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
                    MockProtocolData::fvb_get_block_size_efiapi(
                        fvb_ptr_mut_prot,
                        LBA,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    );
                    MockProtocolData::fvb_get_block_size_efiapi(
                        fvb_ptr_mut_prot,
                        LBA,
                        buffer_valid_size3,
                        buffer_valid_size3,
                    );
                    MockProtocolData::fvb_get_block_size_efiapi(
                        fvb_intf_invalid_mutpro,
                        LBA,
                        buffer_valid_size3,
                        buffer_valid_size3,
                    );
                    MockProtocolData::fvb_get_block_size_efiapi(
                        fvb_intf_data_n_mut,
                        LBA,
                        buffer_valid_size3,
                        buffer_valid_size3,
                    );
                    MockProtocolData::fvb_get_block_size_efiapi(
                        fvb_ptr_mut_prot,
                        u64::MAX,
                        buffer_valid_size3,
                        buffer_valid_size3,
                    );
                    MockProtocolData::fvb_get_block_size_efiapi(
                        fvb_ptr_mut_prot,
                        222222,
                        buffer_size_random_ref,
                        num_buffer_empty_ref,
                    );
                    /* Free Memory */
                    dealloc(buffer_valid3 as *mut u8, layout3);
                };

                let fvb_test_erase_block = || {
                    MockProtocolData::fvb_erase_blocks_efiapi(fvb_ptr_mut_prot);
                };

                let fvb_test_get_physical_address = || {
                    /* Handling Not Found Case */
                    let mut p_address: efi::PhysicalAddress = 0x12345;

                    MockProtocolData::fvb_get_physical_address_efiapi(fvb_intf_data_n_mut, &mut p_address as *mut u64);
                    MockProtocolData::fvb_get_physical_address_efiapi(
                        fvb_intf_invalid_mutpro,
                        &mut p_address as *mut u64,
                    );
                    MockProtocolData::fvb_get_physical_address_efiapi(fvb_ptr_mut_prot, &mut p_address as *mut u64);
                    MockProtocolData::fvb_get_physical_address_efiapi(fvb_ptr_mut_prot, std::ptr::null_mut());
                };
                let fvb_test_write_file = || {
                    let number_of_files: u32 = 0;
                    let write_policy: pi::protocols::firmware_volume::EfiFvWritePolicy = 0;
                    MockProtocolData::fv_write_file_efiapi(
                        fv_ptr1,
                        number_of_files,
                        write_policy,
                        std::ptr::null_mut(),
                    );
                };

                let fvb_test_set_attributes = || {
                    MockProtocolData::fvb_set_attributes_efiapi(fvb_ptr_mut_prot, std::ptr::null_mut());
                };

                let fvb_test_write = || {
                    let mut len3 = 1000;
                    let buffer_valid_size3: *mut usize = &mut len3;
                    let layout3 = Layout::from_size_align(1001, 8).unwrap();
                    let buffer_valid3 = alloc(layout3) as *mut c_void;

                    if buffer_valid3.is_null() {
                        panic!("Memory allocation failed!");
                    }

                    MockProtocolData::fvb_write_efiapi(
                        fvb_ptr_mut_prot,
                        LBA,
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    );
                    MockProtocolData::fvb_write_efiapi(fvb_ptr_mut_prot, LBA, 0, buffer_valid_size3, buffer_valid3);
                    MockProtocolData::fvb_write_efiapi(
                        fvb_intf_invalid_mutpro,
                        LBA,
                        0,
                        buffer_valid_size3,
                        buffer_valid3,
                    );
                    MockProtocolData::fvb_write_efiapi(fvb_intf_data_n_mut, LBA, 0, buffer_valid_size3, buffer_valid3);
                    /* Free Memory */
                    dealloc(buffer_valid3 as *mut u8, layout3);
                };

                let fvb_test_get_attributes = || {
                    let mut fvb_attributes: fvb::attributes::EfiFvbAttributes2 = 0x123456;
                    let fvb_attributes_ref: *mut fvb::attributes::EfiFvbAttributes2 = &mut fvb_attributes;

                    MockProtocolData::fvb_get_attributes_efiapi(fvb_ptr_mut_prot, std::ptr::null_mut());
                    MockProtocolData::fvb_get_attributes_efiapi(fvb_ptr_mut_prot, fvb_attributes_ref);
                    MockProtocolData::fvb_get_attributes_efiapi(fvb_intf_invalid_mutpro, fvb_attributes_ref);
                    MockProtocolData::fvb_get_attributes_efiapi(fvb_intf_data_n_mut, fvb_attributes_ref);
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
                    MockProtocolData::fv_get_next_file_efiapi(
                        ptr::null(),
                        std::ptr::null_mut(),
                        file_type_read_ref,
                        std::ptr::null_mut(),
                        file_attributes,
                        buffer_valid_size3,
                    );
                    MockProtocolData::fv_get_next_file_efiapi(
                        ptr::null(),
                        buffer_valid3,
                        file_type_read_ref,
                        n_guid_ref_mut,
                        file_attributes,
                        buffer_valid_size3,
                    );
                    MockProtocolData::fv_get_next_file_efiapi(
                        fv_ptr1,
                        buffer_valid3,
                        file_type_read_ref,
                        n_guid_ref_mut,
                        file_attributes,
                        buffer_valid_size3,
                    );
                    MockProtocolData::fv_get_next_file_efiapi(
                        fv_ptr3_const,
                        buffer_valid3,
                        file_type_read_ref,
                        n_guid_ref_mut,
                        file_attributes,
                        buffer_valid_size3,
                    );
                    MockProtocolData::fv_get_next_file_efiapi(
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

                    MockProtocolData::fv_get_next_file_efiapi(
                        fv_ptr1,
                        buffer_valid3,
                        file_type_read_ref1,
                        n_guid_ref_mut,
                        file_attributes,
                        buffer_valid_size3,
                    );
                    /* Null BUffer Case*/
                    MockProtocolData::fv_get_next_file_efiapi(
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
                    MockProtocolData::fv_read_section_efiapi(
                        ptr::null(),
                        ptr::null(),
                        SECTION_TYPE,
                        SECTION_INSTANCE,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    );

                    MockProtocolData::fv_read_section_efiapi(
                        fv_ptr1,
                        guid_ref_invalid_ref,
                        6,
                        10,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        auth_valid_p,
                    );

                    /* Valid guid case - panicing, debug this further, for now comment*/
                    MockProtocolData::fv_read_section_efiapi(
                        fv_ptr1,
                        guid_valid_ref,
                        6,
                        10,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        auth_valid_p,
                    );

                    MockProtocolData::fv_read_section_efiapi(
                        fv_ptr1,
                        name_guid2,
                        6,
                        10,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        auth_valid_p,
                    );

                    /* Handle Invalid Physical address case */
                    MockProtocolData::fv_read_section_efiapi(
                        fv_ptr3_const,
                        guid_ref_invalid_ref,
                        1,
                        1,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        auth_valid_p,
                    );

                    /* Handle bad firmware volume data - return efi::Status::NOT_FOUND */
                    MockProtocolData::fv_read_section_efiapi(
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

                    MockProtocolData::fv_read_file_efiapi(
                        ptr::null(),
                        ptr::null(),
                        &mut buffer_valid3 as *mut *mut c_void,
                        std::ptr::null_mut(),
                        found_type_ref,
                        file_attributes,
                        std::ptr::null_mut(),
                    );

                    MockProtocolData::fv_read_file_efiapi(
                        fv_ptr1,
                        guid_ref_invalid_ref,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        found_type_ref,
                        file_attributes,
                        auth_valid_p,
                    );
                    MockProtocolData::fv_read_file_efiapi(
                        fv_ptr1,
                        guid_valid_ref,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        found_type_ref,
                        file_attributes,
                        auth_valid_p,
                    );
                    MockProtocolData::fv_read_file_efiapi(
                        fv_ptr3_const,
                        guid_valid_ref,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        found_type_ref,
                        file_attributes,
                        auth_valid_p,
                    );
                    MockProtocolData::fv_read_file_efiapi(
                        fv_ptr_no_data,
                        guid_valid_ref,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_valid_size3,
                        found_type_ref,
                        file_attributes,
                        auth_valid_p,
                    );
                    MockProtocolData::fv_read_file_efiapi(
                        fv_ptr1,
                        guid_valid_ref,
                        std::ptr::null_mut(),
                        buffer_valid_size3,
                        found_type_ref,
                        file_attributes,
                        auth_valid_p,
                    );
                    let mut buffer_size_zero = 0usize;
                    let buffer_size_zero_ptr: *mut usize = &mut buffer_size_zero;
                    let status = MockProtocolData::fv_read_file_efiapi(
                        fv_ptr1,
                        guid_valid_ref,
                        &mut buffer_valid3 as *mut *mut c_void,
                        buffer_size_zero_ptr,
                        found_type_ref,
                        file_attributes,
                        auth_valid_p,
                    );
                    assert_eq!(status, efi::Status::WARN_BUFFER_TOO_SMALL);
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
            static CORE: MockCore = MockCore::new(CompositeSectionExtractor::new());
            CORE.override_instance();

            let fv_interface = MockProtocolData::new_fv_protocol(parent_handle);

            let fv_ptr = NonNull::from(&*fv_interface);

            let private_data = Metadata::new_fv(fv_interface, base_address);
            // save the protocol structure we're about to install in the private data.
            CORE.pi_dispatcher.fv_data.lock().fv_metadata.insert(fv_ptr.addr(), private_data);
            let fv_ptr1: *const pi::protocols::firmware_volume::Protocol = fv_ptr.as_ptr();

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

                MockProtocolData::fv_read_section_efiapi(
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

    #[test]
    fn test_fv_read_file_truncated_copy() {
        test_support::with_global_lock(|| {
            // This test verifies that when a buffer is too small, the function:
            // 1. Returns WARN_BUFFER_TOO_SMALL status
            // 2. Copies truncated data (up to buffer_size bytes)
            // 3. Updates buffer_size to reflect the amount actually copied
            //
            // This matches the C implementation behavior in FwVolRead.c:
            //   if (FileSize > InputBufferSize) {
            //     Status = EFI_WARN_BUFFER_TOO_SMALL;
            //     FileSize = InputBufferSize;
            //   }
            //   CopyMem (*Buffer, FileHeader, FileSize);

            let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
            let mut fv: Vec<u8> = Vec::new();
            file.read_to_end(&mut fv).expect("failed to read test file");

            let fv = fv.leak();
            let base_address: u64 = fv.as_ptr() as u64;
            let parent_handle: Option<efi::Handle> = None;

            static CORE: MockCore = MockCore::new(CompositeSectionExtractor::new());
            CORE.override_instance();

            let fv_interface = MockProtocolData::new_fv_protocol(parent_handle);

            let fv_ptr = NonNull::from(&*fv_interface);

            let metadata = Metadata::new_fv(fv_interface, base_address);
            CORE.pi_dispatcher.fv_data.lock().fv_metadata.insert(fv_ptr.addr(), metadata);

            let fv_ptr1: *const pi::protocols::firmware_volume::Protocol = fv_ptr.as_ptr();

            // Safety: the following test code must uphold the safety expectations of the unsafe
            // functions it calls. This unsafe section encompasses all of the logic for the remaining
            // test since this is test code.
            unsafe {
                // Use a known file GUID from the test FV
                let mut guid: efi::Guid = efi::Guid::from_fields(
                    0x1fa1f39e,
                    0xfeff,
                    0x4aae,
                    0xbd,
                    0x7b,
                    &[0x38, 0xa0, 0x70, 0xa3, 0xb6, 0x09],
                );
                let name_guid: *mut efi::Guid = &mut guid;

                // First, get the actual file size by passing null buffer
                let mut actual_file_size: usize = 0;
                let mut found_type: u8 = 0;
                let mut file_attributes: u32 = 0;
                let mut auth_status: u32 = 0;

                let status = MockProtocolData::fv_read_file_efiapi(
                    fv_ptr1,
                    name_guid,
                    std::ptr::null_mut(),
                    &mut actual_file_size,
                    &mut found_type,
                    &mut file_attributes,
                    &mut auth_status,
                );
                assert_eq!(status, efi::Status::SUCCESS);
                assert!(actual_file_size > 0, "File size should be greater than 0");

                // Test a truncated copy with a buffer that's smaller than the file
                let truncated_size = actual_file_size / 2; // Use half the file size
                let layout = Layout::from_size_align(truncated_size, 8).unwrap();
                let mut buffer = alloc(layout) as *mut c_void;
                assert!(!buffer.is_null(), "Memory allocation failed!");

                // Fill the buffer with a pattern to check the truncated copy
                let buffer_slice = slice::from_raw_parts_mut(buffer as *mut u8, truncated_size);
                buffer_slice.fill(0xFE);

                let mut buffer_size = truncated_size;
                let status = MockProtocolData::fv_read_file_efiapi(
                    fv_ptr1,
                    name_guid,
                    &mut buffer as *mut *mut c_void,
                    &mut buffer_size,
                    &mut found_type,
                    &mut file_attributes,
                    &mut auth_status,
                );

                // 1. Status should be WARN_BUFFER_TOO_SMALL
                assert_eq!(
                    status,
                    efi::Status::WARN_BUFFER_TOO_SMALL,
                    "Expected WARN_BUFFER_TOO_SMALL when buffer is too small"
                );

                // 2. buffer_size should be updated to the truncated size
                assert_eq!(
                    buffer_size, truncated_size,
                    "buffer_size should be updated to truncated size (what was actually copied)"
                );

                // 3. Verify data was actually copied (not all 0xFE anymore)
                let copied_data = slice::from_raw_parts(buffer as *const u8, truncated_size);
                let all_ff = copied_data.iter().all(|&b| b == 0xFE);
                assert!(!all_ff, "Data should have been copied to buffer (not all 0xFE)");

                dealloc(buffer as *mut u8, layout);

                // Additionally, verify a 0-byte buffer works as expected
                let zero_size = 0;
                let layout_zero = Layout::from_size_align(64, 8).unwrap();
                let mut buffer_zero = alloc(layout_zero) as *mut c_void;
                assert!(!buffer_zero.is_null(), "Memory allocation failed!");

                let mut buffer_size_zero = zero_size;
                let status_zero = MockProtocolData::fv_read_file_efiapi(
                    fv_ptr1,
                    name_guid,
                    &mut buffer_zero as *mut *mut c_void,
                    &mut buffer_size_zero,
                    &mut found_type,
                    &mut file_attributes,
                    &mut auth_status,
                );

                assert_eq!(
                    status_zero,
                    efi::Status::WARN_BUFFER_TOO_SMALL,
                    "Expected WARN_BUFFER_TOO_SMALL with a 0-byte buffer"
                );
                assert_eq!(buffer_size_zero, 0, "buffer_size should remain 0 when input is 0");

                dealloc(buffer_zero as *mut u8, layout_zero);
            }
        })
        .unwrap();
    }

    #[test]
    fn test_fv_read_section_limits_read() {
        // This test verifies that when the buffer size is larger than the section size, only the section
        // size is copied, and the function returns SUCCESS.
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        let fv = fv.leak();
        let base_address: u64 = fv.as_ptr() as u64;
        let parent_handle: Option<efi::Handle> = None;

        test_support::with_global_lock(|| {
            static CORE: MockCore = MockCore::new(CompositeSectionExtractor::new());
            CORE.override_instance();
            unsafe { test_support::init_test_gcd(None) };

            let fv_interface = MockProtocolData::new_fv_protocol(parent_handle);
            let fv_ptr = NonNull::from(&*fv_interface);

            let metadata = Metadata::new_fv(fv_interface, base_address);
            CORE.pi_dispatcher.fv_data.lock().fv_metadata.insert(fv_ptr.addr(), metadata);

            let fv_ptr1 = fv_ptr.as_ptr();

            // Safety: the following test code must uphold the safety expectations of the unsafe
            // functions it calls. This unsafe section encompasses all of the logic for the remaining
            // test since this is test code.
            unsafe {
                // Use a known file GUID from the test FV
                let mut guid: efi::Guid = efi::Guid::from_fields(
                    0x1fa1f39e,
                    0xfeff,
                    0x4aae,
                    0xbd,
                    0x7b,
                    &[0x38, 0xa0, 0x70, 0xa3, 0xb6, 0x09],
                );
                let name_guid: *mut efi::Guid = &mut guid;

                // First get the actual file size by passing null buffer
                let mut actual_section_size: usize = 0;
                let mut auth_status: u32 = 0;

                let status = MockProtocolData::fv_read_section_efiapi(
                    fv_ptr1,
                    name_guid,
                    19,
                    0,
                    &mut std::ptr::null_mut() as *mut *mut c_void,
                    &mut actual_section_size,
                    &mut auth_status,
                );

                assert_eq!(status, efi::Status::SUCCESS);
                assert!(actual_section_size > 0, "Section size should be greater than 0");
                // Test a buffer larger than the file section size
                let larger_size = actual_section_size + 512; // 512 bytes larger than section

                let layout = Layout::from_size_align(larger_size, 8).unwrap();
                let mut buffer = alloc(layout) as *mut c_void;
                assert!(!buffer.is_null(), "Memory allocation failed!");

                // Fill the buffer with a pattern to check the copy
                let buffer_slice = slice::from_raw_parts_mut(buffer as *mut u8, larger_size);
                buffer_slice.fill(0xFE);

                let mut buffer_size = larger_size;
                let status = MockProtocolData::fv_read_section_efiapi(
                    fv_ptr1,
                    name_guid,
                    19,
                    0,
                    &mut buffer,
                    &mut buffer_size,
                    &mut auth_status,
                );

                // 1. Status should be SUCCESS
                assert_eq!(status, efi::Status::SUCCESS, "Expected SUCCESS when buffer is larger than file size");

                // 2. buffer_size should be updated to the actual section size
                assert_eq!(buffer_size, actual_section_size, "buffer_size should be updated to actual section size");

                // 3. Verify only the section data was copied (rest should remain 0xFE)
                let copied_data = slice::from_raw_parts(buffer as *const u8, actual_section_size);
                let all_ff = copied_data.iter().all(|&b| b == 0xFE);
                assert!(!all_ff, "Section data should have been copied to buffer (not all 0xFE)");

                let remaining_data = slice::from_raw_parts(
                    buffer.add(actual_section_size) as *const u8,
                    larger_size - actual_section_size,
                );
                let all_ff_remaining = remaining_data.iter().all(|&b| b == 0xFE);
                assert!(all_ff_remaining, "Remaining buffer beyond section size should remain unchanged (all 0xFE)");
            }
        })
        .unwrap()
    }
}
