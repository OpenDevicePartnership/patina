//! Defines the Firmware Basic Boot Performance Table (FBPT) owned by the DXE Core.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    mem, ptr,
    sync::atomic::{AtomicPtr, Ordering},
};

use patina::performance::{
    self,
    error::Error,
    record::{PerformanceRecord, PerformanceRecordBuffer},
};

use scroll::Pwrite;

/// The number of extra space in byte that will be allocated when publishing the performance buffer.
/// This is used for every performance records that will be added to the table after it is published.
const PUBLISHED_FBPT_EXTRA_SPACE: usize = 0x40_000;

/// Firmware Basic Boot Performance Table (FBPT)
#[derive(Debug)]
pub(crate) struct Fbpt {
    /// When the table will be reported, this will be the address where the fbpt table is.
    fbpt_address: usize,
    /// First value is the length when the table is not been reported and the second one is when the table is reported.
    /// Use `length()` or `length_mut()`. Do not use this field directly.
    length: (u32, AtomicPtr<u32>),
    /// Buffer containing all the performance record.
    other_records: PerformanceRecordBuffer,
}

impl Fbpt {
    /// FBPT - Firmware Basic Boot Performance Table signature
    pub const SIGNATURE: u32 = patina::signature!('F', 'B', 'P', 'T');

    /// Create an new empty FBPT.
    pub const fn new() -> Self {
        Self {
            fbpt_address: 0,
            length: (Self::size_of_empty_table() as u32, AtomicPtr::new(ptr::null_mut())),
            other_records: PerformanceRecordBuffer::new(),
        }
    }

    /// Return the size in bytes of the FBPT table.
    pub fn length(&self) -> &u32 {
        // SAFETY: `length` is a valid field in the FBPT structure.
        unsafe { self.length.1.load(Ordering::Relaxed).as_ref() }.unwrap_or(&self.length.0)
    }

    fn length_mut(&mut self) -> &mut u32 {
        // SAFETY: `length` is a valid field in the FBPT structure.
        unsafe { self.length.1.load(Ordering::Relaxed).as_mut() }.unwrap_or(&mut self.length.0)
    }

    const fn size_of_empty_table() -> usize {
        mem::size_of::<u32>() // Header signature
        + mem::size_of::<u32>() // Header length
        + performance::record::PERFORMANCE_RECORD_HEADER_SIZE
        + FirmwareBasicBootPerfDataRecord::data_size()
    }
}

impl Fbpt {
    /// Return the address where the table is.
    #[cfg(test)]
    pub fn fbpt_address(&self) -> usize {
        self.fbpt_address
    }

    /// Return every performance records that has been added to the table.
    #[cfg(test)]
    pub fn perf_records(&self) -> &PerformanceRecordBuffer {
        &self.other_records
    }

    /// Initialize the performance records.
    pub fn set_perf_records(&mut self, perf_records: PerformanceRecordBuffer) {
        *self.length_mut() = (Self::size_of_empty_table() + perf_records.size()) as u32;
        self.other_records = perf_records;
    }

    /// Add a performance record to the table.
    pub fn add_record<T: PerformanceRecord>(&mut self, record: T) -> Result<(), Error> {
        let record_size = self.other_records.push_record(record)?;
        *self.length_mut() += record_size as u32;
        Ok(())
    }

    /// Returns the number of bytes that must be allocated to publish the table.
    pub fn published_table_size(&self) -> usize {
        Self::size_of_empty_table() + self.other_records.size() + PUBLISHED_FBPT_EXTRA_SPACE
    }

