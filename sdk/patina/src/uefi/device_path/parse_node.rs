//! Defines device path nodes and methods for creating and parsing them.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use super::{
    encoding::invalid,
    node::{GenericDevicePathNode, Header},
};
use core::{
    fmt::{Debug, Display},
    marker::Sized,
};

use scroll::{
    self, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

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
    /// [`MeasureSize::measure_size`]. If the node is too large for a 16-bit
    /// length, the length saturates at `u16::MAX` and [`write_into`](Self::write_into) fails.
    fn header(&self) -> Header
    where
        Self: Sized,
    {
        let length = Header::size_of_header() + self.measure_size();
        Header::new(Self::r#type(), Self::sub_type(), u16::try_from(length).unwrap_or(u16::MAX))
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
        let expected_length = Header::size_of_header()
            .checked_add(self.measure_size())
            .ok_or(scroll::Error::BadInput { size: usize::MAX, msg: "device path node length overflow" })?;
        if expected_length > usize::from(u16::MAX) {
            return Err(scroll::Error::BadInput { size: expected_length, msg: "device path node is too large" });
        }
        let header = self.header();
        let header_length = usize::from(header.length);
        if header_length != expected_length {
            return Err(scroll::Error::BadInput {
                size: header_length,
                msg: "encoded node length differs from header",
            });
        }
        let mut header_bytes = [0u8; 4];
        header_bytes.pwrite_with(header, 0, scroll::LE)?;
        if buffer.len() < header_length {
            return Err(scroll::Error::TooBig { size: header_length, len: buffer.len() });
        }
        let payload = buffer
            .get_mut(Header::size_of_header()..header_length)
            .ok_or(scroll::Error::BadInput { size: header_length, msg: "invalid device path output range" })?;
        let written = payload.pwrite_with(self, 0, scroll::LE)?;
        if written != header_length - Header::size_of_header() {
            return Err(scroll::Error::BadInput { size: written, msg: "encoded node length differs from header" });
        }
        buffer.pwrite_with(header_bytes.as_slice(), 0, ())?;
        Ok(header_length)
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
        let unknown = GenericDevicePathNode::from_bytes(buffer)?.0;
        if !Self::is_type(unknown.header.r#type, unknown.header.sub_type) {
            return Err(invalid("device path node type does not match"));
        }
        Ok((decode_payload(&unknown.data)?, usize::from(unknown.header.length)))
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
                let length = header.length;
                f.debug_struct(stringify!($struct_name))
                    .field("type", &header.r#type)
                    .field("sub_type", &header.sub_type)
                    .field("length", &length)
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
