//! Defines the API and default implementation of performance tables such as the Firmware Basic Boot Performance
//! Table (FBPT).
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

use alloc::vec::Vec;
use core::{
    fmt::Debug,
    marker::Sized,
    mem, ptr, slice,
    sync::atomic::{AtomicPtr, Ordering},
};

use crate::{
    base::UEFI_PAGE_SIZE,
    boot_services::{BootServices, allocation::AllocType},
    efi_types::EfiMemoryType,
    error::EfiError,
    performance::{
        self,
        error::Error,
        record::{PerformanceRecord, PerformanceRecordBuffer},
    },
    runtime_services::RuntimeServices,
};

use r_efi::efi;
use scroll::Pwrite;

/// The number of extra space in byte that will be allocated when publishing the performance buffer.
/// This is used for every performance records that will be added to the table after it is published.
// const PUBLISHED_FBPT_EXTRA_SPACE: usize = 0x400_000;
const PUBLISHED_FBPT_EXTRA_SPACE: usize = 0x10_000;

/// Interface for a Firmware Basic Boot Performance Table.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait FirmwareBasicBootPerfTable: Sized {
    /// Return the address where the table is.
    fn fbpt_address(&self) -> usize;

    /// Return every performance records that has been added to the table.
    fn perf_records(&self) -> &PerformanceRecordBuffer;

    /// Initialize the performance records.
    fn set_perf_records(&mut self, perf_records: PerformanceRecordBuffer);

    /// Add a performance record to the table.
    #[cfg_attr(test, mockall::concretize)]
    fn add_record<T: PerformanceRecord>(&mut self, record: T) -> Result<(), Error>;

    /// Report table allocate new space of memory and move the table to a specific place so it can be found later, the address where the table is allocated is returned.
    /// Additional memory is allocated so the table can still grow in the future step.
    fn report_table<B: BootServices + 'static>(
        &mut self,
        address: Option<usize>,
        boot_services: &B,
    ) -> Result<usize, Error>;
}

/// Firmware Basic Boot Performance Table (FBPT)
#[derive(Debug)]
pub struct FBPT {
    /// When the table will be reported, this will be the address where the fbpt table is.
    fbpt_address: usize,
    /// First value is the length when the table is not been reported and the second one is when the table is reported.
    /// Use `length()` or `length_mut()`. Do not use this field directly.
    _length: (u32, AtomicPtr<u32>),
    /// Buffer containing all the performance record.
    other_records: PerformanceRecordBuffer,
}

impl FBPT {
    /// FBPT - Firmware Basic Boot Performance Table signature
    pub const SIGNATURE: u32 = u32::from_le_bytes([b'F', b'B', b'P', b'T']);

    /// Create an new empty FBPT.
    pub const fn new() -> Self {
        Self {
            fbpt_address: 0,
            _length: (Self::size_of_empty_table() as u32, AtomicPtr::new(ptr::null_mut())),
            other_records: PerformanceRecordBuffer::new(),
        }
    }

    /// Return the size in bytes of the FBPT table.
    pub fn length(&self) -> &u32 {
        // SAFETY: `length` is a valid field in the FBPT structure.
        unsafe { self._length.1.load(Ordering::Relaxed).as_ref() }.unwrap_or(&self._length.0)
    }

    fn length_mut(&mut self) -> &mut u32 {
        // SAFETY: `length` is a valid field in the FBPT structure.
        unsafe { self._length.1.load(Ordering::Relaxed).as_mut() }.unwrap_or(&mut self._length.0)
    }

    const fn size_of_empty_table() -> usize {
        mem::size_of::<u32>() // Header signature
        + mem::size_of::<u32>() // Header length
        + performance::record::PERFORMANCE_RECORD_HEADER_SIZE
        + FirmwareBasicBootPerfDataRecord::data_size()
    }

