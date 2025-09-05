//! Defines performance record and the performance record buffer types.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub mod extended;
pub mod hob;
pub mod known;

use crate::{performance::error::Error, performance_debug_assert};
use alloc::vec::Vec;
use core::{fmt::Debug, mem, ops::AddAssign};
use scroll::{self, Pread, Pwrite};

/// Maximum size in byte that a performance record can have.
pub const FPDT_MAX_PERF_RECORD_SIZE: usize = u8::MAX as usize;

/// Size in byte of the reader of a performance record.
pub const PERFORMANCE_RECORD_HEADER_SIZE: usize = mem::size_of::<u16>() // Type
        + mem::size_of::<u8>() // Length
        + mem::size_of::<u8>(); // Revision

/// Common behavior of every performance records.
pub trait PerformanceRecord {
    /// returns the type ID (NOT Rust's `TypeId`) value of the record
    fn record_type(&self) -> u16;

    /// Returns the revision of the record.
    fn revision(&self) -> u8;

    /// Write the record data into the buffer.
    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), scroll::Error>;

    /// Write the record data and the header into the buffer.
    fn write_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<usize, scroll::Error> {
        let mut writing_offset = *offset;

        // Write performance record header.
        buff.gwrite(self.record_type(), &mut writing_offset)?;
        let record_size_offset = writing_offset;
        buff.gwrite(0_u8, &mut writing_offset)?;
        buff.gwrite(self.revision(), &mut writing_offset)?;

        // Write data.
        self.write_data_into(buff, &mut writing_offset)?;

        let record_size = writing_offset - *offset;

        // Write record size
        buff.pwrite(record_size as u8, record_size_offset)?;

        offset.add_assign(record_size);

        Ok(record_size)
    }
}

/// Performance record used to store any specific type of record.
#[derive(Debug)]
pub struct GenericPerformanceRecord<T: AsRef<[u8]>> {
    /// This value depicts the format and contents of the performance record.
    pub record_type: u16,
    /// This value depicts the length of the performance record, in bytes.
    pub length: u8,
    /// This value is updated if the format of the record type is extended.
    /// Any changes to a performance record layout must be backwards-compatible
    /// in that all previously defined fields must be maintained if still applicable,
    /// but newly defined fields allow the length of the performance record to be increased.
    /// Previously defined record fields must not be redefined, but are permitted to be deprecated.
    pub revision: u8,
    /// The underlying data of the specific performance record.
    pub data: T,
}

impl<T: AsRef<[u8]>> PerformanceRecord for GenericPerformanceRecord<T> {
    fn record_type(&self) -> u16 {
        self.record_type
    }

    fn revision(&self) -> u8 {
        self.revision
    }

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), scroll::Error> {
        buff.gwrite_with(self.data.as_ref(), offset, ())?;
        Ok(())
    }
}

/// Performance record buffer that can be used to collect performance records
pub enum PerformanceRecordBuffer {
    /// Unpublished state, where records can be added and the enum owns the buffer.
    Unpublished(Vec<u8>),
    /// Published state, where the buffer is leaked to it's final destination.
    Published(&'static mut [u8], usize),
}

impl PerformanceRecordBuffer {
    /// Create a new performance record buffer in unpublished state.
    pub const fn new() -> Self {
        Self::Unpublished(Vec::new())
    }

    /// Add a performance record to the buffer.
    pub fn push_record<T: PerformanceRecord>(&mut self, record: T) -> Result<usize, Error> {
        match self {
            Self::Unpublished(buffer) => {
                let mut offset = buffer.len();
                buffer.resize(offset + FPDT_MAX_PERF_RECORD_SIZE, 0);
                let Ok(record_size) = record.write_into(buffer, &mut offset) else {
                    return performance_debug_assert!("Record size should not exceed FPDT_MAX_PERF_RECORD_SIZE");
                };
                buffer.truncate(offset);
                Ok(record_size)
            }
            Self::Published(buffer, offset) => record.write_into(buffer, offset).map_err(|_| Error::OutOfResources),
        }
    }

    /// Move the performance buffer into the memory buffer given as an argument and put itself in a publish state.
    pub fn report(&mut self, buffer: &'static mut [u8]) -> Result<(), Error> {
        let current_buffer = match self {
            PerformanceRecordBuffer::Unpublished(b) => b.as_slice(),
            PerformanceRecordBuffer::Published(_, _) => {
                return performance_debug_assert!("PerformanceRecordBuffer already reported.");
            }
        };
        let size = current_buffer.len();
        if buffer.len() < size {
            return Err(Error::BufferTooSmall);
        }
        buffer[..size].clone_from_slice(current_buffer);
        *self = Self::Published(buffer, size);
        Ok(())
    }

    /// Return a reference to the performance buffer in bytes.
    pub fn buffer(&self) -> &[u8] {
        match &self {
            Self::Unpublished(b) => b.as_slice(),
            Self::Published(b, len) => &b[..*len],
        }
    }

    /// Return a performance record iterator.
    pub fn iter(&self) -> Iter<'_> {
        Iter::new(self.buffer())
    }

    /// Return the size in bytes of the buffer.
    pub fn size(&self) -> usize {
        match &self {
            Self::Unpublished(b) => b.len(),
            Self::Published(_, len) => *len,
        }
    }

