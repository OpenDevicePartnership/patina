//! Defines device path nodes and methods for creating and parsing them.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use super::encoding::invalid;
use alloc::boxed::Box;
use core::{
    clone::Clone,
    fmt::{Debug, Display, Write},
    marker::Sized,
    mem,
};

use scroll::{
    self, Endian, Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

/// Common header of device path nodes.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Header {
    /// Type of the device path node.
    pub r#type: u8,
    /// Subtype of the device path node.
    pub sub_type: u8,
    /// Total length in bytes of the device path node, including the header.
    pub length: usize,
}

impl Header {
    /// Creates a new [`Header`].
    pub const fn new(r#type: u8, sub_type: u8, length: usize) -> Self {
        Self { r#type, sub_type, length }
    }

    /// Return the size of the header in bytes
    pub const fn size_of_header() -> usize {
        mem::size_of::<u8>() + mem::size_of::<u8>() + mem::size_of::<u16>()
    }
}

impl TryIntoCtx<scroll::Endian> for Header {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        let length = u16::try_from(self.length)
            .map_err(|_| scroll::Error::BadInput { size: self.length, msg: "device path node is too large" })?;
        if self.length < Self::size_of_header() {
            return Err(scroll::Error::BadInput {
                size: self.length,
                msg: "device path node is shorter than its header",
            });
        }
        let mut offset = 0;
        dest.gwrite_with(self.r#type, &mut offset, ctx)?;
        dest.gwrite_with(self.sub_type, &mut offset, ctx)?;
        dest.gwrite_with(length, &mut offset, ctx)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for Header {
    type Error = scroll::Error;

    fn try_from_ctx(from: &[u8], ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        Ok((
            Header {
                r#type: from.gread_with(&mut offset, ctx)?,
                sub_type: from.gread_with(&mut offset, ctx)?,
                length: from.gread_with::<u16>(&mut offset, ctx)? as usize,
            },
            offset,
        ))
    }
}

/// Reports the encoded payload size of a device path node.
///
/// The size excludes the four-byte node header and is used to compute the header
/// length. It must equal the number of bytes written by the node's payload encoder.
///
/// Nodes defined with [`device_path_node!`](crate::device_path_node) can derive it
/// with `@[DevicePathNodeDerive(MeasureSize)]`, which sums the fields' Rust sizes.
/// That is only correct for fixed-width integer and byte-array fields; nodes with
/// variable-length or encoded fields must implement it manually.
pub trait MeasureSize {
    /// Return the encoded payload size in bytes, excluding the node header.
    fn measure_size(&self) -> usize;
}

/// Checks the UEFI specification rules on a device path node's field values.
///
/// Scroll conversions only encode and decode bytes. Device path read and write
/// entry points call `validate` so that nodes breaking a specification rule, such
/// as a reserved value, are rejected.
///
/// Nodes whose fields have no specification rules derive it with
/// `@[DevicePathNodeDerive(Validate)]`, which always succeeds.
pub trait Validate {
    /// Check the node's field values against the UEFI specification.
    ///
    /// # Errors
    ///
    /// Returns a description of the first rule the node breaks.
    fn validate(&self) -> Result<(), &'static str>;
}

/// Trait that every device path node must implement.
pub trait DevicePathNode: Debug + Display + MeasureSize + Validate {
    /// Return the device path type identifying this node kind.
    fn r#type() -> u8
    where
        Self: Sized;

    /// Return the device path sub-type identifying this node kind.
    fn sub_type() -> u8
    where
        Self: Sized;

    /// Return the header of the device path node.
    ///
    /// The default combines the node's type, sub-type, and
    /// [`MeasureSize::measure_size`].
    fn header(&self) -> Header
    where
        Self: Sized,
    {
        Header::new(Self::r#type(), Self::sub_type(), Header::size_of_header() + self.measure_size())
    }

    /// Return true if `r#type` and `sub_type` identify this node kind.
    fn is_type(r#type: u8, sub_type: u8) -> bool
    where
        Self: Sized,
    {
        r#type == Self::r#type() && sub_type == Self::sub_type()
    }

    /// Write the device path node, header included, into the buffer.
    ///
    /// Returns the number of bytes written, which is the header length. The node is
    /// checked with [`Validate::validate`] first.
    ///
    /// # Errors
    ///
    /// Returns an error if the node fails validation, the buffer is too small, the
    /// header length does not fit in 16 bits, or the encoded payload size differs
    /// from [`MeasureSize::measure_size`].
    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error>
    where
        Self: Sized + TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        self.validate().map_err(invalid)?;
        let header = self.header();
        let mut header_bytes = [0u8; 4];
        header_bytes.pwrite_with(header, 0, scroll::LE)?;
        if buffer.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: buffer.len() });
        }
        let payload = buffer
            .get_mut(Header::size_of_header()..header.length)
            .ok_or(scroll::Error::BadInput { size: header.length, msg: "invalid device path output range" })?;
        let written = payload.pwrite_with(self, 0, scroll::LE)?;
        if written != header.length - Header::size_of_header() {
            return Err(scroll::Error::BadInput { size: written, msg: "encoded node length differs from header" });
        }
        buffer.pwrite_with(header_bytes.as_slice(), 0, ())?;
        Ok(header.length)
    }

