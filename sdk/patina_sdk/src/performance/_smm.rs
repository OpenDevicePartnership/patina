//! This module is a temporary module that has for goal to make communication protocol work in perf. It will eventually
//! be replaced by another communicate abstraction.
//!
//! This module also contain smm performance communicate structures that define the communicate buffer data that need
//! to be used to fetch perf records from smm.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

// Allow missing docs since this is a temporary module.
#![allow(missing_docs)]

use core::{debug_assert_eq, ptr, slice};

use r_efi::efi;

use crate::{base::UEFI_PAGE_SIZE, component::hob::FromHob, uefi_protocol::ProtocolInterface};
use scroll::{
    Endian, Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

pub const EFI_SMM_COMMUNICATION_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0xc68ed8e2, 0x9dc6, 0x4cbd, 0x9d, 0x94, &[0xdb, 0x65, 0xac, 0xc5, 0xc3, 0x32]);

#[derive(Debug, Clone, Copy, Pread)]
#[repr(C)]
pub struct MmCommRegion {
    pub region_type: u64,
    pub region_address: u64,
    pub region_nb_pages: u64,
}

impl FromHob for MmCommRegion {
    const HOB_GUID: efi::Guid =
        efi::Guid::from_fields(0xd4ffc718, 0xfb82, 0x4274, 0x9a, 0xfc, &[0xaa, 0x8b, 0x1e, 0xef, 0x52, 0x93]);

    fn parse(bytes: &[u8]) -> Self {
        bytes.pread(0).unwrap()
    }
}

impl MmCommRegion {
    pub fn is_supervisor_type(&self) -> bool {
        self.region_type == 0
    }

    pub fn is_user_type(&self) -> bool {
        self.region_type == 1
    }

    pub fn size(&self) -> usize {
        self.region_nb_pages as usize * UEFI_PAGE_SIZE
    }

    /// Get the memory region as a mutable buffer.
    ///
    /// # Safety
    /// This function is unsafe because it assumes that the memory region is valid and properly aligned.
    ///
    /// - The caller must ensure that the `region_address` points to a valid memory region of size `size()`.
    /// - The caller must also ensure that the memory region is not used concurrently by other parts of the code.
    ///
    /// # Returns
    /// A mutable slice representing the memory region.
    pub unsafe fn as_buffer(&self) -> &'static mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.region_address as usize as *mut u8, self.size()) }
    }
}

pub type Communicate =
    extern "efiapi" fn(this: *mut CommunicateProtocol, comm_buffer: *mut u8, comm_size: *mut usize) -> efi::Status;

pub struct CommunicateProtocol {
    pub communicate: Communicate,
}

unsafe impl ProtocolInterface for CommunicateProtocol {
    const PROTOCOL_GUID: efi::Guid = EFI_SMM_COMMUNICATION_PROTOCOL_GUID;
}

/// This trait should be implemented on type that represents communicate data.
///
/// [`TryIntoCtx`] is used to define how to write the struct as the data in communicate buffer.
///
/// [`TryFromCtx`] is used to define how to read the struct as the data in communicate buffer.
///
/// # Safety
/// Make sure you write and read the struct in the expected format defined by the guid.
pub unsafe trait CommunicateData:
    TryIntoCtx<Endian, Error = scroll::Error> + TryFromCtx<'static, Endian, Error = scroll::Error>
{
    /// Guid use as header guid in the communicate buffer.
    const GUID: efi::Guid;
}

impl CommunicateProtocol {
    /// Abstraction over [Communicate].
    ///
    /// # Safety
    /// Make sure the communication_memory_region is valid.
    pub unsafe fn communicate<T>(
        &mut self,
        data: T,
        communication_memory_region: MmCommRegion,
    ) -> Result<T, efi::Status>
    where
        T: CommunicateData,
    {
        assert_ne!(0, communication_memory_region.region_address);
        assert_ne!(0, communication_memory_region.region_nb_pages);

        let comm_buffer = unsafe { communication_memory_region.as_buffer() };
        let mut offset = 0;

        comm_buffer.gwrite_with(T::GUID.as_bytes().as_slice(), &mut offset, ()).unwrap();

        let size_offset = offset;
        // Write place holder data size for now.
        comm_buffer.gwrite_with(0_u64, &mut offset, scroll::NATIVE).unwrap();

        let data_offset = offset;
        comm_buffer.gwrite_with(data, &mut offset, scroll::NATIVE).unwrap();

        // Write the data actual size.
        comm_buffer.pwrite(offset as u64, size_offset).unwrap();

        let mut comm_size = comm_buffer.len();
        let status = (self.communicate)(self, comm_buffer.as_mut_ptr(), ptr::addr_of_mut!(comm_size));

        if status.is_error() {
            Err(status)
        } else {
            Ok(comm_buffer.pread_with::<T>(data_offset, scroll::NATIVE).unwrap())
        }
    }
}

pub const EFI_FIRMWARE_PERFORMANCE_GUID: efi::Guid =
    efi::Guid::from_fields(0xc095791a, 0x3001, 0x47b2, 0x80, 0xc9, &[0xea, 0xc7, 0x31, 0x9f, 0x2f, 0xa4]);

