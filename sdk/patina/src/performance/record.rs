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

use crate::{BinaryGuid, performance::error::Error, performance_debug_assert};
use alloc::vec::Vec;
use core::{fmt, fmt::Debug, mem};
use scroll::Pread;
use zerocopy::{FromBytes, IntoBytes};
use zerocopy_derive::*;

/// Maximum size in byte that a performance record can have.
pub const FPDT_MAX_PERF_RECORD_SIZE: usize = u8::MAX as usize;

/// Performance record header structure.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable)]
pub struct PerformanceRecordHeader {
    /// This value depicts the format and contents of the performance record.
    pub record_type: u16,
    /// This value depicts the length of the performance record, in bytes.
    pub length: u8,
    /// This value is updated if the format of the record type is extended.
    pub revision: u8,
}

impl PerformanceRecordHeader {
    /// Size of the header structure in bytes
    pub const SIZE: usize = core::mem::size_of::<Self>();

    /// Create a new performance record header.
    pub const fn new(record_type: u16, length: u8, revision: u8) -> Self {
        Self { record_type, length, revision }
    }

    /// Convert the header to little-endian format.
    pub fn to_le(self) -> Self {
        Self { record_type: self.record_type.to_le(), length: self.length, revision: self.revision }
    }
}

impl TryFrom<&[u8]> for PerformanceRecordHeader {
    type Error = &'static str;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        if bytes.len() < Self::SIZE {
            return Err("Insufficient bytes for PerformanceRecordHeader");
        }

        Self::read_from_prefix(bytes)
            .map_err(|_| "Failed to parse PerformanceRecordHeader from bytes")
            .map(|(header, _)| header.to_le())
    }
}

impl From<PerformanceRecordHeader> for [u8; mem::size_of::<PerformanceRecordHeader>()] {
    fn from(header: PerformanceRecordHeader) -> Self {
        let le_header = header.to_le();
        le_header.as_bytes().try_into().expect("Size mismatch in From implementation")
    }
}

/// Size in byte of the header of a performance record.
pub const PERFORMANCE_RECORD_HEADER_SIZE: usize = mem::size_of::<PerformanceRecordHeader>();

/// Trait implemented by all performance record types that can be serialized into
/// the Firmware Basic Boot Performance Table (FBPT) buffer.
/// [`crate::performance::error::Error`].
pub trait PerformanceRecord {
    /// returns the type ID (NOT Rust's `TypeId`) value of the record
    fn record_type(&self) -> u16;

    /// Returns the revision of the record.
    fn revision(&self) -> u8;

    /// Write just the record payload (not including the common header)
    /// into `buff` at `offset`, advancing `offset` on success.
    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), Error>;

    /// Serialize the full record (header + payload) into `buff` at `offset`.
    ///
    /// ## Errors
    ///
    /// - On success returns the total size (header + payload).
    /// - Fails with:
    ///   - `Error::Serialization` if there is insufficient remaining space.
    ///   - `Error::RecordTooLarge` if the final size exceeds `u8::MAX`.
    fn write_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<usize, Error> {
        let start = *offset;
        if start + PERFORMANCE_RECORD_HEADER_SIZE > buff.len() {
            return Err(Error::Serialization);
        }

        // Create header with placeholder length
        let mut header = PerformanceRecordHeader::new(self.record_type(), 0, self.revision());

        // Skip header space and write data first
        *offset += PERFORMANCE_RECORD_HEADER_SIZE;
        self.write_data_into(buff, offset)?;

        // Calculate total record size and update header
        let record_size = *offset - start;
        if record_size > u8::MAX as usize {
            return Err(Error::RecordTooLarge { size: record_size });
        }
        header.length = record_size as u8;

        // Write the complete header
        let header_bytes: [u8; mem::size_of::<PerformanceRecordHeader>()] = header.into();
        buff[start..start + PERFORMANCE_RECORD_HEADER_SIZE].copy_from_slice(&header_bytes);

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

impl<T: AsRef<[u8]>> GenericPerformanceRecord<T> {
    /// Create a new generic performance record.
    pub fn new(record_type: u16, length: u8, revision: u8, data: T) -> Self {
        Self { record_type, length, revision, data }
    }

    /// Get the header as a structured type.
    pub fn header(&self) -> PerformanceRecordHeader {
        PerformanceRecordHeader::new(self.record_type, self.length, self.revision)
    }
}

impl<T: AsRef<[u8]>> PerformanceRecord for GenericPerformanceRecord<T> {
    fn record_type(&self) -> u16 {
        self.record_type
    }

    fn revision(&self) -> u8 {
        self.revision
    }

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), Error> {
        let remaining = buff.len().saturating_sub(*offset);
        let data = self.data.as_ref();
        if data.len() > remaining {
            return Err(Error::Serialization);
        }
        buff[*offset..*offset + data.len()].copy_from_slice(data);
        *offset += data.len();
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
        Some(GenericPerformanceRecord::new(record_type, length, revision, data))
    }
}

