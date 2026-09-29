//! Device path end markers.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{DevicePathType, EndSubType};
use crate::{device_path_node, uefi::device_path::parse_node::DevicePathNode};
use core::fmt::{Display, Write};
use scroll::{
    Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

device_path_node! {
    /// End of entire device path node.
    @[DevicePathNode(DevicePathType::End, EndSubType::Entire)]
    @[DevicePathNodeDerive(Debug, MeasureSize)]
    #[derive(Clone, Copy)]
    pub struct EndEntire;
}

impl Display for EndEntire {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_char('.')
    }
}

impl TryIntoCtx<scroll::Endian> for EndEntire {
    type Error = scroll::Error;

    fn try_into_ctx(self, _: &mut [u8], _: scroll::Endian) -> Result<usize, Self::Error> {
        Ok(0)
    }
}

impl TryFromCtx<'_, scroll::Endian> for EndEntire {
    type Error = scroll::Error;

    fn try_from_ctx(_: &[u8], _: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        Ok((Self, 0))
    }
}

device_path_node! {
    /// End of device path instance node.
    @[DevicePathNode(DevicePathType::End, EndSubType::Instance)]
    @[DevicePathNodeDerive(Debug, MeasureSize)]
    #[derive(Clone, Copy)]
    pub struct EndInstance;
}

impl Display for EndInstance {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_char(';')
    }
}

impl TryIntoCtx<scroll::Endian> for EndInstance {
    type Error = scroll::Error;

    fn try_into_ctx(self, _: &mut [u8], _: scroll::Endian) -> Result<usize, Self::Error> {
        Ok(0)
    }
}

impl TryFromCtx<'_, scroll::Endian> for EndInstance {
    type Error = scroll::Error;

    fn try_from_ctx(_: &[u8], _: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        Ok((Self, 0))
    }
}
