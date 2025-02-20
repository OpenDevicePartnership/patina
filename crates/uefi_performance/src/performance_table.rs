//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use alloc::vec::Vec;
use core::{
    fmt::Debug,
    mem, ptr, slice,
    sync::atomic::{AtomicPtr, Ordering},
};

use r_efi::efi;
use scroll::Pwrite;

use uefi_sdk::{
    base::UEFI_PAGE_SIZE,
    boot_services::{
        allocation::{AllocType, MemoryType},
        BootServices,
    },
    runtime_services::RuntimeServices,
};

use crate::performance_record::{self, PerformanceRecord, PerformanceRecordBuffer};

const PUBLISHED_FBPT_EXTRA_SPACE: usize = 0x10_000;

#[derive(Debug, Clone, Pwrite)]
#[repr(C)]
pub struct PerformanceTableHeader {
    pub signature: u32,
    pub length: u32,
}

/// Firmware Basic Boot Performance Table (FBPT)
pub struct FBPT {
    /// When the table will be reported, this will be the address where the fbpt table is.
    fbpt_address: usize,
    /// First value is the length when the table is not been reported and the second one is when the table is reported.
    /// Use `length()` or `length_mut()`. Do now use this field directly.
    _length: (u32, AtomicPtr<u32>),
    /// Buffer containing all the performance record.
    other_records: PerformanceRecordBuffer,
}

impl FBPT {
    pub const SIGNATURE: u32 = u32::from_le_bytes([b'F', b'B', b'P', b'T']);

    pub const fn new() -> Self {
        Self {
            fbpt_address: 0,
            _length: (Self::size_of_empty_table() as u32, AtomicPtr::new(ptr::null_mut())),
            other_records: PerformanceRecordBuffer::new(),
        }
    }

    pub fn set_records(&mut self, records: PerformanceRecordBuffer) {
        *self.length_mut() += records.size() as u32;
        self.other_records = records;
    }

    pub fn add_record(&mut self, record: impl PerformanceRecord) -> Result<(), efi::Status> {
        let record_size = self.other_records.push_record(record)?;
        *self.length_mut() += record_size as u32;
        Ok(())
    }

