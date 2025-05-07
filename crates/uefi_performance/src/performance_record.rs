//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

pub mod extended;
pub mod known_records;

use alloc::vec::Vec;
use core::{fmt::Debug, mem, ops::Deref};

use r_efi::efi;
use scroll::{self, Pread, Pwrite};

use crate::_debug::DbgMemory;

pub const FPDT_MAX_PERF_RECORD_SIZE: usize = u8::MAX as usize;

pub const PERFORMANCE_RECORD_HEADER_SIZE: usize = mem::size_of::<u16>() // Type
        + mem::size_of::<u8>() // Length 
        + mem::size_of::<u8>(); // Revision

pub trait PerformanceRecord: Sized + scroll::ctx::TryIntoCtx<scroll::Endian, Error = scroll::Error> {
    fn record_type(&self) -> u16;

    fn revision(&self) -> u8;

    fn write_into(self, buff: &mut [u8], offset: &mut usize) -> Result<usize, scroll::Error> {
        let mut record_size = 0;

        // Write performance record header.
        record_size += buff.gwrite(self.record_type(), offset)?;
        let mut record_size_offset = *offset;
        record_size += buff.gwrite(0_u8, offset)?;
        record_size += buff.gwrite(self.revision(), offset)?;

        // Write data.
        record_size += buff.gwrite(self, offset)?;

        // Write record size
        buff.gwrite(record_size as u8, &mut record_size_offset)?;

        Ok(record_size)
    }
}

pub struct GenericPerformanceRecord<T: Deref<Target = [u8]>> {
    // This value depicts the format and contents of the performance record.
    pub record_type: u16,
    /// This value depicts the length of the performance record, in bytes.
    pub length: u8,
    /// This value is updated if the format of the record type is extended.
    /// Any changes to a performance record layout must be backwards-compatible
    /// in that all previously defined fields must be maintained if still applicable,
    /// but newly defined fields allow the length of the performance record to be increased.
    /// Previously defined record fields must not be redefined, but are permitted to be deprecated.
    pub revision: u8,
    data: T,
}

impl<T: Deref<Target = [u8]>> scroll::ctx::TryIntoCtx<scroll::Endian> for GenericPerformanceRecord<T> {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], _ctx: scroll::Endian) -> Result<usize, Self::Error> {
        let mut offset = 0;
        dest.gwrite_with(self.data.deref(), &mut offset, ())?;
        Ok(offset)
    }
}

impl<T: Deref<Target = [u8]>> PerformanceRecord for GenericPerformanceRecord<T> {
    fn record_type(&self) -> u16 {
        self.record_type
    }

    fn revision(&self) -> u8 {
        self.revision
    }
}

impl<T: Deref<Target = [u8]>> Debug for GenericPerformanceRecord<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GenericPerformanceRecord")
            .field("record_type", &self.record_type)
            .field("length", &self.length)
            .field("revision", &self.revision)
            .field("data", &DbgMemory(&self.data))
            .finish()
    }
}

pub enum PerformanceRecordBuffer {
    Unpublished(Vec<u8>),
    Published(&'static mut [u8], usize),
}

impl PerformanceRecordBuffer {
    pub const fn new() -> Self {
        Self::Unpublished(Vec::new())
    }

    pub fn push_record<T: PerformanceRecord>(&mut self, record: T) -> Result<usize, efi::Status> {
        match self {
            Self::Unpublished(buffer) => {
                let mut offset = buffer.len();
                buffer.resize(offset + FPDT_MAX_PERF_RECORD_SIZE, 0);
                let record_size = record
                    .write_into(buffer, &mut offset)
                    .expect("Record size should not exceed FPDT_MAX_PERF_RECORD_SIZE");
                buffer.truncate(offset);
                Ok(record_size)
            }
            Self::Published(buffer, offset) => {
                record.write_into(buffer, offset).map_err(|_| efi::Status::OUT_OF_RESOURCES)
            }
        }
    }

    pub fn report(&mut self, buffer: &'static mut [u8]) {
        let current_buffer = match self {
            PerformanceRecordBuffer::Unpublished(b) => b.as_slice(),
            PerformanceRecordBuffer::Published(_, _) => panic!("PerformanceRecordBuffer already reported."),
        };
        let size = current_buffer.len();
        buffer[..size].clone_from_slice(current_buffer);
        *self = Self::Published(buffer, size);
    }

    pub fn buffer(&self) -> &[u8] {
        match &self {
            Self::Unpublished(b) => b.as_slice(),
            Self::Published(b, len) => &b[..*len],
        }
    }

    pub fn iter(&self) -> Iter {
        Iter::new(self.buffer())
    }

    pub fn size(&self) -> usize {
        match &self {
            Self::Unpublished(b) => b.len(),
            Self::Published(_, len) => *len,
        }
    }

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
        let _is_published = match self {
            Self::Unpublished(_) => true,
            Self::Published(_, _) => false,
        };
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

pub struct Iter<'a> {
    buffer: &'a [u8],
}

impl<'a> Iter<'a> {
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
mod test {
    use core::{assert_eq, slice};

    use crate::performance_record::extended::{
        DualGuidStringEventRecord, DynamicStringEventRecord, GuidEventRecord, GuidQwordEventRecord,
        GuidQwordStringEventRecord,
    };
    use efi::Guid;

    use super::*;

    #[test]
    fn test_performance_record_buffer_new() {
        let performance_record_buffer = PerformanceRecordBuffer::new();
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
                _ => assert!(false),
            }
        }
    }

    fn test_performance_record_buffer_reported_table() {
        let guid = efi::Guid::from_bytes(&[0; 16]);
        let mut performance_record_buffer = PerformanceRecordBuffer::new();

        performance_record_buffer.push_record(GuidEventRecord::new(1, 0, 10, guid)).unwrap();
        performance_record_buffer.push_record(DynamicStringEventRecord::new(1, 0, 10, guid, "test")).unwrap();

        let mut buffer = vec![0_u8; 1000];
        let buffer = unsafe { slice::from_raw_parts_mut(buffer.as_mut_ptr(), buffer.len()) };

        performance_record_buffer.report(buffer);

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
                _ => assert!(false),
            }
        }
    }
}
