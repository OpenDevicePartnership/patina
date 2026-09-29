//! BIOS Boot Specification device path nodes.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{BiosSubType, DevicePathType, ascii, check_length, read_ascii};
use crate::uefi::device_path::parse_node::{DevicePathNode, Header};
use alloc::string::String;
use scroll::{
    Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

/// BIOS Boot Specification device path with a NUL-terminated ASCII description.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Bios {
    /// BIOS device type.
    pub device_type: u16,
    /// Status flags.
    pub status_flag: u16,
    /// Description string without the wire terminator.
    pub description_str: String,
}

impl DevicePathNode for Bios {
    fn header(&self) -> Header {
        Header::new(
            DevicePathType::Bios as u8,
            BiosSubType::BiosBootSpecification as u8,
            9 + self.description_str.len(),
        )
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Bios as u8 && subtype == BiosSubType::BiosBootSpecification as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        ascii(&self.description_str)
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for Bios {
    type Error = scroll::Error;
    fn try_from_ctx(buffer: &[u8], ctx: scroll::Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let device_type = buffer.gread_with::<u16>(&mut offset, ctx)?;
        let status_flag = buffer.gread_with::<u16>(&mut offset, ctx)?;
        let description_str = read_ascii(buffer, &mut offset)?;
        let (node, consumed) = (Self { device_type, status_flag, description_str }, offset);
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for Bios {
    type Error = scroll::Error;
    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        dest.gwrite_with(self.device_type, &mut offset, ctx)?;
        dest.gwrite_with(self.status_flag, &mut offset, ctx)?;
        dest.gwrite_with(self.description_str.as_bytes(), &mut offset, ())?;
        dest.gwrite_with(0u8, &mut offset, ctx)?;
        Ok(offset)
    }
}

impl core::fmt::Display for Bios {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}