    /// Report table allocate new space of memory and move the table to a specific place so it can be found later.
    /// Additional memory is allocated so the table can still grow in the future step.
    pub fn report_table(
        &mut self,
        boot_services: &impl BootServices,
        runtime_services: &impl RuntimeServices,
    ) -> Result<(), efi::Status> {
        const ADDRESS_VARIABLE_GUID: efi::Guid =
            efi::Guid::from_fields(0xc095791a, 0x3001, 0x47b2, 0x80, 0xc9, &[0xea, 0xc7, 0x31, 0x9f, 0x2f, 0xa4]);

        let allocation_size = Self::size_of_empty_table() + self.other_records.size() + PUBLISHED_FBPT_EXTRA_SPACE;
        let allocation_nb_page = allocation_size.div_ceil(UEFI_PAGE_SIZE);
        let allocation_size = allocation_nb_page * UEFI_PAGE_SIZE;

        self.fbpt_address = 'find_address: {
            if let Ok(prev_address) = {
                struct FirmwarePerformanceVariable {
                    boot_performance_table_pointer: usize,
                    _s3_performance_table_pointer: usize,
                }
                impl TryFrom<Vec<u8>> for FirmwarePerformanceVariable {
                    type Error = ();

                    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
                        if value.len() == mem::size_of::<Self>() {
                            Ok(unsafe { ptr::read_unaligned(value.as_ptr() as *const FirmwarePerformanceVariable) })
                        } else {
                            Err(())
                        }
                    }
                }

                runtime_services
                    .get_variable::<FirmwarePerformanceVariable>(
                        &[0],
                        &ADDRESS_VARIABLE_GUID,
                        Some(mem::size_of::<FirmwarePerformanceVariable>()),
                    )
                    .map(|(v, _)| v.boot_performance_table_pointer)
            } {
                if let Ok(prev_address) = boot_services.allocate_pages(
                    AllocType::Address(prev_address),
                    MemoryType::RESERVED_MEMORY_TYPE,
                    allocation_nb_page,
                ) {
                    break 'find_address prev_address;
                }
            }
            boot_services.allocate_pages(
                AllocType::MaxAddress(u32::MAX as usize),
                MemoryType::RESERVED_MEMORY_TYPE,
                allocation_nb_page,
            )?
        };
        let fbpt_ptr = self.fbpt_address as *mut u8;

        let fbpt_buffer = unsafe { slice::from_raw_parts_mut(fbpt_ptr, allocation_size) };

        let mut offset = 0;
        fbpt_buffer.gwrite(Self::SIGNATURE, &mut offset).unwrap();
        let length_ptr = unsafe { fbpt_ptr.byte_add(offset) } as *mut u32;
        fbpt_buffer.gwrite(*self.length(), &mut offset).unwrap();
        FirmwareBasicBootPerfDataRecord::new().write_into(fbpt_buffer, &mut offset).unwrap();

        debug_assert_eq!(Self::size_of_empty_table(), offset);
        self.other_records.report(&mut fbpt_buffer[offset..]);

        self._length.1.store(length_ptr, Ordering::Relaxed);
        Ok(())
    }

    pub fn length(&self) -> &u32 {
        unsafe { self._length.1.load(Ordering::Relaxed).as_ref() }.unwrap_or(&self._length.0)
    }

    pub fn length_mut(&mut self) -> &mut u32 {
        unsafe { self._length.1.load(Ordering::Relaxed).as_mut() }.unwrap_or(&mut self._length.0)
    }

    pub fn other_records(&self) -> &PerformanceRecordBuffer {
        &self.other_records
    }

    pub fn fbpt_address(&self) -> usize {
        self.fbpt_address
    }

    pub const fn size_of_empty_table() -> usize {
        mem::size_of::<u32>() // Header signature
        + mem::size_of::<u32>() // Header length
        + performance_record::PERFORMANCE_RECORD_HEADER_SIZE
        + FirmwareBasicBootPerfDataRecord::data_size()
    }
}

impl Default for FBPT {
    fn default() -> Self {
        Self::new()
    }
}

impl Debug for FBPT {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let record_count = self.other_records.iter().count();
        f.debug_struct("FBPT")
            .field("fbpt_address", &(self.fbpt_address as *const u8))
            .field("length", self.length())
            .field("other_records::size", &self.other_records.size())
            .field("other_records::capacity", &self.other_records.capacity())
            .field("other_records::count", &record_count)
            .finish()
    }
}

#[derive(Clone)]
#[repr(C)]
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

    pub const fn new() -> Self {
        Self {
            reset_end: 0,
            os_loader_load_image_start: 0,
            os_loader_start_image_start: 0,
            exit_boot_services_entry: 0,
            exit_boot_services_exit: 0,
        }
    }

    pub const fn data_size() -> usize {
        4 // Reserved bytes
        + mem::size_of::<Self>()
    }
}

impl scroll::ctx::TryIntoCtx<scroll::Endian> for FirmwareBasicBootPerfDataRecord {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        let mut offset = 0;
        dest.gwrite_with([0_u8; 4], &mut offset, ctx)?; // Reserved bytes
        dest.gwrite_with(self.reset_end, &mut offset, ctx)?;
        dest.gwrite_with(self.os_loader_load_image_start, &mut offset, ctx)?;
        dest.gwrite_with(self.os_loader_start_image_start, &mut offset, ctx)?;
        dest.gwrite_with(self.exit_boot_services_entry, &mut offset, ctx)?;
        dest.gwrite_with(self.exit_boot_services_exit, &mut offset, ctx)?;
        Ok(offset)
    }
}

impl PerformanceRecord for FirmwareBasicBootPerfDataRecord {
    fn record_type(&self) -> u16 {
        Self::TYPE
    }

    fn revision(&self) -> u8 {
        Self::REVISION
    }
}

impl Default for FirmwareBasicBootPerfDataRecord {
    fn default() -> Self {
        Self::new()
    }
}