    /// Returns [`Error::BufferTooSmall`] if `buffer` is not large enough to hold the table.
    pub fn publish_table(&mut self, buffer: &'static mut [u8]) -> Result<usize, Error> {
        self.fbpt_address = buffer.as_ptr() as usize;

        let mut offset = 0;
        buffer.gwrite(Self::SIGNATURE, &mut offset).map_err(|_| Error::BufferTooSmall)?;
        // SAFETY: The caller guarantees `buffer` is at least `published_table_size()` bytes.
        let length_ptr = unsafe { buffer.as_ptr().byte_add(offset) } as *mut u32;
        buffer.gwrite(*self.length(), &mut offset).map_err(|_| Error::BufferTooSmall)?;
        FirmwareBasicBootPerfDataRecord::new().write_into(buffer, &mut offset).map_err(|_| Error::BufferTooSmall)?;

        debug_assert_eq!(Self::size_of_empty_table(), offset);
        self.other_records.report(buffer.get_mut(offset..).ok_or(Error::BufferTooSmall)?)?;

        self.length.1.store(length_ptr, Ordering::Relaxed);
        Ok(self.fbpt_address)
    }
}

impl Default for Fbpt {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
#[repr(C)]
/// Firmware Basic Boot Performance Record
pub(crate) struct FirmwareBasicBootPerfDataRecord {
    /// Timer value logged at the beginning of firmware image execution. This may not always be zero or near zero.
    pub reset_end: u64,
    /// Timer value logged just prior to loading the OS boot loader into memory. For non-UEFI compatible boots, this field must be zero.
    pub os_loader_load_image_start: u64,
    /// Timer value logged just prior to launching the currently loaded OS boot loader image.
    /// For non-UEFI compatible boots, the timer value logged will be just prior to the INT 19h handler invocation.
    pub os_loader_start_image_start: u64,
    /// Timer value logged at the point when the OS loader calls the `ExitBootServices` function for UEFI compatible firmware.
    /// For non-UEFI compatible boots, this field must be zero.
    pub exit_boot_services_entry: u64,
    /// Timer value logged at the point just prior to the OS loader gaining control back from the
    /// `ExitBootServices` function for UEFI compatible firmware.
    /// For non-UEFI compatible boots, this field must be zero.
    pub exit_boot_services_exit: u64,
}

impl FirmwareBasicBootPerfDataRecord {
    const TYPE: u16 = 2;
    const REVISION: u8 = 2;

    /// Create a new empty `FirmwareBasicBootPerfDataRecord`.
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

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), Error> {
        if buff.gwrite_with([0_u8; 4], offset, scroll::NATIVE).is_err()
            || buff.gwrite_with(self.reset_end, offset, scroll::NATIVE).is_err()
            || buff.gwrite_with(self.os_loader_load_image_start, offset, scroll::NATIVE).is_err()
            || buff.gwrite_with(self.os_loader_start_image_start, offset, scroll::NATIVE).is_err()
            || buff.gwrite_with(self.exit_boot_services_entry, offset, scroll::NATIVE).is_err()
            || buff.gwrite_with(self.exit_boot_services_exit, offset, scroll::NATIVE).is_err()
        {
            return Err(Error::Serialization);
        }
        Ok(())
    }
}

impl Default for FirmwareBasicBootPerfDataRecord {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    use core::{assert_eq, slice, unreachable};
    use scroll::Pread;

    use patina::performance::record::{
        PERFORMANCE_RECORD_HEADER_SIZE,
        extended::{
            DualGuidStringEventRecord, DynamicStringEventRecord, GuidEventRecord, GuidQwordEventRecord,
            GuidQwordStringEventRecord,
        },
    };

    #[test]
    fn test_set_perf_records() {
        let mut performance_record_buffer = PerformanceRecordBuffer::new();
        crate::performance::push_generic_record(&mut performance_record_buffer, 1, 1, &[0_u8; 16]);

        let mut fbpt = Fbpt::new();
        assert_eq!(&56, fbpt.length());

        fbpt.set_perf_records(performance_record_buffer);
        assert_eq!(&76, fbpt.length());
    }

