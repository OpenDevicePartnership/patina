//! Patina Performance Management Mode (MM) Module
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use r_efi::efi;
use zerocopy::{IntoBytes, LittleEndian, U64};
use zerocopy_derive::*;

/// Errors that may occur when parsing MM structures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Buffer is too small to contain the required data.
    BufferTooSmall { required: usize, available: usize },
    /// Function ID in the header doesn't match expected value.
    InvalidFunctionId { expected: u64, found: u64 },
}

/// SMM communication header wire format.
/// This represents the 5 64-bit fields used for communication with the MM Performance module.
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable)]
pub struct SmmCommHeader {
    pub function_id: U64<LittleEndian>,
    pub return_status: U64<LittleEndian>,
    pub boot_record_size: U64<LittleEndian>,
    pub boot_record_data: U64<LittleEndian>,
    pub boot_record_offset: U64<LittleEndian>,
}

/// Size in bytes of the SMM communication header.
pub const SMM_COMM_HEADER_SIZE: usize = core::mem::size_of::<SmmCommHeader>();

impl SmmCommHeader {
    /// Create a new SMM communication header for the given function and parameters.
    pub fn new(
        function_id: u64,
        return_status: efi::Status,
        boot_record_size: usize,
        boot_record_data: u64,
        boot_record_offset: usize,
    ) -> Self {
        Self {
            function_id: U64::new(function_id),
            return_status: U64::new(return_status.as_usize() as u64),
            boot_record_size: U64::new(boot_record_size as u64),
            boot_record_data: U64::new(boot_record_data),
            boot_record_offset: U64::new(boot_record_offset as u64),
        }
    }

    /// Write this header into the destination buffer.
    pub fn write_into(&self, dest: &mut [u8]) -> Result<usize, ()> {
        if dest.len() < SMM_COMM_HEADER_SIZE {
            return Err(());
        }
        dest[..SMM_COMM_HEADER_SIZE].copy_from_slice(self.as_bytes());
        Ok(SMM_COMM_HEADER_SIZE)
    }

    /// Read a header from the source buffer, validating the expected function ID.
    pub fn read_from(src: &[u8], expected_function_id: u64) -> Result<(Self, usize), ParseError> {
        if src.len() < SMM_COMM_HEADER_SIZE {
            return Err(ParseError::BufferTooSmall { required: SMM_COMM_HEADER_SIZE, available: src.len() });
        }
        // SAFETY: The length is validated. SmmCommHeader is repr(C) and plain data.
        let header = unsafe { &*(src.as_ptr() as *const SmmCommHeader) };
        let found_function_id = header.function_id.get();
        if found_function_id != expected_function_id {
            return Err(ParseError::InvalidFunctionId { expected: expected_function_id, found: found_function_id });
        }
        Ok((*header, SMM_COMM_HEADER_SIZE))
    }

    /// Get the return status as an efi::Status.
    pub fn return_status(&self) -> efi::Status {
        efi::Status::from_usize(self.return_status.get() as usize)
    }

    /// Get the boot record size as a usize.
    pub fn boot_record_size(&self) -> usize {
        self.boot_record_size.get() as usize
    }

    /// Get the boot record offset as a usize.
    pub fn boot_record_offset(&self) -> usize {
        self.boot_record_offset.get() as usize
    }
}

/// Maximum number of bytes for SMM boot performance records.
///
/// This is a defensive upper bound to avoid attempting to allocate or copy
/// unreasonably large buffers if firmware returns a bad size. It can be
/// adjusted if a platform legitimately requires more.
pub const MAX_SMM_BOOT_RECORD_BYTES: usize = 2 * 1024 * 1024; // 2 MiB

/// Default chunk size for fetching SMM boot performance records.
pub const SMM_FETCH_CHUNK_BYTES: usize = 1024;

pub const EFI_FIRMWARE_PERFORMANCE_GUID: efi::Guid =
    efi::Guid::from_fields(0xc095791a, 0x3001, 0x47b2, 0x80, 0xc9, &[0xea, 0xc7, 0x31, 0x9f, 0x2f, 0xa4]);

