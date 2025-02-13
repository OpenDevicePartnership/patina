//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use core::fmt::Debug;

use mu_rust_helpers::guid::guid_fmt;
use r_efi::efi;
use scroll::Pwrite;

use super::PerformanceRecord;

#[repr(C)]
pub struct GuidEventRecord {
    /// ProgressID < 0x10 are reserved for core performance entries.
    /// Start measurement point shall have lowered one nibble set to zero and
    /// corresponding end points shall have lowered one nibble set to non-zero value;
    /// keeping other nibbles same as start point.
    pub progress_id: u16,
    /// APIC ID for the processor in the system used as a timestamp clock source.
    /// If only one timestamp clock source is used, this field is Reserved and populated as 0.
    pub acpi_id: u32,
    /// 64-bit value (nanosecond) describing elapsed time since the most recent deassertion of processor reset.
    pub timestamp: u64,
    /// If ProgressID < 0x10, GUID of the referenced module; otherwise, GUID of the module logging the event.
    pub guid: efi::Guid,
}

impl GuidEventRecord {
    const TYPE: u16 = 0x1010;
    const REVISION: u8 = 1;

    pub fn new(progress_id: u16, acpi_id: u32, timestamp: u64, guid: efi::Guid) -> Self {
        Self { progress_id, acpi_id, timestamp, guid }
    }
}

impl PerformanceRecord for GuidEventRecord {
    fn record_type(&self) -> u16 {
        Self::TYPE
    }

    fn revision(&self) -> u8 {
        Self::REVISION
    }
}

impl scroll::ctx::TryIntoCtx<scroll::Endian> for GuidEventRecord {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        let mut offset = 0;
        dest.gwrite_with(self.progress_id, &mut offset, ctx)?;
        dest.gwrite_with(self.acpi_id, &mut offset, ctx)?;
        dest.gwrite_with(self.timestamp, &mut offset, ctx)?;
        dest.gwrite_with(self.guid.as_bytes().as_slice(), &mut offset, ())?;
        Ok(offset)
    }
}

impl Debug for GuidEventRecord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GuidEventRecord")
            .field("type", &self.record_type())
            .field("revision", &self.revision())
            .field("progress_id", &self.progress_id)
            .field("acpi_id", &self.acpi_id)
            .field("timestamp", &self.timestamp)
            .field("guid", &guid_fmt!(&self.guid))
            .finish()
    }
}

#[repr(C)]
pub struct DynamicStringEventRecord<'a> {
    /// ProgressID < 0x10 are reserved for core performance entries.
    /// Start measurement point shall have lowered one nibble set to zero and
    /// corresponding end points shall have lowered one nibble set to non-zero value;
    /// keeping other nibbles same as start point.
    pub progress_id: u16,
    /// APIC ID for the processor in the system used as a timestamp clock source.
    /// If only one timestamp clock source is used, this field is Reserved and populated as 0.
    pub acpi_id: u32,
    /// 64-bit value (nanosecond) describing elapsed time since the most recent deassertion of processor reset.
    pub timestamp: u64,
    /// If ProgressID < 0x10, GUID of the referenced module; otherwise, GUID of the module logging the event.
    pub guid: efi::Guid,
    /// ASCII string describing the module. Padding supplied at the end if necessary with null characters (0x00).
    /// It may be module name, function name, or token name.
    pub string: &'a str,
}

impl<'a> DynamicStringEventRecord<'a> {
    const TYPE: u16 = 0x1011;
    const REVISION: u8 = 1;

    pub fn new(progress_id: u16, acpi_id: u32, timestamp: u64, guid: efi::Guid, string: &'a str) -> Self {
        Self { progress_id, acpi_id, timestamp, guid, string }
    }
}

impl scroll::ctx::TryIntoCtx<scroll::Endian> for DynamicStringEventRecord<'_> {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        let mut offset = 0;
        dest.gwrite_with(self.progress_id, &mut offset, ctx)?;
        dest.gwrite_with(self.acpi_id, &mut offset, ctx)?;
        dest.gwrite_with(self.timestamp, &mut offset, ctx)?;
        dest.gwrite_with(self.guid.as_bytes().as_slice(), &mut offset, ())?;
        dest.gwrite_with(self.string.as_bytes(), &mut offset, ())?;
        dest.gwrite_with(0_u8, &mut offset, ctx)?;
        Ok(offset)
    }
}

impl PerformanceRecord for DynamicStringEventRecord<'_> {
    fn record_type(&self) -> u16 {
        Self::TYPE
    }

    fn revision(&self) -> u8 {
        Self::REVISION
    }
}

