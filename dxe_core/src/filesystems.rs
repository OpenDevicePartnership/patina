//! DXE Core Filesystem
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::{vec, vec::Vec};
use core::{ffi::c_void, mem::size_of};
use r_efi::efi;

use crate::protocols::PROTOCOL_DB;

/// Provides a wrapper for interacting with SimpleFileSystem
pub struct SimpleFile<'a> {
    file: &'a mut efi::protocols::file::Protocol,
}

impl SimpleFile<'_> {
    /// Opens the given filename with appropriate mode/attributes and returns a new instance of SimpleFile for it.
    pub fn open(&mut self, filename: Vec<u16>, mode: u64, attributes: u64) -> Result<Self, efi::Status> {
        let mut file_ptr = core::ptr::null_mut();
        let status = (self.file.open)(
            self.file,
            core::ptr::addr_of_mut!(file_ptr),
            filename.as_ptr() as *mut u16,
            mode,
            attributes,
        );

        if status != efi::Status::SUCCESS {
            return Err(status);
        }

        let file = unsafe { file_ptr.as_mut().ok_or(efi::Status::NOT_FOUND)? };
        Ok(Self { file })
    }

    /// Opens the root of a Simple File System and returns a SimpleFile object for it.
    pub fn open_volume(handle: efi::Handle) -> Result<Self, efi::Status> {
        let sfs = unsafe {
            let sfs_protocol_ptr =
                PROTOCOL_DB.get_interface_for_handle(handle, efi::protocols::simple_file_system::PROTOCOL_GUID)?;
            (sfs_protocol_ptr as *mut efi::protocols::simple_file_system::Protocol)
                .as_mut()
                .ok_or(efi::Status::NOT_FOUND)?
        };

        let mut file_system_ptr = core::ptr::null_mut();
        let status = (sfs.open_volume)(sfs, core::ptr::addr_of_mut!(file_system_ptr));
        if status != efi::Status::SUCCESS {
            Err(status)?;
        }

        let root = unsafe { file_system_ptr.as_mut().ok_or(efi::Status::NOT_FOUND)? };

        Ok(Self { file: root })
    }

    // returns a byte buffer containing the file info for this SimpleFile instance.
    fn get_info(&mut self) -> Result<Vec<u8>, efi::Status> {
        let mut info_size = 0;
        let status = (self.file.get_info)(
            self.file,
            &efi::protocols::file::INFO_ID as *const efi::Guid as *mut efi::Guid,
            core::ptr::addr_of_mut!(info_size),
            core::ptr::null_mut(),
        );
        match status {
            efi::Status::BUFFER_TOO_SMALL => (),                     //expected
            efi::Status::SUCCESS => Err(efi::Status::DEVICE_ERROR)?, //unexpected success.
            err => Err(err)?,                                        //unexpected failure.
        }

        let mut file_info_buffer = vec![0u8; info_size];
        let status = (self.file.get_info)(
            self.file,
            &efi::protocols::file::INFO_ID as *const efi::Guid as *mut efi::Guid,
            core::ptr::addr_of_mut!(info_size),
            file_info_buffer.as_mut_ptr() as *mut c_void,
        );

        if status != efi::Status::SUCCESS {
            Err(status)?;
        }
        Ok(file_info_buffer)
    }

    /// Returns the size of the file
    pub fn get_size(&mut self) -> Result<u64, efi::Status> {
        let file_info_buffer = self.get_info()?;

        //to avoid an unsafe transmute, read the file size directly from the buffer instead of trying to convert the whole
        //buffer efi::protocols::file::Info. The file size is the second u64 in that buffer. TODO: proper conversion routine
        //for byte buffer -> efi::protocols::file::Info.
        let file_size_as_bytes =
            file_info_buffer.chunks_exact(size_of::<u64>()).nth(1).ok_or(efi::Status::NOT_FOUND)?;

        Ok(u64::from_le_bytes(file_size_as_bytes.try_into().or(Err(efi::Status::INVALID_PARAMETER))?))
    }

    /// Returns the file attributes
    pub fn get_attribute(&mut self) -> Result<u64, efi::Status> {
        let file_info_buffer = self.get_info()?;

        //to avoid an unsafe transmute, read the attribute directly from the buffer instead of trying to convert the whole
        //buffer efi::protocols::file::Info. The attribute is the 10th u64 in that buffer. TODO: proper conversion routine
        //for byte buffer -> efi::protocols::file::Info
        let file_attribute = file_info_buffer.chunks_exact(size_of::<u64>()).nth(9).ok_or(efi::Status::NOT_FOUND)?;

        Ok(u64::from_le_bytes(file_attribute.try_into().or(Err(efi::Status::INVALID_PARAMETER))?))
    }

    /// Reads the entire file into a byte vector and returns it
    pub fn read(&mut self) -> Result<Vec<u8>, efi::Status> {
        let file_attribute = self.get_attribute()?;
        if (file_attribute & efi::protocols::file::DIRECTORY) != 0 {
            Err(efi::Status::NOT_FOUND)?;
        }

        let mut file_size = self.get_size()? as usize;
        let mut file_buffer = vec![0u8; file_size];

        let status = (self.file.set_position)(self.file, 0);
        if status != efi::Status::SUCCESS {
            Err(status)?;
        }

        let status =
            (self.file.read)(self.file, core::ptr::addr_of_mut!(file_size), file_buffer.as_mut_ptr() as *mut c_void);

        if status != efi::Status::SUCCESS {
            Err(status)?;
        }

        //in case the read somehow returned fewer bytes than indicated by get_size, truncate the vector returned to the
        //actual read size.
        assert!(file_size <= file_buffer.len());
        if file_size < file_buffer.len() {
            Ok(file_buffer[0..file_size].to_vec())
        } else {
            Ok(file_buffer)
        }
    }
}