    fn allocate_table_buffer(
        &self,
        previous_address: Option<usize>,
        boot_services: &impl BootServices,
    ) -> Result<&'static mut [u8], EfiError> {
        let allocation_size = Self::size_of_empty_table() + self.other_records.size() + PUBLISHED_FBPT_EXTRA_SPACE;
        let allocation_nb_page = allocation_size.div_ceil(UEFI_PAGE_SIZE);
        let allocation_size = allocation_nb_page * UEFI_PAGE_SIZE;

        let address = previous_address
            .and_then(|address| {
                boot_services
                    .allocate_pages(AllocType::Address(address), EfiMemoryType::ReservedMemoryType, allocation_nb_page)
                    .ok()
            })
            .map_or_else(
                || {
                    // Allocate at a new address if no address found or if the previous address allocation failed.
                    boot_services.allocate_pages(
                        AllocType::MaxAddress(u32::MAX as usize),
                        EfiMemoryType::ReservedMemoryType,
                        allocation_nb_page,
                    )
                },
                Result::Ok,
            )? as *mut u8;

        // SAFETY: the allocation at this addres was of size `allocation_size`
        Ok(unsafe { slice::from_raw_parts_mut(address, allocation_size) })
    }
}

impl FirmwareBasicBootPerfTable for FBPT {
    fn fbpt_address(&self) -> usize {
        self.fbpt_address
    }

    fn perf_records(&self) -> &PerformanceRecordBuffer {
        &self.other_records
    }

    fn set_perf_records(&mut self, perf_records: PerformanceRecordBuffer) {
        *self.length_mut() = (Self::size_of_empty_table() + perf_records.size()) as u32;
        self.other_records = perf_records;
    }

    fn add_record<T: PerformanceRecord>(&mut self, record: T) -> Result<(), Error> {
        let record_size = self.other_records.push_record(record)?;
        *self.length_mut() += record_size as u32;
        Ok(())
    }

    fn report_table<B: BootServices + 'static>(
        &mut self,
        address: Option<usize>,
        boot_services: &B,
    ) -> Result<usize, Error> {
        let fbpt_buffer = self.allocate_table_buffer(address, boot_services)?;

        self.fbpt_address = fbpt_buffer.as_ptr() as usize;

        let mut offset = 0;
        fbpt_buffer.gwrite(Self::SIGNATURE, &mut offset).map_err(|_| Error::BufferTooSmall)?;
        // SAFETY: `allocate_table_buffer` ensures the `fbpt_buffer` is large enough to hold the FBPT structure.
        let length_ptr = unsafe { fbpt_buffer.as_ptr().byte_add(offset) } as *mut u32;
        fbpt_buffer.gwrite(*self.length(), &mut offset).map_err(|_| Error::BufferTooSmall)?;
        FirmwareBasicBootPerfDataRecord::new()
            .write_into(fbpt_buffer, &mut offset)
            .map_err(|_| Error::BufferTooSmall)?;

        debug_assert_eq!(Self::size_of_empty_table(), offset);
        self.other_records.report(&mut fbpt_buffer[offset..])?;

        self._length.1.store(length_ptr, Ordering::Relaxed);
        Ok(self.fbpt_address)
    }
}

impl Default for FBPT {
    fn default() -> Self {
        Self::new()
    }
}

/// Return the address where the FBPT has been allocated during the previous boot.
pub fn find_previous_table_address(runtime_services: &impl RuntimeServices) -> Option<usize> {
    runtime_services
        .get_variable::<FirmwarePerformanceVariable>(
            &[0],
            &FirmwarePerformanceVariable::ADDRESS_VARIABLE_GUID,
            Some(mem::size_of::<FirmwarePerformanceVariable>()),
        )
        .map(|(v, _)| v.boot_performance_table_pointer)
        .ok()
}

/// Struct used to get the value from the FirmwarePerformanceVariable
#[repr(C)]
pub struct FirmwarePerformanceVariable {
    boot_performance_table_pointer: usize,
    _s3_performance_table_pointer: usize,
}