    /// Read a device path node, header included, from the start of the buffer.
    ///
    /// Returns the node and the number of bytes read, which is the header length.
    /// Bytes after the node are ignored.
    ///
    /// # Errors
    ///
    /// Returns an error if the header is malformed or longer than the buffer, the
    /// header does not identify this node kind, the payload cannot be decoded or has
    /// trailing bytes, or the node fails [`Validate::validate`].
    fn read_from<'a>(buffer: &'a [u8]) -> Result<(Self, usize), scroll::Error>
    where
        Self: Sized + TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
    {
        let unknown: UnknownDevicePathNode<'a> = buffer.pread_with(0, scroll::LE)?;
        if !Self::is_type(unknown.header.r#type, unknown.header.sub_type) {
            return Err(invalid("device path node type does not match"));
        }
        Ok((decode_payload(unknown.data)?, unknown.header.length))
    }
}

/// Decode and validate a node payload that must be consumed completely.
pub(crate) fn decode_payload<'a, T>(data: &'a [u8]) -> Result<T, scroll::Error>
where
    T: Validate + TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
{
    let (node, consumed) = T::try_from_ctx(data, scroll::LE)?;
    if consumed != data.len() {
        return Err(invalid("unexpected trailing device path payload"));
    }
    node.validate().map_err(invalid)?;
    Ok(node)
}

/// `UnknownDevicePathNode` represents device path nodes that have not been cast to a more specific associated type
/// or that are undefined in the spec.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UnknownDevicePathNode<'a> {
    /// Header of the device path node.
    pub header: Header,
    /// Data of the device path node, could be anything depending on the header.
    pub data: &'a [u8],
}

impl<'a> UnknownDevicePathNode<'a> {
    /// Cast the Unknown device path to a dyn `DevicePathNode` of the right type.
    pub fn cast_to_dyn_device_path_node(self) -> Box<dyn DevicePathNode + 'a> {
        crate::uefi::device_path::node::cast_to_dyn_device_path_node(self)
    }
}

impl MeasureSize for UnknownDevicePathNode<'_> {
    fn measure_size(&self) -> usize {
        self.data.len()
    }
}

impl Validate for UnknownDevicePathNode<'_> {
    /// Unknown payloads are not interpreted, so there are no field rules to check.
    fn validate(&self) -> Result<(), &'static str> {
        Ok(())
    }
}

