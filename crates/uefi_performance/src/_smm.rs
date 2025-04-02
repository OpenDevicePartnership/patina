//! This module is a temporary module that has for goal to make communication protocol work in perf. It will eventually be replaced by another communicate abstraction.
//!
//! This module also contain smm performance communicate structures that define the communicate buffer data that need to be used to fetch perf records from smm.

use core::{debug_assert_eq, marker::PhantomPinned, ops::Deref, ptr, result::Result::Ok, slice};

use r_efi::efi;

use scroll::{
    ctx::{TryFromCtx, TryIntoCtx},
    Endian, Pread, Pwrite,
};
use uefi_sdk::{base::UEFI_PAGE_SIZE, protocol::Protocol};

pub const EFI_SMM_COMMUNICATION_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0xc68ed8e2, 0x9dc6, 0x4cbd, 0x9d, 0x94, &[0xdb, 0x65, 0xac, 0xc5, 0xc3, 0x32]);
pub const EDKII_PI_SMM_COMMUNICATION_REGION_TABLE_GUID: efi::Guid =
    efi::Guid::from_fields(0x4e28ca50, 0xd582, 0x44ac, 0xa1, 0x1f, &[0xe3, 0xd5, 0x65, 0x26, 0xdb, 0x34]);

#[derive(Debug)]
#[repr(C)]
/// Memory layout of a smm communication region table.
///
/// Memory descriptor of this struct are stored after in memory. To access those use the [`Self::iter`] function.
/// # Note
/// This struct is not sized, should never be used as an own type.
pub struct SmmCommunicationRegionTable {
    /// Version of the smm communication region table.
    pub version: u32,
    /// Number of memory descriptor present in the region table
    pub number_of_entries: u32,
    /// Size in byte of one memory descriptor
    pub descriptor_size: u32,
    _reserved: u32,
    /// Used to prevent the move because memory descriptors lives at the end of this struct
    _pin: PhantomPinned,
    memory_descriptors: [efi::MemoryDescriptor; 0],
}

impl SmmCommunicationRegionTable {
    // Iterate over memory descriptors at the end of the struct.
    pub fn iter(&self) -> SmmCommunicationRegionTableIter<'_> {
        SmmCommunicationRegionTableIter { index: 0, region_table: self }
    }
}

/// Memrory descriptor interation given by [`SmmCommunicationRegionTable::iter`].
pub struct SmmCommunicationRegionTableIter<'a> {
    index: usize,
    region_table: &'a SmmCommunicationRegionTable,
}

impl<'a> Iterator for SmmCommunicationRegionTableIter<'a> {
    type Item = &'a efi::MemoryDescriptor;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.region_table.number_of_entries as usize {
            return None;
        }

        let memory_descriptor_ptr = unsafe {
            self.region_table
                .memory_descriptors
                .as_ptr()
                .byte_add(self.index * self.region_table.descriptor_size as usize)
        };
        self.index += 1;
        unsafe { memory_descriptor_ptr.as_ref() }
    }
}

pub type Communicate = extern "efiapi" fn(
    this: *mut CommunicateProtocolInterface,
    comm_buffer: *mut u8,
    comm_size: *mut usize,
) -> efi::Status;

pub struct CommunicateProtocolInterface {
    pub communicate: Communicate,
}

pub struct CommunicateProtocol;

impl Deref for CommunicateProtocol {
    type Target = efi::Guid;

    fn deref(&self) -> &Self::Target {
        self.protocol_guid()
    }
}

unsafe impl Protocol for CommunicateProtocol {
    type Interface = CommunicateProtocolInterface;

    fn protocol_guid(&self) -> &'static efi::Guid {
        &EFI_SMM_COMMUNICATION_PROTOCOL_GUID
    }
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

impl CommunicateProtocolInterface {
    /// Abstraction over [Communicate].
    ///
    /// # Safety
    /// Make sure the communication_memory_region is valid.
    pub unsafe fn communicate<T>(
        &mut self,
        data: T,
        communication_memory_region: &efi::MemoryDescriptor,
    ) -> Result<T, efi::Status>
    where
        T: CommunicateData,
    {
        assert_eq!(efi::CONVENTIONAL_MEMORY, communication_memory_region.r#type);
        assert_ne!(0, communication_memory_region.physical_start);
        assert_ne!(0, communication_memory_region.number_of_pages);

        let mut comm_size = communication_memory_region.number_of_pages as usize * UEFI_PAGE_SIZE;

        let comm_buffer = slice::from_raw_parts_mut(communication_memory_region.physical_start as *mut u8, comm_size);
        let mut offset = 0;

        comm_buffer.gwrite_with(T::GUID.as_bytes().as_slice(), &mut offset, ()).unwrap();

        let size_offset = offset;
        // Write place holder data size for now.
        comm_buffer.gwrite_with(0_u64, &mut offset, scroll::NATIVE).unwrap();

        let data_offset = offset;
        comm_buffer.gwrite_with(data, &mut offset, scroll::NATIVE).unwrap();

        // Write the data actual size.
        comm_buffer.pwrite(offset as u64, size_offset).unwrap();

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
pub struct SmmFpdtGetRecordSize {
    pub return_status: efi::Status,
    pub boot_record_size: usize,
}

impl SmmFpdtGetRecordSize {
    pub const SMM_FPDT_FUNCTION_GET_BOOT_RECORD_SIZE: u64 = 1;

    pub fn new() -> Self {
        Self::default()
    }
}

unsafe impl CommunicateData for SmmFpdtGetRecordSize {
    const GUID: efi::Guid = EFI_FIRMWARE_PERFORMANCE_GUID;
}

impl TryIntoCtx<Endian> for SmmFpdtGetRecordSize {
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

impl TryFromCtx<'_, Endian> for SmmFpdtGetRecordSize {
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
pub struct SmmFpdtGetRecordDataByOffset<const BUFFER_SIZE: usize> {
    pub return_status: efi::Status,
    pub boot_record_data: [u8; BUFFER_SIZE],
    pub boot_record_data_size: usize,
    pub boot_record_offset: usize,
}

impl<const BUFFER_SIZE: usize> SmmFpdtGetRecordDataByOffset<BUFFER_SIZE> {
    pub const SMM_FPDT_FUNCTION_GET_BOOT_RECORD_DATA_BY_OFFSET: u64 = 3;

    pub fn new(boot_record_offset: usize) -> SmmFpdtGetRecordDataByOffset<BUFFER_SIZE> {
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

unsafe impl<const BUFFER_SIZE: usize> CommunicateData for SmmFpdtGetRecordDataByOffset<BUFFER_SIZE> {
    const GUID: efi::Guid = EFI_FIRMWARE_PERFORMANCE_GUID;
}

impl<const BUFFER_SIZE: usize> TryIntoCtx<Endian> for SmmFpdtGetRecordDataByOffset<BUFFER_SIZE> {
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

impl<const BUFFER_SIZE: usize> TryFromCtx<'_, Endian> for SmmFpdtGetRecordDataByOffset<BUFFER_SIZE> {
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