impl FirmwarePerformanceVariable {
    const ADDRESS_VARIABLE_GUID: efi::Guid =
        efi::Guid::from_fields(0xc095791a, 0x3001, 0x47b2, 0x80, 0xc9, &[0xea, 0xc7, 0x31, 0x9f, 0x2f, 0xa4]);
}

impl TryFrom<Vec<u8>> for FirmwarePerformanceVariable {
    type Error = ();

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        if value.len() == mem::size_of::<Self>() {
            // SAFETY: This is safe because the value for ADDRESS_VARIABLE_GUID is an address where a FirmwarePerformanceVariable is.
            Ok(unsafe { ptr::read_unaligned(value.as_ptr() as *const FirmwarePerformanceVariable) })
        } else {
            Err(())
        }
    }
}

#[derive(Clone)]
#[repr(C)]
/// Firmware Basic Boot Performance Record
pub struct FirmwareBasicBootPerfDataRecord {
    /// Timer value logged at the beginning of firmware image execution. This may not always be zero or near zero.
    pub reset_end: u64,
    /// Timer value logged just prior to loading the OS boot loader into memory. For non-UEFI compatible boots, this field must be zero.
    pub os_loader_load_image_start: u64,
    /// Timer value logged just prior to launching the currently loaded OS boot loader image.
    /// For non-UEFI compatible boots, the timer value logged will be just prior to the INT 19h handler invocation.
    pub os_loader_start_image_start: u64,
    /// Timer value logged at the point when the OS loader calls the ExitBootServices function for UEFI compatible firmware.
    /// For non-UEFI compatible boots, this field must be zero.
    pub exit_boot_services_entry: u64,
    /// Timer value logged at the point just prior to the OS loader gaining control back from the
    /// ExitBootServices function for UEFI compatible firmware.
    /// For non-UEFI compatible boots, this field must be zero.
    pub exit_boot_services_exit: u64,
}

impl FirmwareBasicBootPerfDataRecord {
    const TYPE: u16 = 2;
    const REVISION: u8 = 2;

    /// Create a new empty FirmwareBasicBootPerfDataRecord.
    pub const fn new() -> Self {
        Self {
            reset_end: 0,
            os_loader_load_image_start: 0,
            os_loader_start_image_start: 0,
            exit_boot_services_entry: 0,
            exit_boot_services_exit: 0,
        }
    }

    const fn data_size() -> usize {
        4 // Reserved bytes
        + mem::size_of::<Self>()
    }
}

impl PerformanceRecord for FirmwareBasicBootPerfDataRecord {
    fn record_type(&self) -> u16 {
        Self::TYPE
    }

    fn revision(&self) -> u8 {
        Self::REVISION
    }

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), scroll::Error> {
        buff.gwrite_with([0_u8; 4], offset, scroll::NATIVE)?; // Reserved bytes
        buff.gwrite_with(self.reset_end, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.os_loader_load_image_start, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.os_loader_start_image_start, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.exit_boot_services_entry, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.exit_boot_services_exit, offset, scroll::NATIVE)?;
        Ok(())
    }
}

impl Default for FirmwareBasicBootPerfDataRecord {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    use core::{assert_eq, slice, unreachable};
    use scroll::Pread;

    use crate::{
        boot_services::MockBootServices,
        performance::{
            record::{
                GenericPerformanceRecord, PERFORMANCE_RECORD_HEADER_SIZE,
                extended::{
                    DualGuidStringEventRecord, DynamicStringEventRecord, GuidEventRecord, GuidQwordEventRecord,
                    GuidQwordStringEventRecord,
                },
            },
            table::FirmwareBasicBootPerfDataRecord,
        },
        runtime_services::MockRuntimeServices,
    };