impl Debug for DynamicStringEventRecord<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DynamicStringEventRecord")
            .field("type", &self.record_type())
            .field("revision", &self.revision())
            .field("progress_id", &self.progress_id)
            .field("acpi_id", &self.acpi_id)
            .field("timestamp", &self.timestamp)
            .field("guid", &self.guid)
            .field("string", &self.string)
            .finish()
    }
}

#[repr(C)]
pub struct DualGuidStringEventRecord<'a> {
    /// ProgressID < 0x10 are reserved for core performance entries.
    /// Start measurement point shall have lowered one nibble set to zero and
    /// corresponding end points shall have lowered one nibble set to non-zero value;
    /// keeping other nibbles same as start point.
    pub progress_id: u16,
    /// APIC ID for the processor in the system used as a timestamp clock source.
    /// If only one timestamp clock source is used, this field is Reserved and populated as 0.
    pub acpi_id: u32,
    /// 64-bit value (nanosecond) describing elapsed time since the most recent deassertion of processor reset.
    pub timestamp: u64,
    /// GUID of the module logging the event.
    pub guid_1: efi::Guid,
    /// Event or Ppi or Protocol GUID for Callback.
    pub guid_2: efi::Guid,
    /// ASCII string describing the module.
    /// It is the function name.
    pub string: &'a str,
}

impl<'a> DualGuidStringEventRecord<'a> {
    const TYPE: u16 = 0x1012;
    const REVISION: u8 = 1;

    pub fn new(
        progress_id: u16,
        acpi_id: u32,
        timestamp: u64,
        guid_1: efi::Guid,
        guid_2: efi::Guid,
        string: &'a str,
    ) -> Self {
        Self { progress_id, acpi_id, timestamp, guid_1, guid_2, string }
    }
}

impl scroll::ctx::TryIntoCtx<scroll::Endian> for DualGuidStringEventRecord<'_> {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        let mut offset = 0;
        dest.gwrite_with(self.progress_id, &mut offset, ctx)?;
        dest.gwrite_with(self.acpi_id, &mut offset, ctx)?;
        dest.gwrite_with(self.timestamp, &mut offset, ctx)?;
        dest.gwrite_with(self.guid_1.as_bytes().as_slice(), &mut offset, ())?;
        dest.gwrite_with(self.guid_2.as_bytes().as_slice(), &mut offset, ())?;
        dest.gwrite_with(self.string.as_bytes(), &mut offset, ())?;
        dest.gwrite_with(0_u8, &mut offset, ctx)?;
        Ok(offset)
    }
}

impl PerformanceRecord for DualGuidStringEventRecord<'_> {
    fn record_type(&self) -> u16 {
        Self::TYPE
    }

    fn revision(&self) -> u8 {
        Self::REVISION
    }
}

impl Debug for DualGuidStringEventRecord<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DualGuidStringEventRecord")
            .field("type", &self.record_type())
            .field("revision", &self.revision())
            .field("progress_id", &self.progress_id)
            .field("acpi_id", &self.acpi_id)
            .field("timestamp", &self.timestamp)
            .field("guid_1", &guid_fmt!(&self.guid_1))
            .field("guid_2", &guid_fmt!(&self.guid_2))
            .field("string", &self.string)
            .finish()
    }
}

#[repr(C)]
pub struct GuidQwordEventRecord {
    /// ProgressID < 0x10 are reserved for core performance entries.
    /// Start measurement point shall have lowered one nibble set to zero and
    /// corresponding end points shall have lowered one nibble set to non-zero value;
    /// keeping other nibbles same as start point.
    pub progress_id: u16,
    /// APIC ID for the processor in the system used as a timestamp clock source.
    /// If only one timestamp clock source is used, this field is Reserved and populated as 0.
    pub acpi_id: u32,
    /// 64-bit value (nanosecond) describing elapsed time since the most recent deassertion of processor reset.
    pub timestamp: u64,
    /// GUID of the module logging the event.
    pub guid: efi::Guid,
    /// Qword of misc data, meaning depends on the ProgressId.
    pub qword: u64,
}

impl GuidQwordEventRecord {
    pub const TYPE: u16 = 0x1013;
    pub const REVISION: u8 = 1;

    pub fn new(progress_id: u16, timestamp: u64, guid: efi::Guid, qword: u64) -> Self {
        Self { progress_id, acpi_id: 0, timestamp, guid, qword }
    }
}

impl PerformanceRecord for GuidQwordEventRecord {
    fn record_type(&self) -> u16 {
        Self::TYPE
    }

    fn revision(&self) -> u8 {
        Self::REVISION
    }
}

