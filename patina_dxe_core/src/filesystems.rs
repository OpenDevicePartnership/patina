//! DXE Core Filesystem
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::vec::Vec;
use core::{ffi::c_void, mem::size_of};
use patina::{Char16Str, error::EfiError, standard::efi};

use crate::protocols::PROTOCOL_DB;

/// Provides a wrapper for interacting with `SimpleFileSystem`
pub struct SimpleFile<'a> {
    file: &'a mut efi::protocols::file::Protocol,
}

impl SimpleFile<'_> {
    /// Opens the given filename with appropriate mode/attributes and returns a new instance of `SimpleFile` for it.
    pub fn open(&mut self, filename: &Char16Str, mode: u64, attributes: u64) -> Result<Self, EfiError> {
        let mut file_ptr = core::ptr::null_mut();
        // SAFETY: self.file is a valid pointer to a file protocol instance
        // obtained from the protocol database during the construction of this
        // SimpleFile instance.
        let status = unsafe {
            (self.file.open)(
                self.file,
                core::ptr::addr_of_mut!(file_ptr),
                filename.as_ptr().cast_mut(),
                mode,
                attributes,
            )
        };

        EfiError::status_to_result(status)?;

        // SAFETY: file_ptr is filled by the open call above on success.
        let file = unsafe { file_ptr.as_mut().ok_or(EfiError::NotFound)? };
        Ok(Self { file })
    }

    /// Opens the root of a Simple File System and returns a `SimpleFile` object for it.
    pub fn open_volume(handle: efi::Handle) -> Result<Self, EfiError> {
        // SAFETY: Protocol database returns a valid interface pointer for the handle.
        let sfs = unsafe {
            let sfs_protocol_ptr =
                PROTOCOL_DB.get_interface_for_handle(handle, efi::protocols::simple_file_system::PROTOCOL_GUID)?;
            sfs_protocol_ptr
                .cast::<efi::protocols::simple_file_system::Protocol>()
                .as_mut()
                .ok_or(EfiError::NotFound)?
        };

        let mut file_system_ptr = core::ptr::null_mut();
        // SAFETY: sfs is a valid pointer to a simple file system protocol instance
        // obtained from the protocol database above.
        let status = unsafe { (sfs.open_volume)(sfs, core::ptr::addr_of_mut!(file_system_ptr)) };
        EfiError::status_to_result(status)?;

        // SAFETY: file_system_ptr is filled by the open_volume call above on success.
        let root = unsafe { file_system_ptr.as_mut().ok_or(EfiError::NotFound)? };

        Ok(Self { file: root })
    }

    // returns a byte buffer containing the file info for this SimpleFile instance.
    fn get_info(&mut self) -> Result<Vec<u8>, EfiError> {
        let mut info_size = 0;
        // SAFETY: self.file is a valid pointer to a file protocol instance
        // obtained from the protocol database during the construction of this
        // SimpleFile instance.
        let status = unsafe {
            (self.file.get_info)(
                self.file,
                core::ptr::from_ref::<efi::Guid>(&efi::protocols::file::INFO_ID).cast_mut(),
                core::ptr::addr_of_mut!(info_size),
                core::ptr::null_mut(),
            )
        };
        match status {
            efi::Status::BUFFER_TOO_SMALL => (),                 // expected
            efi::Status::SUCCESS => Err(EfiError::DeviceError)?, // unexpected success.
            err => EfiError::status_to_result(err)?,             // unexpected failure.
        }

        let mut file_info_buffer = Vec::new();
        file_info_buffer.try_reserve_exact(info_size).map_err(|_| EfiError::OutOfResources)?;
        let reserved_size = info_size;
        // SAFETY: self.file is a valid pointer to a file protocol instance
        // obtained from the protocol database during the construction of this
        // SimpleFile instance. file_info_buffer has room for info_size bytes.
        let status = unsafe {
            (self.file.get_info)(
                self.file,
                core::ptr::from_ref::<efi::Guid>(&efi::protocols::file::INFO_ID).cast_mut(),
                core::ptr::addr_of_mut!(info_size),
                file_info_buffer.spare_capacity_mut().as_mut_ptr().cast::<c_void>(),
            )
        };
        EfiError::status_to_result(status)?;

        if info_size > reserved_size {
            Err(EfiError::DeviceError)?;
        }
        // SAFETY: GetInfo() initialized the first info_size bytes of the buffer.
        unsafe { file_info_buffer.set_len(info_size) };
        Ok(file_info_buffer)
    }

    /// Returns the size of the file
    pub fn get_size(&mut self) -> Result<u64, EfiError> {
        let file_info_buffer = self.get_info()?;

        //to avoid an unsafe transmute, read the file size directly from the buffer instead of trying to convert the whole
        //buffer efi::protocols::file::Info. The file size is the second u64 in that buffer. TODO: proper conversion routine
        //for byte buffer -> efi::protocols::file::Info.
        let file_size_as_bytes =
            file_info_buffer.as_chunks::<{ size_of::<u64>() }>().0.get(1).ok_or(EfiError::NotFound)?;

        Ok(u64::from_le_bytes(*file_size_as_bytes))
    }

    /// Returns the file attributes
    pub fn get_attribute(&mut self) -> Result<u64, EfiError> {
        let file_info_buffer = self.get_info()?;

        //to avoid an unsafe transmute, read the attribute directly from the buffer instead of trying to convert the whole
        //buffer efi::protocols::file::Info. The attribute is the 10th u64 in that buffer. TODO: proper conversion routine
        //for byte buffer -> efi::protocols::file::Info
        let file_attribute = file_info_buffer.as_chunks::<{ size_of::<u64>() }>().0.get(9).ok_or(EfiError::NotFound)?;

        Ok(u64::from_le_bytes(*file_attribute))
    }

    /// Reads the entire file into a byte vector and returns it
    pub fn read(&mut self) -> Result<Vec<u8>, EfiError> {
        let file_attribute = self.get_attribute()?;
        if (file_attribute & efi::protocols::file::DIRECTORY) != 0 {
            Err(EfiError::NotFound)?;
        }

        let mut file_size = self.get_size()? as usize;
        let mut file_buffer = Vec::new();
        file_buffer.try_reserve_exact(file_size).map_err(|_| EfiError::OutOfResources)?;
        let reserved_size = file_size;
        // SAFETY: self.file is a valid pointer to a file protocol instance
        // obtained from the protocol database during the construction of this
        // SimpleFile instance.
        let status = unsafe { (self.file.set_position)(self.file, 0) };
        EfiError::status_to_result(status)?;

        // SAFETY: self.file is a valid pointer to a file protocol instance
        // obtained from the protocol database during the construction of this
        // SimpleFile instance. file_buffer has room for file_size bytes.
        let status = unsafe {
            (self.file.read)(
                self.file,
                core::ptr::addr_of_mut!(file_size),
                file_buffer.spare_capacity_mut().as_mut_ptr().cast::<c_void>(),
            )
        };
        EfiError::status_to_result(status)?;

        if file_size > reserved_size {
            Err(EfiError::DeviceError)?;
        }
        // SAFETY: Read() initialized the first file_size bytes of the buffer.
        unsafe { file_buffer.set_len(file_size) };
        Ok(file_buffer)
    }
}