/// MM communicate function to return performance record size info.
#[derive(Debug, Default, Copy, Clone)]
pub struct GetRecordSize {
    pub return_status: efi::Status,
    pub boot_record_size: usize,
}

impl GetRecordSize {
    pub const FUNCTION_ID: u64 = 1;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn write_into(self, dest: &mut [u8]) -> Result<usize, ()> {
        let header = SmmCommHeader::new(
            Self::FUNCTION_ID,
            self.return_status,
            self.boot_record_size,
            0, // boot_record_data (unused for GetRecordSize)
            0, // boot_record_offset (unused for GetRecordSize)
        );
        header.write_into(dest)
    }

    pub fn read_from(src: &[u8]) -> Result<(Self, usize), ParseError> {
        let (header, consumed) = SmmCommHeader::read_from(src, Self::FUNCTION_ID)?;
        Ok((Self { boot_record_size: header.boot_record_size(), return_status: header.return_status() }, consumed))
    }
}

/// MM communicate helper to get a BUFFER_SIZE of bytes at an offset.
#[derive(Debug, Copy, Clone)]
pub struct GetRecordDataByOffset<const BUFFER_SIZE: usize = SMM_FETCH_CHUNK_BYTES> {
    pub return_status: efi::Status,
    pub boot_record_data: [u8; BUFFER_SIZE],
    pub boot_record_data_size: usize,
    pub boot_record_offset: usize,
}

impl<const BUFFER_SIZE: usize> GetRecordDataByOffset<BUFFER_SIZE> {
    pub const FUNCTION_ID: u64 = 3;

    pub fn new(boot_record_offset: usize) -> GetRecordDataByOffset<BUFFER_SIZE> {
        Self {
            return_status: efi::Status::SUCCESS,
            boot_record_data: [0; BUFFER_SIZE],
            boot_record_data_size: BUFFER_SIZE,
            boot_record_offset,
        }
    }

    pub fn boot_record_data(&self) -> &[u8] {
        &self.boot_record_data[..self.boot_record_data_size]
    }
}

impl GetRecordDataByOffset<SMM_FETCH_CHUNK_BYTES> {
    /// Creates a new instance with the default buffer size.
    /// This is a convenience method made available to avoid having to specify the const generic parameter.
    pub fn new_default(boot_record_offset: usize) -> Self {
        Self::new(boot_record_offset)
    }

    /// Reads from buffer using the default buffer size.
    /// This is a convenience method made available to avoid having to specify the const generic parameter.
    pub fn read_from_default(src: &[u8]) -> Result<(Self, usize), ParseError> {
        Self::read_from(src)
    }
}

impl<const BUFFER_SIZE: usize> GetRecordDataByOffset<BUFFER_SIZE> {
    pub fn write_into(self, dest: &mut [u8]) -> Result<usize, ()> {
        let header = SmmCommHeader::new(
            Self::FUNCTION_ID,
            self.return_status,
            self.boot_record_data_size,
            0, // boot_record_data (unused)
            self.boot_record_offset,
        );
        header.write_into(dest)
    }

    pub fn read_from(src: &[u8]) -> Result<(Self, usize), ParseError> {
        let (header, header_consumed) = SmmCommHeader::read_from(src, Self::FUNCTION_ID)?;

        let mut boot_record_data = [0u8; BUFFER_SIZE];
        let remaining = src.len().saturating_sub(header_consumed);
        let take = core::cmp::min(remaining, BUFFER_SIZE);
        boot_record_data[..take].copy_from_slice(&src[header_consumed..header_consumed + take]);

        Ok((
            Self {
                return_status: header.return_status(),
                boot_record_data,
                boot_record_data_size: header.boot_record_size(),
                boot_record_offset: header.boot_record_offset(),
            },
            header_consumed + take,
        ))
    }
}