    #[test]
    fn test_serialize_into() {
        let buffer: &'static mut [u8] = std::boxed::Box::leak(std::vec![0u8; 1000].into_boxed_slice());
        let address = buffer.as_ptr() as usize;

        let mut fbpt = Fbpt::new();
        let guid = patina::BinaryGuid::ZERO;
        fbpt.add_record(GuidEventRecord::new(1, 0, 10, guid)).unwrap();
        fbpt.add_record(DynamicStringEventRecord::new(1, 0, 10, guid, "test")).unwrap();

        fbpt.publish_table(buffer).unwrap();
        assert_eq!(address, fbpt.fbpt_address);

        fbpt.add_record(DualGuidStringEventRecord::new(1, 0, 10, guid, guid, "test")).unwrap();
        fbpt.add_record(GuidQwordEventRecord::new(1, 0, 10, guid, 64)).unwrap();
        fbpt.add_record(GuidQwordStringEventRecord::new(1, 0, 10, guid, 64, "test")).unwrap();

        for (i, record) in fbpt.perf_records().iter().enumerate() {
            let actual = (record.header.record_type, record.header.revision);
            match i {
                _ if i == 0 => assert_eq!((GuidEventRecord::TYPE, GuidEventRecord::REVISION), actual),
                _ if i == 1 => {
                    assert_eq!((DynamicStringEventRecord::TYPE, DynamicStringEventRecord::REVISION), actual);
                }
                _ if i == 2 => {
                    assert_eq!((DualGuidStringEventRecord::TYPE, DualGuidStringEventRecord::REVISION), actual);
                }
                _ if i == 3 => assert_eq!((GuidQwordEventRecord::TYPE, GuidQwordEventRecord::REVISION), actual),
                _ if i == 4 => {
                    assert_eq!((GuidQwordStringEventRecord::TYPE, GuidQwordStringEventRecord::REVISION), actual);
                }
                _ => unreachable!(),
            }
        }

        assert_eq!(&273, fbpt.length());
    }

    #[test]
    fn test_published_table_size() {
        let mut fbpt = Fbpt::new();
        let empty_size = fbpt.published_table_size();

        let guid = patina::BinaryGuid::ZERO;
        fbpt.add_record(GuidEventRecord::new(1, 0, 10, guid)).unwrap();
        fbpt.add_record(DynamicStringEventRecord::new(1, 0, 10, guid, "test")).unwrap();

        // The required publishing size grows by exactly the size of the buffered records.
        assert_eq!(empty_size + fbpt.perf_records().size(), fbpt.published_table_size());
    }

    #[test]
    fn test_performance_table_well_written_in_memory() {
        let buffer: &'static mut [u8] = std::boxed::Box::leak(std::vec![0u8; 1000].into_boxed_slice());
        let address = buffer.as_ptr() as usize;

        let mut fbpt = Fbpt::new();
        let guid = patina::BinaryGuid::ZERO;
        fbpt.add_record(GuidEventRecord::new(1, 0, 10, guid)).unwrap();
        fbpt.add_record(DynamicStringEventRecord::new(1, 0, 10, guid, "test")).unwrap();

        fbpt.publish_table(buffer).unwrap();
        assert_eq!(address, fbpt.fbpt_address());

        fbpt.add_record(DualGuidStringEventRecord::new(1, 0, 10, guid, guid, "test")).unwrap();
        fbpt.add_record(GuidQwordEventRecord::new(1, 0, 10, guid, 64)).unwrap();
        fbpt.add_record(GuidQwordStringEventRecord::new(1, 0, 10, guid, 64, "test")).unwrap();

        // SAFETY: Test code - creating a slice from the FBPT address for validation.
        let buffer = unsafe { slice::from_raw_parts(fbpt.fbpt_address() as *const u8, 1000) };

        let mut offset = 0;
        let signature = buffer.gread_with::<u32>(&mut offset, scroll::NATIVE).unwrap();
        assert_eq!(Fbpt::SIGNATURE, signature);
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