    #[test]
    fn test_find_previous_address() {
        let mut runtime_services = MockRuntimeServices::new();

        runtime_services
            .expect_get_variable::<FirmwarePerformanceVariable>()
            .once()
            .withf(|name, namespace, size_hint| {
                assert_eq!(&[0], name);
                assert_eq!(&FirmwarePerformanceVariable::ADDRESS_VARIABLE_GUID, namespace);
                assert_eq!(&Some(16), size_hint);
                true
            })
            .returning(|_, _, _| {
                Ok((
                    FirmwarePerformanceVariable {
                        boot_performance_table_pointer: 0x12341234,
                        _s3_performance_table_pointer: 0,
                    },
                    16,
                ))
            });

        let address = find_previous_table_address(&runtime_services);

        assert_eq!(Some(0x12341234), address);
    }

    #[test]
    fn test_set_perf_records() {
        let mut performance_record_buffer = PerformanceRecordBuffer::new();
        performance_record_buffer
            .push_record(GenericPerformanceRecord { record_type: 1, length: 20, revision: 1, data: [0_u8; 16] })
            .unwrap();

        let mut fbpt = FBPT::new();
        assert_eq!(&56, fbpt.length());

        fbpt.set_perf_records(performance_record_buffer);
        assert_eq!(&76, fbpt.length());
    }

    #[test]
    fn test_reporting_fbpt_with_previous_address() {
        let memory_buffer = Vec::<u8>::with_capacity(1000);
        let address = memory_buffer.as_ptr() as usize;

        let mut boot_services = MockBootServices::new();
        boot_services
            .expect_allocate_pages()
            .once()
            .withf(move |alloc_type, memory_type, _| {
                assert_eq!(&AllocType::Address(address), alloc_type);
                assert_eq!(&EfiMemoryType::ReservedMemoryType, memory_type);
                true
            })
            .returning(move |_, _, _| Ok(address));

        let mut fbpt = FBPT::new();
        let guid = efi::Guid::from_bytes(&[0; 16]);
        fbpt.add_record(GuidEventRecord::new(1, 0, 10, guid)).unwrap();
        fbpt.add_record(DynamicStringEventRecord::new(1, 0, 10, guid, "test")).unwrap();

        fbpt.report_table(Some(address), &boot_services).unwrap();
        assert_eq!(address, fbpt.fbpt_address);

        fbpt.add_record(DualGuidStringEventRecord::new(1, 0, 10, guid, guid, "test")).unwrap();
        fbpt.add_record(GuidQwordEventRecord::new(1, 0, 10, guid, 64)).unwrap();
        fbpt.add_record(GuidQwordStringEventRecord::new(1, 0, 10, guid, 64, "test")).unwrap();

        for (i, record) in fbpt.perf_records().iter().enumerate() {
            match i {
                _ if i == 0 => assert_eq!(
                    (GuidEventRecord::TYPE, GuidEventRecord::REVISION),
                    (record.record_type, record.revision)
                ),
                _ if i == 1 => assert_eq!(
                    (DynamicStringEventRecord::TYPE, DynamicStringEventRecord::REVISION),
                    (record.record_type, record.revision)
                ),
                _ if i == 2 => assert_eq!(
                    (DualGuidStringEventRecord::TYPE, DualGuidStringEventRecord::REVISION),
                    (record.record_type, record.revision)
                ),
                _ if i == 3 => assert_eq!(
                    (GuidQwordEventRecord::TYPE, GuidQwordEventRecord::REVISION),
                    (record.record_type, record.revision)
                ),
                _ if i == 4 => assert_eq!(
                    (GuidQwordStringEventRecord::TYPE, GuidQwordStringEventRecord::REVISION),
                    (record.record_type, record.revision)
                ),
                _ => unreachable!(),
            }
        }

        assert_eq!(&273, fbpt.length());
    }