// ============================================================================
// MM Performance Record Data Structures
// ============================================================================

/// GUID Event Record (Type 0x1010)
///
/// A performance event record which includes a GUID.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct GuidEventRecordData {
    /// ProgressID < 0x10 are reserved for core performance entries.
    pub progress_id: u16,
    /// APIC ID for the processor in the system used as a timestamp clock source.
    pub apic_id: u32,
    /// 64-bit value (nanosecond) describing elapsed time since the most recent deassertion of processor reset.
    pub timestamp: u64,
    /// If ProgressID < 0x10, GUID of the referenced module; otherwise, GUID of the module logging the event.
    pub guid: [u8; 16],
}

impl GuidEventRecordData {
    /// Name of the record type
    pub const NAME: &'static str = "GUID Event";
}

impl fmt::Display for GuidEventRecordData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Note: Copying packed fields to local variables to avoid unaligned references
        let progress_id = self.progress_id;
        let apic_id = self.apic_id;
        let timestamp = self.timestamp;
        let guid = BinaryGuid::from_bytes(&self.guid);
        write!(f, "progress_id={}, apic_id={}, timestamp={}, guid={}", progress_id, apic_id, timestamp, guid)
    }
}

/// Dynamic String Event Record (Type 0x1011)
///
/// A performance event record which includes an ASCII string.
/// Note: The string is variable-length and follows this fixed header.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DynamicStringEventRecordData {
    /// ProgressID < 0x10 are reserved for core performance entries.
    pub progress_id: u16,
    /// APIC ID for the processor in the system used as a timestamp clock source.
    pub apic_id: u32,
    /// 64-bit value (nanosecond) describing elapsed time since the most recent deassertion of processor reset.
    pub timestamp: u64,
    /// If ProgressID < 0x10, GUID of the referenced module; otherwise, GUID of the module logging the event.
    pub guid: [u8; 16],
    // String data follows but is not a part of this fixed structure
}

impl DynamicStringEventRecordData {
    /// Name of the record type
    pub const NAME: &'static str = "Dynamic String Event";

    /// Get the string portion from the full record data
    pub fn extract_string(full_data: &[u8]) -> &str {
        if full_data.len() <= core::mem::size_of::<Self>() {
            return "";
        }
        let string_bytes = &full_data[core::mem::size_of::<Self>()..];
        // Find the null terminator
        let string_len = string_bytes.iter().position(|&b| b == 0).unwrap_or(string_bytes.len());
        core::str::from_utf8(&string_bytes[..string_len]).unwrap_or("<invalid UTF-8>")
    }
}

impl fmt::Display for DynamicStringEventRecordData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Note: Copying packed fields to local variables to avoid unaligned references
        let progress_id = self.progress_id;
        let apic_id = self.apic_id;
        let timestamp = self.timestamp;
        let guid = BinaryGuid::from_bytes(&self.guid);
        write!(f, "progress_id: 0x{:04X}, apic_id: {}, timestamp: {}, guid: {}", progress_id, apic_id, timestamp, guid)
    }
}