impl DevicePathNode for UnknownDevicePathNode<'_> {
    /// Unknown nodes have no fixed type. This placeholder is unused because
    /// `header()` and `is_type()` are overridden.
    fn r#type() -> u8 {
        0
    }

    /// Unknown nodes have no fixed sub-type; see `r#type()`.
    fn sub_type() -> u8 {
        0
    }

    fn header(&self) -> Header {
        self.header
    }

    fn is_type(_type: u8, _sub_type: u8) -> bool {
        // An unknown device type can represent every type so it always return true.
        true
    }

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error>
    where
        Self: TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        let mut offset = 0;
        buffer.gwrite_with(self, &mut offset, Endian::Little)?;
        Ok(offset)
    }

    fn read_from<'b>(buffer: &'b [u8]) -> Result<(Self, usize), scroll::Error>
    where
        Self: TryFromCtx<'b, scroll::Endian, Error = scroll::Error>,
    {
        Self::try_from_ctx(buffer, scroll::LE)
    }
}

impl TryIntoCtx<scroll::Endian> for UnknownDevicePathNode<'_> {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], _ctx: scroll::Endian) -> Result<usize, Self::Error> {
        if self.header.length != 4 + self.data.len() || self.header.length > usize::from(u16::MAX) {
            return Err(scroll::Error::BadInput {
                size: self.header.length,
                msg: "inconsistent device path node length",
            });
        }
        if self.header.r#type == 0x7f && (self.header.length != 4 || !matches!(self.header.sub_type, 1 | 0xff)) {
            return Err(scroll::Error::BadInput { size: self.header.length, msg: "invalid end device path node" });
        }
        let mut offset = 0;
        dest.gwrite_with(self.header, &mut offset, scroll::Endian::Little)?;
        dest.gwrite_with(self.data, &mut offset, ())?;
        Ok(offset)
    }
}

impl<'a> TryFromCtx<'a, scroll::Endian> for UnknownDevicePathNode<'a> {
    type Error = scroll::Error;

    fn try_from_ctx(from: &'a [u8], ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let header = from.gread_with::<Header>(&mut offset, ctx)?;
        if header.length < Header::size_of_header() {
            return Err(scroll::Error::BadInput {
                size: header.length,
                msg: "device path node is shorter than its header",
            });
        }
        if header.length > from.len() {
            return Err(scroll::Error::TooBig { size: header.length, len: from.len() });
        }
        if header.r#type == 0x7f && (header.length != 4 || !matches!(header.sub_type, 1 | 0xff)) {
            return Err(scroll::Error::BadInput { size: header.length, msg: "invalid end device path node" });
        }
        let this = UnknownDevicePathNode {
            header,
            data: from
                .get(offset..header.length)
                .ok_or(scroll::Error::TooBig { size: header.length, len: from.len() })?,
        };
        Ok((this, header.length))
    }
}

impl Display for UnknownDevicePathNode<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!("Path({}, {},", self.header.r#type, self.header.sub_type))?;
        for b in self.data {
            f.write_fmt(format_args!(" {b:02X}"))?;
        }
        f.write_char(')')
    }
}