impl scroll::ctx::TryIntoCtx<scroll::Endian> for GuidQwordEventRecord {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        let mut offset = 0;
        dest.gwrite_with(self.progress_id, &mut offset, ctx)?;
        dest.gwrite_with(self.acpi_id, &mut offset, ctx)?;
        dest.gwrite_with(self.timestamp, &mut offset, ctx)?;
        dest.gwrite_with(*self.guid.as_bytes(), &mut offset, ctx)?;
        dest.gwrite_with(self.qword, &mut offset, ctx)?;
        Ok(offset)
    }
}

impl Debug for GuidQwordEventRecord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GuidQwordEventRecord")
            .field("type", &self.record_type())
            .field("revision", &self.revision())
            .field("progress_id", &self.progress_id)
            .field("acpi_id", &self.acpi_id)
            .field("timestamp", &self.timestamp)
            .field("guid", &guid_fmt!(&self.guid))
            .field("qword", &self.qword)
            .finish()
    }
}

#[repr(C)]
pub struct GuidQwordStringEventRecord<'a> {
    /// ProgressID < 0x10 are reserved for core performance entries.
    /// Start measurement point shall have lowered one nibble set to zero and
    /// corresponding end points shall have lowered one nibble set to non-zero value;
    /// keeping other nibbles same as start point.
    pub progress_id: u16,
    /// APIC ID for the processor in the system used as a timestamp clock source.
    /// If only one timestamp clock source is used, this field is Reserved and populated as 0.
    pub acpi_id: u32,
    /// 64-bit value (nanosecond) describing elapsed time since the most recent deassertion of processor reset.
    pub timestamp: u64,
    /// GUID of the module logging the event
    pub guid: efi::Guid,
    /// Qword of misc data, meaning depends on the ProgressId
    pub qword: u64,
    /// ASCII string describing the module.
    pub string: &'a str,
}

impl<'a> GuidQwordStringEventRecord<'a> {
    const TYPE: u16 = 0x1014;
    const REVISION: u8 = 1;

    pub fn new(progress_id: u16, acpi_id: u32, timestamp: u64, guid: efi::Guid, qword: u64, string: &'a str) -> Self {
        Self { progress_id, acpi_id, timestamp, guid, qword, string }
    }
}

impl scroll::ctx::TryIntoCtx<scroll::Endian> for GuidQwordStringEventRecord<'_> {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        let mut offset = 0;
        dest.gwrite_with(self.progress_id, &mut offset, ctx)?;
        dest.gwrite_with(self.acpi_id, &mut offset, ctx)?;
        dest.gwrite_with(self.timestamp, &mut offset, ctx)?;
        dest.gwrite_with(*self.guid.as_bytes(), &mut offset, ctx)?;
        dest.gwrite_with(self.qword, &mut offset, ctx)?;
        dest.gwrite_with(self.string, &mut offset, ())?;
        dest.gwrite_with(0_u8, &mut offset, ctx)?;
        Ok(offset)
    }
}

impl PerformanceRecord for GuidQwordStringEventRecord<'_> {
    fn record_type(&self) -> u16 {
        Self::TYPE
    }

    fn revision(&self) -> u8 {
        Self::REVISION
    }
}

impl Debug for GuidQwordStringEventRecord<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GuidQwordStringEventRecord")
            .field("type", &self.record_type())
            .field("revision", &self.revision())
            .field("progress_id", &self.progress_id)
            .field("acpi_id", &self.acpi_id)
            .field("timestamp", &self.timestamp)
            .field("guid", &guid_fmt!(&self.guid))
            .field("qword", &self.qword)
            .field("string", &self.string)
            .finish()
    }
}

mod tests {
    use scroll::Endian;

    use super::*;
    #[test]
    fn test_write_guid_event_record() {
        let guid =
            efi::Guid::from_fields(0x12345678, 0x9ABC, 0xDEF1, 0x23, 0x45, &[0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x12]);
        let record = GuidEventRecord::new(0x12, 0x3456789, 0x1122334455667788, guid);
        let guid_bytes = record.guid.as_bytes().to_owned();

        let mut buffer = [0u8; 30]; // Adjust size based on the actual struct size
        let result = buffer.pwrite_with(record, 0, Endian::Little);

        // Ensure no errors occurred and check the written byte count
        assert!(result.is_ok());
        let written_bytes = result.unwrap();
        assert_eq!(written_bytes, 30); // Expected byte size

        // Manually check if the fields were written correctly in little-endian
        assert_eq!(&buffer[0..2], &0x12u16.to_le_bytes());
        assert_eq!(&buffer[2..6], &0x3456789u32.to_le_bytes());
        assert_eq!(&buffer[6..14], &0x1122334455667788u64.to_le_bytes());
        assert_eq!(&buffer[14..], guid_bytes.as_slice()); // GUID should be written directly
    }