    /// Return the capacity in bytes of the buffer.
    pub fn capacity(&self) -> usize {
        match &self {
            Self::Unpublished(b) => b.capacity(),
            Self::Published(b, _) => b.len(),
        }
    }
}

impl scroll::ctx::TryIntoCtx<scroll::Endian> for PerformanceRecordBuffer {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], _ctx: scroll::Endian) -> Result<usize, Self::Error> {
        dest.pwrite_with(self.buffer(), 0, ())
    }
}

impl Default for PerformanceRecordBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Debug for PerformanceRecordBuffer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let size = self.size();
        let capacity = self.capacity();
        let nb_report = self.iter().count();
        let records = self.iter().collect::<Vec<_>>();
        f.debug_struct("PerformanceRecordBuffer")
            .field("size", &size)
            .field("capacity", &capacity)
            .field("nb_report", &nb_report)
            .field("records", &records)
            .finish()
    }
}

/// Performance record iterator.
pub struct Iter<'a> {
    buffer: &'a [u8],
}

impl<'a> Iter<'a> {
    /// Iterate through performance records in a memory buffer. The buffer must contains valid records.
    pub fn new(buffer: &'a [u8]) -> Self {
        Self { buffer }
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = GenericPerformanceRecord<&'a [u8]>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.buffer.is_empty() {
            return None;
        }
        let mut offset = 0;
        let record_type = self.buffer.gread::<u16>(&mut offset).unwrap();
        let length = self.buffer.gread::<u8>(&mut offset).unwrap();
        let revision = self.buffer.gread::<u8>(&mut offset).unwrap();

        let data = &self.buffer[offset..length as usize];
        self.buffer = &self.buffer[length as usize..];
        Some(GenericPerformanceRecord { record_type, length, revision, data })
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use core::{assert_eq, slice, unreachable};

    use r_efi::efi;

    use extended::{
        DualGuidStringEventRecord, DynamicStringEventRecord, GuidEventRecord, GuidQwordEventRecord,
        GuidQwordStringEventRecord,
    };

    #[test]
    fn test_performance_record_buffer_new() {
        let performance_record_buffer = PerformanceRecordBuffer::new();
        println!("{performance_record_buffer:?}");
        assert_eq!(0, performance_record_buffer.size());
    }

    #[test]
    fn test_performance_record_buffer_push_record() {
        let guid = efi::Guid::from_bytes(&[0; 16]);
        let mut performance_record_buffer = PerformanceRecordBuffer::new();
        let mut size = 0;

        size += performance_record_buffer.push_record(GuidEventRecord::new(1, 0, 10, guid)).unwrap();
        assert_eq!(size, performance_record_buffer.size());

        size += performance_record_buffer.push_record(DynamicStringEventRecord::new(1, 0, 10, guid, "test")).unwrap();
        assert_eq!(size, performance_record_buffer.size());

        size += performance_record_buffer
            .push_record(DualGuidStringEventRecord::new(1, 0, 10, guid, guid, "test"))
            .unwrap();
        assert_eq!(size, performance_record_buffer.size());

        size += performance_record_buffer.push_record(GuidQwordEventRecord::new(1, 0, 10, guid, 64)).unwrap();
        assert_eq!(size, performance_record_buffer.size());

        size +=
            performance_record_buffer.push_record(GuidQwordStringEventRecord::new(1, 0, 10, guid, 64, "test")).unwrap();
        assert_eq!(size, performance_record_buffer.size());
    }

    #[test]
    fn test_performance_record_buffer_iter() {
        let guid = efi::Guid::from_bytes(&[0; 16]);
        let mut performance_record_buffer = PerformanceRecordBuffer::new();

        performance_record_buffer.push_record(GuidEventRecord::new(1, 0, 10, guid)).unwrap();
        performance_record_buffer.push_record(DynamicStringEventRecord::new(1, 0, 10, guid, "test")).unwrap();
        performance_record_buffer.push_record(DualGuidStringEventRecord::new(1, 0, 10, guid, guid, "test")).unwrap();
        performance_record_buffer.push_record(GuidQwordEventRecord::new(1, 0, 10, guid, 64)).unwrap();
        performance_record_buffer.push_record(GuidQwordStringEventRecord::new(1, 0, 10, guid, 64, "test")).unwrap();

        for (i, record) in performance_record_buffer.iter().enumerate() {
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
    }

    #[test]
    fn test_performance_record_buffer_reported_table() {
        let guid = efi::Guid::from_bytes(&[0; 16]);
        let mut performance_record_buffer = PerformanceRecordBuffer::new();

        performance_record_buffer.push_record(GuidEventRecord::new(1, 0, 10, guid)).unwrap();
        performance_record_buffer.push_record(DynamicStringEventRecord::new(1, 0, 10, guid, "test")).unwrap();

        let mut buffer = vec![0_u8; 1000];
        let buffer = unsafe { slice::from_raw_parts_mut(buffer.as_mut_ptr(), buffer.len()) };

        performance_record_buffer.report(buffer).unwrap();

        performance_record_buffer.push_record(DualGuidStringEventRecord::new(1, 0, 10, guid, guid, "test")).unwrap();
        performance_record_buffer.push_record(GuidQwordEventRecord::new(1, 0, 10, guid, 64)).unwrap();
        performance_record_buffer.push_record(GuidQwordStringEventRecord::new(1, 0, 10, guid, 64, "test")).unwrap();

        for (i, record) in performance_record_buffer.iter().enumerate() {
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
    }
}