/// Dual GUID String Event Record (Type 0x1012)
///
/// A performance event record which includes two GUIDs and an ASCII string.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DualGuidStringEventRecordData {
    /// ProgressID < 0x10 are reserved for core performance entries.
    pub progress_id: u16,
    /// APIC ID for the processor in the system used as a timestamp clock source.
    pub apic_id: u32,
    /// 64-bit value (nanosecond) describing elapsed time since the most recent deassertion of processor reset.
    pub timestamp: u64,
    /// GUID of the module logging the event.
    pub guid_1: [u8; 16],
    /// Event or PPI or Protocol GUID for Callback.
    pub guid_2: [u8; 16],
    // String data follows but is not part of this fixed structure
}

impl DualGuidStringEventRecordData {
    /// Name of the record type
    pub const NAME: &'static str = "Dual GUID String Event";

    /// Get the string portion from the full record data
    pub fn extract_string(full_data: &[u8]) -> &str {
        if full_data.len() <= core::mem::size_of::<Self>() {
            return "";
        }
        let string_bytes = &full_data[core::mem::size_of::<Self>()..];
        // Find the null terminator
        let string_len = string_bytes.iter().position(|&b| b == 0).unwrap_or(string_bytes.len());
        core::str::from_utf8(&string_bytes[..string_len]).unwrap_or("<invalid UTF-8>")
    }
}

impl fmt::Display for DualGuidStringEventRecordData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Note: Copying packed fields to local variables to avoid unaligned references
        let progress_id = self.progress_id;
        let apic_id = self.apic_id;
        let timestamp = self.timestamp;
        let guid_1 = BinaryGuid::from_bytes(&self.guid_1);
        let guid_2 = BinaryGuid::from_bytes(&self.guid_2);
        write!(
            f,
            "progress_id: 0x{:04X}, apic_id: {}, timestamp: {}, guid_1: {}, guid_2: {}",
            progress_id, apic_id, timestamp, guid_1, guid_2
        )
    }
}

/// GUID QWORD Event Record (Type 0x1013)
///
/// A performance event record which includes a GUID and a QWORD value.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct GuidQwordEventRecordData {
    /// ProgressID < 0x10 are reserved for core performance entries.
    pub progress_id: u16,
    /// APIC ID for the processor in the system used as a timestamp clock source.
    pub apic_id: u32,
    /// 64-bit value (nanosecond) describing elapsed time since the most recent deassertion of processor reset.
    pub timestamp: u64,
    /// If ProgressID < 0x10, GUID of the referenced module; otherwise, GUID of the module logging the event.
    pub guid: [u8; 16],
    /// Event-specific QWORD value.
    pub qword: u64,
}

impl GuidQwordEventRecordData {
    /// Name of the record type
    pub const NAME: &'static str = "GUID QWORD Event";
}

impl fmt::Display for GuidQwordEventRecordData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Note: Copying packed fields to local variables to avoid unaligned references
        let progress_id = self.progress_id;
        let apic_id = self.apic_id;
        let timestamp = self.timestamp;
        let guid = BinaryGuid::from_bytes(&self.guid);
        let qword = self.qword;
        write!(
            f,
            "progress_id: 0x{:04X}, apic_id: {}, timestamp: {}, guid: {}, qword: 0x{:016X}",
            progress_id, apic_id, timestamp, guid, qword
        )
    }
}

/// GUID QWORD String Event Record (Type 0x1014)
///
/// A performance event record which includes a GUID, a QWORD value, and an ASCII string.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct GuidQwordStringEventRecordData {
    /// ProgressID < 0x10 are reserved for core performance entries.
    pub progress_id: u16,
    /// APIC ID for the processor in the system used as a timestamp clock source.
    pub apic_id: u32,
    /// 64-bit value (nanosecond) describing elapsed time since the most recent deassertion of processor reset.
    pub timestamp: u64,
    /// If ProgressID < 0x10, GUID of the referenced module; otherwise, GUID of the module logging the event.
    pub guid: [u8; 16],
    /// Event-specific QWORD value.
    pub qword: u64,
    // String data follows but is not part of this fixed structure
}

impl GuidQwordStringEventRecordData {
    /// Name of the record type
    pub const NAME: &'static str = "GUID QWORD String Event";