    #[test]
    fn test_write_dynamic_string_event_record() {
        let guid =
            efi::Guid::from_fields(0x12345678, 0x9ABC, 0xDEF1, 0x23, 0x45, &[0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x12]);
        let record = DynamicStringEventRecord::new(0x12, 0x3456789, 0x1122334455667788, guid, "test_string");

        let mut buffer = [0u8; 42]; // 30 bytes for header + 12 for string+null
        let result = buffer.pwrite_with(record, 0, Endian::Little);

        assert!(result.is_ok());
        let written_bytes = result.unwrap();
        assert_eq!(written_bytes, 42);

        assert_eq!(&buffer[0..2], &0x12u16.to_le_bytes());
        assert_eq!(&buffer[2..6], &0x3456789u32.to_le_bytes());
        assert_eq!(&buffer[6..14], &0x1122334455667788u64.to_le_bytes());
        assert_eq!(&buffer[14..30], guid.as_bytes());
        assert_eq!(&buffer[30..41], b"test_string");
        assert_eq!(buffer[41], 0); // Null terminator
    }

    #[test]
    fn test_write_dual_guid_string_event_record() {
        let guid1 =
            efi::Guid::from_fields(0x12345678, 0x9ABC, 0xDEF1, 0x23, 0x45, &[0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x12]);
        let guid2 =
            efi::Guid::from_fields(0x87654321, 0xCBA9, 0x1FED, 0x32, 0x54, &[0x76, 0x98, 0xBA, 0xDC, 0xFE, 0x21]);
        let record = DualGuidStringEventRecord::new(0x12, 0x3456789, 0x1122334455667788, guid1, guid2, "test_string");

        let mut buffer = [0u8; 58]; // 46 bytes for header + 12 for string+null
        let result = buffer.pwrite_with(record, 0, Endian::Little);

        assert!(result.is_ok());
        let written_bytes = result.unwrap();
        assert_eq!(written_bytes, 58);

        assert_eq!(&buffer[0..2], &0x12u16.to_le_bytes());
        assert_eq!(&buffer[2..6], &0x3456789u32.to_le_bytes());
        assert_eq!(&buffer[6..14], &0x1122334455667788u64.to_le_bytes());
        assert_eq!(&buffer[14..30], guid1.as_bytes());
        assert_eq!(&buffer[30..46], guid2.as_bytes());
        assert_eq!(&buffer[46..57], b"test_string");
        assert_eq!(buffer[57], 0); // Null terminator
    }

    #[test]
    fn test_write_guid_qword_event_record() {
        let guid =
            efi::Guid::from_fields(0x12345678, 0x9ABC, 0xDEF1, 0x23, 0x45, &[0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x12]);
        let record = GuidQwordEventRecord::new(0x12, 0x1122334455667788, guid, 0x8877665544332211);

        let mut buffer = [0u8; 38]; // 30 bytes + 8 for qword
        let result = buffer.pwrite_with(record, 0, Endian::Little);

        assert!(result.is_ok());
        let written_bytes = result.unwrap();
        assert_eq!(written_bytes, 38);

        assert_eq!(&buffer[0..2], &0x12u16.to_le_bytes());
        assert_eq!(&buffer[2..6], &0x0u32.to_le_bytes());
        assert_eq!(&buffer[6..14], &0x1122334455667788u64.to_le_bytes());
        assert_eq!(&buffer[14..30], guid.as_bytes());
        assert_eq!(&buffer[30..38], &0x8877665544332211u64.to_le_bytes());
    }

    #[test]
    fn test_write_guid_qword_string_event_record() {
        let guid =
            efi::Guid::from_fields(0x12345678, 0x9ABC, 0xDEF1, 0x23, 0x45, &[0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x12]);
        let record = GuidQwordStringEventRecord::new(
            0x12,
            0x3456789,
            0x1122334455667788,
            guid,
            0x8877665544332211,
            "test_string",
        );

        let mut buffer = [0u8; 50]; // 38 bytes + 12 for string+null
        let result = buffer.pwrite_with(record, 0, Endian::Little);

        assert!(result.is_ok());
        let written_bytes = result.unwrap();
        assert_eq!(written_bytes, 50);

        assert_eq!(&buffer[0..2], &0x12u16.to_le_bytes());
        assert_eq!(&buffer[2..6], &0x3456789u32.to_le_bytes());
        assert_eq!(&buffer[6..14], &0x1122334455667788u64.to_le_bytes());
        assert_eq!(&buffer[14..30], guid.as_bytes());
        assert_eq!(&buffer[30..38], &0x8877665544332211u64.to_le_bytes());
        assert_eq!(&buffer[38..49], b"test_string");
        assert_eq!(buffer[49], 0); // Null terminator
    }
}
