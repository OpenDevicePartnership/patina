//! This module contains every implementation of [`PerformanceRecord`] produced by Patina SDK performance measurements.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::fmt::Debug;

use r_efi::efi;
use scroll::Pwrite;

use super::PerformanceRecord;

/// A performance string event record which includes a GUID.
#[derive(Debug)]
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
    /// The defined type ID for this record.
    pub const TYPE: u16 = 0x1010;
    /// The current revision version of this structure.
    pub const REVISION: u8 = 1;

    /// Creates a new `GuidEventRecord`.
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

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), scroll::Error> {
        buff.gwrite_with(self.progress_id, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.acpi_id, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.timestamp, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.guid.as_bytes().as_slice(), offset, ())?;
        Ok(())
    }
}

/// A performance string event record which includes an ASCII string.
#[derive(Debug)]
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
    /// The defined type ID for this record.
    pub const TYPE: u16 = 0x1011;
    /// The current revision version of this structure.
    pub const REVISION: u8 = 1;

    /// Creates a new `DynamicStringEventRecord`.
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
        dest.gwrite_with(0_u8, &mut offset, ctx)?; // End of the string.
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

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), scroll::Error> {
        buff.gwrite_with(self.progress_id, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.acpi_id, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.timestamp, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.guid.as_bytes().as_slice(), offset, ())?;
        buff.gwrite_with(self.string.as_bytes(), offset, ())?;
        buff.gwrite_with(0_u8, offset, scroll::NATIVE)?; // End of the string.
        Ok(())
    }
}

/// A performance string event record which includes a two GUIDs and an ASCII string.
#[derive(Debug)]
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
    /// The defined type ID for this record.
    pub const TYPE: u16 = 0x1012;
    /// The current revision version of this structure.
    pub const REVISION: u8 = 1;

    /// Creates a new `DualGuidStringEventRecord`.
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

impl PerformanceRecord for DualGuidStringEventRecord<'_> {
    fn record_type(&self) -> u16 {
        Self::TYPE
    }

    fn revision(&self) -> u8 {
        Self::REVISION
    }

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> core::result::Result<(), scroll::Error> {
        buff.gwrite_with(self.progress_id, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.acpi_id, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.timestamp, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.guid_1.as_bytes().as_slice(), offset, ())?;
        buff.gwrite_with(self.guid_2.as_bytes().as_slice(), offset, ())?;
        buff.gwrite_with(self.string.as_bytes(), offset, ())?;
        buff.gwrite_with(0_u8, offset, scroll::NATIVE)?; // End of the string.
        Ok(())
    }
}

/// A performance string event record which includes a GUID, and a QWORD.
#[derive(Debug)]
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
    /// The defined type ID for this record.
    pub const TYPE: u16 = 0x1013;
    /// The current revision version of this structure.
    pub const REVISION: u8 = 1;

    /// Creates a new `GuidQwordEventRecord`.
    pub fn new(progress_id: u16, acpi_id: u32, timestamp: u64, guid: efi::Guid, qword: u64) -> Self {
        Self { progress_id, acpi_id, timestamp, guid, qword }
    }
}

impl PerformanceRecord for GuidQwordEventRecord {
    fn record_type(&self) -> u16 {
        Self::TYPE
    }

    fn revision(&self) -> u8 {
        Self::REVISION
    }

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), scroll::Error> {
        buff.gwrite_with(self.progress_id, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.acpi_id, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.timestamp, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.guid.as_bytes().as_slice(), offset, ())?;
        buff.gwrite_with(self.qword, offset, scroll::NATIVE)?;
        Ok(())
    }
}

/// A performance string event record which includes a GUID, QWORD, and an ASCII string.
#[derive(Debug)]
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
    /// The defined type ID for this record.
    pub const TYPE: u16 = 0x1014;
    /// The current revision version of this structure.
    pub const REVISION: u8 = 1;

    /// Creates a new `GuidQwordStringEventRecord`.
    pub fn new(progress_id: u16, acpi_id: u32, timestamp: u64, guid: efi::Guid, qword: u64, string: &'a str) -> Self {
        Self { progress_id, acpi_id, timestamp, guid, qword, string }
    }
}

impl PerformanceRecord for GuidQwordStringEventRecord<'_> {
    fn record_type(&self) -> u16 {
        Self::TYPE
    }

    fn revision(&self) -> u8 {
        Self::REVISION
    }

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> core::result::Result<(), scroll::Error> {
        buff.gwrite_with(self.progress_id, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.acpi_id, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.timestamp, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.guid.as_bytes().as_slice(), offset, ())?;
        buff.gwrite_with(self.qword, offset, scroll::NATIVE)?;
        buff.gwrite_with(self.string.as_bytes(), offset, ())?;
        buff.gwrite_with(0_u8, offset, scroll::NATIVE)?; // End of the string.
        Ok(())
    }
}