    #[test]
    fn test_reporting_fbpt_without_previous_address() {
        let memory_buffer = Vec::<u8>::with_capacity(1000);
        let address = memory_buffer.as_ptr() as usize;

        let mut boot_services = MockBootServices::new();
        boot_services
            .expect_allocate_pages()
            .once()
            .withf(move |alloc_type, memory_type, _| {
                assert_eq!(&AllocType::MaxAddress(u32::MAX as usize), alloc_type);
                assert_eq!(&EfiMemoryType::ReservedMemoryType, memory_type);
                true
            })
            .returning(move |_, _, _| Ok(address));

        let mut fbpt = FBPT::new();
        let guid = efi::Guid::from_bytes(&[0; 16]);
        fbpt.add_record(GuidEventRecord::new(1, 0, 10, guid)).unwrap();
        fbpt.add_record(DynamicStringEventRecord::new(1, 0, 10, guid, "test")).unwrap();

        fbpt.report_table(None, &boot_services).unwrap();
        assert_eq!(address, fbpt.fbpt_address());

        fbpt.add_record(DualGuidStringEventRecord::new(1, 0, 10, guid, guid, "test")).unwrap();
        fbpt.add_record(GuidQwordEventRecord::new(1, 0, 10, guid, 64)).unwrap();
        fbpt.add_record(GuidQwordStringEventRecord::new(1, 0, 10, guid, 64, "test")).unwrap();
    }

    #[test]
    fn test_performance_table_well_written_in_memory() {
        let memory_buffer = Vec::<u8>::with_capacity(1000);
        let address = memory_buffer.as_ptr() as usize;

        let mut boot_services = MockBootServices::new();
        boot_services
            .expect_allocate_pages()
            .once()
            .withf(move |_, memory_type, _| {
                assert_eq!(&EfiMemoryType::ReservedMemoryType, memory_type);
                true
            })
            .returning(move |_, _, _| Ok(address));

        let mut fbpt = FBPT::new();
        let guid = efi::Guid::from_bytes(&[0; 16]);
        fbpt.add_record(GuidEventRecord::new(1, 0, 10, guid)).unwrap();
        fbpt.add_record(DynamicStringEventRecord::new(1, 0, 10, guid, "test")).unwrap();

        fbpt.report_table(Some(address), &boot_services).unwrap();
        assert_eq!(address, fbpt.fbpt_address());

        fbpt.add_record(DualGuidStringEventRecord::new(1, 0, 10, guid, guid, "test")).unwrap();
        fbpt.add_record(GuidQwordEventRecord::new(1, 0, 10, guid, 64)).unwrap();
        fbpt.add_record(GuidQwordStringEventRecord::new(1, 0, 10, guid, 64, "test")).unwrap();

        let buffer = unsafe { slice::from_raw_parts(fbpt.fbpt_address() as *const u8, 1000) };

        let mut offset = 0;
        let signature = buffer.gread_with::<u32>(&mut offset, scroll::NATIVE).unwrap();
        assert_eq!(FBPT::SIGNATURE, signature);
        let length = buffer.gread_with::<u32>(&mut offset, scroll::NATIVE).unwrap();
        assert_eq!(fbpt.length(), &length);
        let record_type = buffer.gread_with::<u16>(&mut offset, scroll::NATIVE).unwrap();
        let record_length = buffer.gread_with::<u8>(&mut offset, scroll::NATIVE).unwrap();
        let record_revision = buffer.gread_with::<u8>(&mut offset, scroll::NATIVE).unwrap();
        assert_eq!(FirmwareBasicBootPerfDataRecord::TYPE, record_type);
        assert_eq!(
            PERFORMANCE_RECORD_HEADER_SIZE + FirmwareBasicBootPerfDataRecord::data_size(),
            record_length as usize
        );
        assert_eq!(FirmwareBasicBootPerfDataRecord::REVISION, record_revision);
        offset += FirmwareBasicBootPerfDataRecord::data_size();
        assert_eq!(fbpt.perf_records().buffer().as_ptr() as usize, address + offset);
    }
}