// Communicate protocol data to ask smm the size of its performance records.
#[derive(Debug, Default)]
pub struct SmmGetRecordSize {
    pub return_status: efi::Status,
    pub boot_record_size: usize,
}

impl SmmGetRecordSize {
    pub const SMM_FPDT_FUNCTION_GET_BOOT_RECORD_SIZE: u64 = 1;

    pub fn new() -> Self {
        Self::default()
    }
}

unsafe impl CommunicateData for SmmGetRecordSize {
    const GUID: efi::Guid = EFI_FIRMWARE_PERFORMANCE_GUID;
}

impl TryIntoCtx<Endian> for SmmGetRecordSize {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: Endian) -> Result<usize, Self::Error> {
        let mut offset = 0;
        dest.gwrite_with(Self::SMM_FPDT_FUNCTION_GET_BOOT_RECORD_SIZE, &mut offset, ctx)?;
        dest.gwrite_with(self.return_status.as_usize() as u64, &mut offset, ctx)?;
        dest.gwrite_with(self.boot_record_size as u64, &mut offset, ctx)?;
        dest.gwrite_with(0_u64, &mut offset, ctx)?; // Boot record data.
        dest.gwrite_with(0_u64, &mut offset, ctx)?; // Boot record offset.
        Ok(offset)
    }
}

impl TryFromCtx<'_, Endian> for SmmGetRecordSize {
    type Error = scroll::Error;

    fn try_from_ctx(from: &'_ [u8], ctx: Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let function = from.gread_with::<u64>(&mut offset, ctx)?;
        debug_assert_eq!(Self::SMM_FPDT_FUNCTION_GET_BOOT_RECORD_SIZE, function);
        let return_status = efi::Status::from_usize(from.gread_with::<u64>(&mut offset, ctx)? as usize);
        let boot_record_size = from.gread_with::<u64>(&mut offset, ctx)? as usize;
        let _boot_record_data_address = from.gread_with::<u64>(&mut offset, ctx)? as usize;
        let _boot_record_offset = from.gread_with::<u64>(&mut offset, ctx)? as usize;

        Ok((Self { boot_record_size, return_status }, offset))
    }
}

// Communicate protocol data to ask smm to return a BUFFER_SIZE about of byte at an offset.
#[derive(Debug)]
pub struct SmmGetRecordDataByOffset<const BUFFER_SIZE: usize> {
    pub return_status: efi::Status,
    pub boot_record_data: [u8; BUFFER_SIZE],
    pub boot_record_data_size: usize,
    pub boot_record_offset: usize,
}

impl<const BUFFER_SIZE: usize> SmmGetRecordDataByOffset<BUFFER_SIZE> {
    pub const SMM_FPDT_FUNCTION_GET_BOOT_RECORD_DATA_BY_OFFSET: u64 = 3;

    pub fn new(boot_record_offset: usize) -> SmmGetRecordDataByOffset<BUFFER_SIZE> {
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

unsafe impl<const BUFFER_SIZE: usize> CommunicateData for SmmGetRecordDataByOffset<BUFFER_SIZE> {
    const GUID: efi::Guid = EFI_FIRMWARE_PERFORMANCE_GUID;
}

impl<const BUFFER_SIZE: usize> TryIntoCtx<Endian> for SmmGetRecordDataByOffset<BUFFER_SIZE> {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: Endian) -> Result<usize, Self::Error> {
        let mut offset = 0;
        dest.gwrite_with(Self::SMM_FPDT_FUNCTION_GET_BOOT_RECORD_DATA_BY_OFFSET, &mut offset, ctx)?;
        dest.gwrite_with(self.return_status.as_usize() as u64, &mut offset, ctx)?;
        dest.gwrite_with(self.boot_record_data_size as u64, &mut offset, ctx)?;
        dest.gwrite_with(0_u64, &mut offset, ctx)?; // Boot record data.
        dest.gwrite_with(self.boot_record_offset as u64, &mut offset, ctx)?;
        Ok(offset)
    }
}

impl<const BUFFER_SIZE: usize> TryFromCtx<'_, Endian> for SmmGetRecordDataByOffset<BUFFER_SIZE> {
    type Error = scroll::Error;

    fn try_from_ctx(from: &'_ [u8], ctx: Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let function = from.gread_with::<u64>(&mut offset, ctx)?;
        debug_assert_eq!(Self::SMM_FPDT_FUNCTION_GET_BOOT_RECORD_DATA_BY_OFFSET, function);
        let return_status = efi::Status::from_usize(from.gread_with::<u64>(&mut offset, ctx)? as usize);
        let boot_record_data_size = from.gread_with::<u64>(&mut offset, ctx)? as usize;
        let _boot_record_data_address = from.gread_with::<u64>(&mut offset, ctx)? as usize;
        let boot_record_offset = from.gread_with::<u64>(&mut offset, ctx)? as usize;

        let boot_record_data = from.gread::<[u8; BUFFER_SIZE]>(&mut offset)?;

        Ok((Self { return_status, boot_record_data, boot_record_data_size, boot_record_offset }, offset))
    }
}