    /// Get the string portion from the full record data
    pub fn extract_string(full_data: &[u8]) -> &str {
        if full_data.len() <= core::mem::size_of::<Self>() {
            return "";
        }
        let string_bytes = &full_data[core::mem::size_of::<Self>()..];
        // Find the null terminator
        let string_len = string_bytes.iter().position(|&b| b == 0).unwrap_or(string_bytes.len());
        core::str::from_utf8(&string_bytes[..string_len]).unwrap_or("<invalid UTF-8>")
    }
}

impl fmt::Display for GuidQwordStringEventRecordData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Note: Copying packed fields to local variables to avoid unaligned references
        let progress_id = self.progress_id;
        let apic_id = self.apic_id;
        let timestamp = self.timestamp;
        let guid = BinaryGuid::from_bytes(&self.guid);
        let qword = self.qword;
        write!(
            f,
            "progress_id: 0x{:04X}, apic_id: {}, timestamp: {}, guid: {}, qword: 0x{:016X}",
            progress_id, apic_id, timestamp, guid, qword
        )
    }
}

/// Trait for types that can print detailed record information
pub trait PerformanceRecordDetails {
    /// Print detailed information about the record
    fn print_details(&self, record_number: usize);
}

impl PerformanceRecordDetails for GuidEventRecordData {
    fn print_details(&self, record_number: usize) {
        log::debug!("  Record #{}: {}", record_number, self);
    }
}

impl PerformanceRecordDetails for DynamicStringEventRecordData {
    fn print_details(&self, record_number: usize) {
        log::debug!("  Record #{}: {}", record_number, self);
    }
}

impl PerformanceRecordDetails for DualGuidStringEventRecordData {
    fn print_details(&self, record_number: usize) {
        log::debug!("  Record #{}: {}", record_number, self);
    }
}

impl PerformanceRecordDetails for GuidQwordEventRecordData {
    fn print_details(&self, record_number: usize) {
        log::debug!("  Record #{}: {}", record_number, self);
    }
}

impl PerformanceRecordDetails for GuidQwordStringEventRecordData {
    fn print_details(&self, record_number: usize) {
        log::debug!("  Record #{}: {}", record_number, self);
    }
}

/// Print detailed information about a performance record based on its type
pub fn print_record_details(record_type: u16, record_number: usize, data: &[u8]) {
    match record_type {
        0x1010 => {
            if data.len() >= core::mem::size_of::<GuidEventRecordData>() {
                // SAFETY: We've verified the data is large enough and the struct is packed
                let record = unsafe { &*(data.as_ptr() as *const GuidEventRecordData) };
                record.print_details(record_number);
            }
        }
        0x1011 => {
            if data.len() >= core::mem::size_of::<DynamicStringEventRecordData>() {
                // SAFETY: We've verified the data is large enough and the struct is packed
                let record = unsafe { &*(data.as_ptr() as *const DynamicStringEventRecordData) };
                record.print_details(record_number);
                let string_data = DynamicStringEventRecordData::extract_string(data);
                if !string_data.is_empty() {
                    log::debug!("    String: \"{}\"", string_data);
                }
            }
        }
        0x1012 => {
            if data.len() >= core::mem::size_of::<DualGuidStringEventRecordData>() {
                // SAFETY: We've verified the data is large enough and the struct is packed
                let record = unsafe { &*(data.as_ptr() as *const DualGuidStringEventRecordData) };
                record.print_details(record_number);
                let string_data = DualGuidStringEventRecordData::extract_string(data);
                if !string_data.is_empty() {
                    log::debug!("    String: \"{}\"", string_data);
                }
            }
        }
        0x1013 => {
            if data.len() >= core::mem::size_of::<GuidQwordEventRecordData>() {
                // SAFETY: We've verified the data is large enough and the struct is packed
                let record = unsafe { &*(data.as_ptr() as *const GuidQwordEventRecordData) };
                record.print_details(record_number);
            }
        }
        0x1014 => {
            if data.len() >= core::mem::size_of::<GuidQwordStringEventRecordData>() {
                // SAFETY: We've verified the data is large enough and the struct is packed
                let record = unsafe { &*(data.as_ptr() as *const GuidQwordStringEventRecordData) };
                record.print_details(record_number);
                let string_data = GuidQwordStringEventRecordData::extract_string(data);
                if !string_data.is_empty() {
                    log::debug!("    String: \"{}\"", string_data);
                }
            }
        }
        _ => {
            log::debug!("  Record #{}: Unknown type 0x{:04X}", record_number, record_type);
        }
    }
}

