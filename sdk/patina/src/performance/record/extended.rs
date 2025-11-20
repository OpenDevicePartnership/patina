//! This module contains every implementation of [`PerformanceRecord`] produced by Patina SDK performance measurements.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::fmt::Debug;

use r_efi::efi;

use super::PerformanceRecord;
use crate::performance::error::Error;

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

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), Error> {
        write_u16_le(buff, offset, self.progress_id)?;
        write_u32_le(buff, offset, self.acpi_id)?;
        write_u64_le(buff, offset, self.timestamp)?;
        write_bytes(buff, offset, self.guid.as_bytes())?;
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

impl PerformanceRecord for DynamicStringEventRecord<'_> {
    fn record_type(&self) -> u16 {
        Self::TYPE
    }

    fn revision(&self) -> u8 {
        Self::REVISION
    }

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), Error> {
        write_u16_le(buff, offset, self.progress_id)?;
        write_u32_le(buff, offset, self.acpi_id)?;
        write_u64_le(buff, offset, self.timestamp)?;
        write_bytes(buff, offset, self.guid.as_bytes())?;
        write_bytes(buff, offset, self.string.as_bytes())?;
        write_u8(buff, offset, 0)?; // terminator
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

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), Error> {
        write_u16_le(buff, offset, self.progress_id)?;
        write_u32_le(buff, offset, self.acpi_id)?;
        write_u64_le(buff, offset, self.timestamp)?;
        write_bytes(buff, offset, self.guid_1.as_bytes())?;
        write_bytes(buff, offset, self.guid_2.as_bytes())?;
        write_bytes(buff, offset, self.string.as_bytes())?;
        write_u8(buff, offset, 0)?;
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

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), Error> {
        write_u16_le(buff, offset, self.progress_id)?;
        write_u32_le(buff, offset, self.acpi_id)?;
        write_u64_le(buff, offset, self.timestamp)?;
        write_bytes(buff, offset, self.guid.as_bytes())?;
        write_u64_le(buff, offset, self.qword)?;
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

    fn write_data_into(&self, buff: &mut [u8], offset: &mut usize) -> Result<(), Error> {
        write_u16_le(buff, offset, self.progress_id)?;
        write_u32_le(buff, offset, self.acpi_id)?;
        write_u64_le(buff, offset, self.timestamp)?;
        write_bytes(buff, offset, self.guid.as_bytes())?;
        write_u64_le(buff, offset, self.qword)?;
        write_bytes(buff, offset, self.string.as_bytes())?;
        write_u8(buff, offset, 0)?;
        Ok(())
    }
}

trait IntoLeBytes {
    type Bytes: AsRef<[u8]>;
    fn to_le_bytes(self) -> Self::Bytes;
}

macro_rules! impl_into_le_bytes {
    ($t:ty) => {
        impl IntoLeBytes for $t {
            type Bytes = [u8; core::mem::size_of::<$t>()];
            fn to_le_bytes(self) -> Self::Bytes {
                <$t>::to_le_bytes(self)
            }
        }
    };
}

impl_into_le_bytes!(u8);
impl_into_le_bytes!(u16);
impl_into_le_bytes!(u32);
impl_into_le_bytes!(u64);

fn ensure_space(buff: &[u8], offset: usize, needed: usize) -> Result<(), Error> {
    if offset + needed > buff.len() {
        return Err(Error::Serialization);
    }
    Ok(())
}

fn write_bytes(dest: &mut [u8], offset: &mut usize, src: &[u8]) -> Result<(), Error> {
    ensure_space(dest, *offset, src.len())?;
    dest[*offset..*offset + src.len()].copy_from_slice(src);
    *offset += src.len();
    Ok(())
}

fn write_uint<T: IntoLeBytes>(dest: &mut [u8], offset: &mut usize, v: T) -> Result<(), Error> {
    let bytes = v.to_le_bytes();
    write_bytes(dest, offset, bytes.as_ref())
}

fn write_u8(dest: &mut [u8], offset: &mut usize, v: u8) -> Result<(), Error> {
    write_uint(dest, offset, v)
}

fn write_u16_le(dest: &mut [u8], offset: &mut usize, v: u16) -> Result<(), Error> {
    write_uint(dest, offset, v)
}

fn write_u32_le(dest: &mut [u8], offset: &mut usize, v: u32) -> Result<(), Error> {
    write_uint(dest, offset, v)
}

fn write_u64_le(dest: &mut [u8], offset: &mut usize, v: u64) -> Result<(), Error> {
    write_uint(dest, offset, v)
}