/// This macro is used to define a device path node struct and implement device path node traits for it.
/// To configure the type and subtype of this node, use an attribute `@[DevicePathNode(DevicePathType::Type, SubType::MySubtype)]`
/// where the Type and subtype are enum paths where the value can be expressed as u8.a
///
/// Some additional traits can be implemented with `@[DevicePathNodeDerive(...)]`
/// Currently supported traits are: Debug, Display, [`MeasureSize`], and [`Validate`].
///
/// The header length is computed from the node's [`MeasureSize`] implementation.
/// Derive it for nodes containing only fixed-width integer and byte-array fields;
/// implement it manually for nodes with variable-length or encoded fields.
#[macro_export]
macro_rules! device_path_node {
    // match a struct with fields.
    (
        $(#[$struct_attr_1:meta])*
        @[DevicePathNode( $device_path_type:path, $device_path_sub_type:path)]
        $(@[DevicePathNodeDerive( $($derive_trait:ident),* )])?
        $(#[$struct_attr_2:meta])*
        $struct_vis:vis struct $struct_name:ident {
            $(
                $(#[$field_attr:meta])*
                $field_vis:vis $field_name:ident: $field_type:ty,
            )*
        }
    ) => {
        $(#[$struct_attr_1])*
        $(#[$struct_attr_2])*
        $struct_vis struct $struct_name {
            $(
                $(#[$field_attr])*
                $field_vis $field_name: $field_type
            ),*
        }

        device_path_node!(@ImplDevicePathNode; $device_path_type, $device_path_sub_type, $struct_name);
        device_path_node!(@Derive; $struct_name, $($field_name: $field_type),*; $($($derive_trait),*)?);
    };
    // Match an empty struct.
    (
        $(#[$struct_attr_1:meta])*
        @[DevicePathNode( $device_path_type:path, $device_path_sub_type:path)]
        $(@[DevicePathNodeDerive( $($derive_trait:ident),* )])?
        $(#[$struct_attr_2:meta])*
        $struct_vis:vis struct $struct_name:ident;
    ) => {
        $(#[$struct_attr_1])*
        $(#[$struct_attr_2])*
        $struct_vis struct $struct_name;

        device_path_node!(@ImplDevicePathNode; $device_path_type, $device_path_sub_type, $struct_name);
        device_path_node!(@Derive; $struct_name, ; $($($derive_trait),*)?);
    };
    // Internal Matching to implement the device path node trait.
    (@ImplDevicePathNode; $device_path_type:path, $device_path_sub_type:path, $struct_name:ident) => {
        impl $crate::uefi::device_path::parse_node::DevicePathNode for $struct_name
        {
            fn r#type() -> u8 {
                $device_path_type as u8
            }

            fn sub_type() -> u8 {
                $device_path_sub_type as u8
            }
        }
    };
    // Internal Matching to implement the debug trait.
    (@Derive; $struct_name:ident, $($field_name:ident: $field_type:ty),*; Debug) => {
        impl core::fmt::Debug for $struct_name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                let header = $crate::uefi::device_path::parse_node::DevicePathNode::header(self);
                f.debug_struct(stringify!($struct_name))
                    .field("type", &header.r#type)
                    .field("sub_type", &header.sub_type)
                    .field("length", &header.length)
                    $(
                        .field(stringify!($field_name), &self.$field_name)
                    )*
                    .finish()
            }
        }
    };
    // Internal Matching to implement the display trait.
    (@Derive; $struct_name:ident, $($field_name:ident: $field_type:ty),*; Display) => {
        impl core::fmt::Display for $struct_name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.debug_tuple(stringify!($struct_name))
                $(
                    .field(&self.$field_name)
                )*
                    .finish()
            }
        }
    };
    // Internal Matching to implement the measure size trait from the fields' Rust sizes.
    (@Derive; $struct_name:ident, $($field_name:ident: $field_type:ty),*; MeasureSize) => {
        impl $crate::uefi::device_path::parse_node::MeasureSize for $struct_name {
            fn measure_size(&self) -> usize {
                0 $(+ core::mem::size_of::<$field_type>())*
            }
        }
    };
    // Internal Matching to implement the validate trait for nodes without field rules.
    (@Derive; $struct_name:ident, $($field_name:ident: $field_type:ty),*; Validate) => {
        impl $crate::uefi::device_path::parse_node::Validate for $struct_name {
            fn validate(&self) -> Result<(), &'static str> {
                Ok(())
            }
        }
    };
    // Internal matching to add a compilation error in case of an unsupported derive trait.
    (@Derive; $struct_name:ident, $($field_name:ident: $field_type:ty),*; $trait:ident) => {
        compile_error!("An unsupported derive trait was specified.");
    };
    // Internal matching to call all the derive to add their impl to the struct.
    (@Derive; $struct_name:ident, $($field_name:ident: $field_type:ty),*; $head:ident, $($derive_trait:ident),+) => {
        device_path_node!(@Derive; $struct_name, $($field_name: $field_type),*; $head);
        device_path_node!(@Derive; $struct_name, $($field_name: $field_type),*; $($derive_trait),+);
    };
    // Internal matching: do nothing if no derive specified.
    (@Derive; $struct_name:ident, $($field_name:ident: $field_type:ty),*; ) => {};
}