/// Get a human-readable name for a record type
pub fn record_type_name(record_type: u16) -> &'static str {
    match record_type {
        0x1010 => GuidEventRecordData::NAME,
        0x1011 => DynamicStringEventRecordData::NAME,
        0x1012 => DualGuidStringEventRecordData::NAME,
        0x1013 => GuidQwordEventRecordData::NAME,
        0x1014 => GuidQwordStringEventRecordData::NAME,
        _ => "Unknown",
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
        // SAFETY: Test code - creating a mutable slice from vector for testing record reporting.
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

    #[test]
    fn test_performance_record_header_try_from_valid_bytes() {
        let original_header = PerformanceRecordHeader::new(0x1234, 42, 1);
        let bytes: [u8; 4] = original_header.into();

        let parsed_header = PerformanceRecordHeader::try_from(bytes.as_slice()).unwrap();

        // Copy values locally since `PerformanceRecordHeader` is packed
        let parsed_type = parsed_header.record_type;
        let parsed_length = parsed_header.length;
        let parsed_revision = parsed_header.revision;

        assert_eq!(parsed_type, 0x1234);
        assert_eq!(parsed_length, 42);
        assert_eq!(parsed_revision, 1);
    }

    #[test]
    fn test_performance_record_header_try_from_insufficient_bytes() {
        let bytes = [0x34, 0x12]; // Only 2 bytes instead of 4
        let result = PerformanceRecordHeader::try_from(bytes.as_slice());

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Insufficient bytes for PerformanceRecordHeader");
    }

    #[test]
    fn test_performance_record_header_try_from_empty_bytes() {
        let bytes: &[u8] = &[];
        let result = PerformanceRecordHeader::try_from(bytes);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Insufficient bytes for PerformanceRecordHeader");
    }

    #[test]
    fn test_performance_record_header_from_trait_conversion_le() {
        let header = PerformanceRecordHeader::new(0xABCD, 100, 2);
        let bytes: [u8; mem::size_of::<PerformanceRecordHeader>()] = header.into();

        // Check little-endian order
        assert_eq!(bytes[0], 0xCD); // Low byte of 0xABCD
        assert_eq!(bytes[1], 0xAB); // High byte of 0xABCD
        assert_eq!(bytes[2], 100); // Length
        assert_eq!(bytes[3], 2); // Revision
    }

    #[test]
    fn test_performance_record_header_roundtrip_conversion() {
        // Test that we can convert header -> bytes -> header and get the same result
        let original_header = PerformanceRecordHeader::new(0x5678, 200, 3);

        let bytes: [u8; mem::size_of::<PerformanceRecordHeader>()] = original_header.into();
        let parsed_header = PerformanceRecordHeader::try_from(bytes.as_slice()).unwrap();

        let orig_type = original_header.record_type;
        let orig_length = original_header.length;
        let orig_revision = original_header.revision;
        let parsed_type = parsed_header.record_type;
        let parsed_length = parsed_header.length;
        let parsed_revision = parsed_header.revision;

        assert_eq!(orig_type, parsed_type);
        assert_eq!(orig_length, parsed_length);
        assert_eq!(orig_revision, parsed_revision);
    }

    #[test]
    fn test_performance_record_header_le_handling() {
        // Test that little-endian conversion works correctly for multi-byte fields
        let header = PerformanceRecordHeader::new(0x0102, 50, 1);
        let bytes: [u8; mem::size_of::<PerformanceRecordHeader>()] = header.into();

        assert_eq!(bytes[0], 0x02);
        assert_eq!(bytes[1], 0x01);
        assert_eq!(bytes[2], 50);
        assert_eq!(bytes[3], 1);

        // Parse it back
        let parsed = PerformanceRecordHeader::try_from(bytes.as_slice()).unwrap();

        let parsed_type = parsed.record_type;
        let parsed_length = parsed.length;
        let parsed_revision = parsed.revision;

        assert_eq!(parsed_type, 0x0102);
        assert_eq!(parsed_length, 50);
        assert_eq!(parsed_revision, 1);
    }

    #[test]
    fn test_performance_record_header_try_from_extra_bytes() {
        // Test with more bytes than needed (should still work)
        let mut bytes = vec![0x34, 0x12, 42, 1]; // Valid header
        bytes.extend_from_slice(&[0xFF, 0xFF, 0xFF]); // Extra bytes

        let parsed_header = PerformanceRecordHeader::try_from(bytes.as_slice()).unwrap();

        let parsed_type = parsed_header.record_type;
        let parsed_length = parsed_header.length;
        let parsed_revision = parsed_header.revision;

        assert_eq!(parsed_type, 0x1234);
        assert_eq!(parsed_length, 42);
        assert_eq!(parsed_revision, 1);
    }

    #[test]
    fn test_guid_event_record_data_display() {
        let record = GuidEventRecordData {
            progress_id: 0x1234,
            apic_id: 42,
            timestamp: 1000000,
            guid: [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10],
        };

        let display_str = format!("{}", record);
        assert!(display_str.contains("progress_id=4660"));
        assert!(display_str.contains("apic_id=42"));
        assert!(display_str.contains("timestamp=1000000"));
    }

    #[test]
    fn test_dynamic_string_event_record_data_display() {
        let record = DynamicStringEventRecordData {
            progress_id: 0x5678,
            apic_id: 99,
            timestamp: 2000000,
            guid: [0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x20],
        };

        let display_str = format!("{}", record);
        assert!(display_str.contains("progress_id: 0x5678"));
        assert!(display_str.contains("apic_id: 99"));
        assert!(display_str.contains("timestamp: 2000000"));
    }

    #[test]
    fn test_dynamic_string_event_record_data_extract_string() {
        let mut data = vec![0u8; core::mem::size_of::<DynamicStringEventRecordData>()];
        let test_string = b"Test String\0";
        data.extend_from_slice(test_string);

        let extracted = DynamicStringEventRecordData::extract_string(&data);
        assert_eq!(extracted, "Test String");
    }

    #[test]
    fn test_dynamic_string_event_record_data_extract_string_empty() {
        let data = vec![0u8; core::mem::size_of::<DynamicStringEventRecordData>()];
        let extracted = DynamicStringEventRecordData::extract_string(&data);
        assert_eq!(extracted, "");
    }

    #[test]
    fn test_dynamic_string_event_record_data_extract_string_invalid_utf8() {
        let mut data = vec![0u8; core::mem::size_of::<DynamicStringEventRecordData>()];
        data.extend_from_slice(&[0xFF, 0xFE, 0xFD, 0x00]); // Invalid UTF-8

        let extracted = DynamicStringEventRecordData::extract_string(&data);
        assert_eq!(extracted, "<invalid UTF-8>");
    }

    #[test]
    fn test_dynamic_string_event_record_data_extract_string_no_null_terminator() {
        let mut data = vec![0u8; core::mem::size_of::<DynamicStringEventRecordData>()];
        data.extend_from_slice(b"NoNull");

        let extracted = DynamicStringEventRecordData::extract_string(&data);
        assert_eq!(extracted, "NoNull");
    }

    #[test]
    fn test_dual_guid_string_event_record_data_display() {
        let record = DualGuidStringEventRecordData {
            progress_id: 0xABCD,
            apic_id: 123,
            timestamp: 3000000,
            guid_1: [0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2E, 0x2F, 0x30],
            guid_2: [0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F, 0x40],
        };

        let display_str = format!("{}", record);
        assert!(display_str.contains("progress_id: 0xABCD"));
        assert!(display_str.contains("apic_id: 123"));
        assert!(display_str.contains("timestamp: 3000000"));
        assert!(display_str.contains("guid_1:"));
        assert!(display_str.contains("guid_2:"));
    }

    #[test]
    fn test_dual_guid_string_event_record_data_extract_string() {
        let mut data = vec![0u8; core::mem::size_of::<DualGuidStringEventRecordData>()];
        let test_string = b"DualGuidTest\0";
        data.extend_from_slice(test_string);

        let extracted = DualGuidStringEventRecordData::extract_string(&data);
        assert_eq!(extracted, "DualGuidTest");
    }

    #[test]
    fn test_dual_guid_string_event_record_data_extract_string_empty() {
        let data = vec![0u8; core::mem::size_of::<DualGuidStringEventRecordData>()];
        let extracted = DualGuidStringEventRecordData::extract_string(&data);
        assert_eq!(extracted, "");
    }

    #[test]
    fn test_guid_qword_event_record_data_display() {
        let record = GuidQwordEventRecordData {
            progress_id: 0xEF01,
            apic_id: 200,
            timestamp: 4000000,
            guid: [0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F, 0x50],
            qword: 0x123456789ABCDEF0,
        };

        let display_str = format!("{}", record);
        assert!(display_str.contains("progress_id: 0xEF01"));
        assert!(display_str.contains("apic_id: 200"));
        assert!(display_str.contains("timestamp: 4000000"));
        assert!(display_str.contains("qword: 0x123456789ABCDEF0"));
    }

    #[test]
    fn test_guid_qword_string_event_record_data_display() {
        let record = GuidQwordStringEventRecordData {
            progress_id: 0x2345,
            apic_id: 77,
            timestamp: 5000000,
            guid: [0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F, 0x60],
            qword: 0xFEDCBA9876543210,
        };

        let display_str = format!("{}", record);
        assert!(display_str.contains("progress_id: 0x2345"));
        assert!(display_str.contains("apic_id: 77"));
        assert!(display_str.contains("timestamp: 5000000"));
        assert!(display_str.contains("qword: 0xFEDCBA9876543210"));
    }

    #[test]
    fn test_guid_qword_string_event_record_data_extract_string() {
        let mut data = vec![0u8; core::mem::size_of::<GuidQwordStringEventRecordData>()];
        let test_string = b"QwordString\0";
        data.extend_from_slice(test_string);

        let extracted = GuidQwordStringEventRecordData::extract_string(&data);
        assert_eq!(extracted, "QwordString");
    }

    #[test]
    fn test_guid_qword_string_event_record_data_extract_string_empty() {
        let data = vec![0u8; core::mem::size_of::<GuidQwordStringEventRecordData>()];
        let extracted = GuidQwordStringEventRecordData::extract_string(&data);
        assert_eq!(extracted, "");
    }

    #[test]
    fn test_record_type_name_all_types() {
        assert_eq!(record_type_name(0x1010), GuidEventRecordData::NAME);
        assert_eq!(record_type_name(0x1011), DynamicStringEventRecordData::NAME);
        assert_eq!(record_type_name(0x1012), DualGuidStringEventRecordData::NAME);
        assert_eq!(record_type_name(0x1013), GuidQwordEventRecordData::NAME);
        assert_eq!(record_type_name(0x1014), GuidQwordStringEventRecordData::NAME);
        assert_eq!(record_type_name(0x9999), "Unknown");
    }

    #[test]
    fn test_record_type_name_boundary_values() {
        assert_eq!(record_type_name(0x0000), "Unknown");
        assert_eq!(record_type_name(0xFFFF), "Unknown");
        assert_eq!(record_type_name(0x100F), "Unknown");
        assert_eq!(record_type_name(0x1015), "Unknown");
    }

    #[test]
    fn test_guid_event_record_data_name_constant() {
        assert_eq!(GuidEventRecordData::NAME, "GUID Event");
    }

    #[test]
    fn test_dynamic_string_event_record_data_name_constant() {
        assert_eq!(DynamicStringEventRecordData::NAME, "Dynamic String Event");
    }

    #[test]
    fn test_dual_guid_string_event_record_data_name_constant() {
        assert_eq!(DualGuidStringEventRecordData::NAME, "Dual GUID String Event");
    }

    #[test]
    fn test_guid_qword_event_record_data_name_constant() {
        assert_eq!(GuidQwordEventRecordData::NAME, "GUID QWORD Event");
    }

    #[test]
    fn test_guid_qword_string_event_record_data_name_constant() {
        assert_eq!(GuidQwordStringEventRecordData::NAME, "GUID QWORD String Event");
    }
}
