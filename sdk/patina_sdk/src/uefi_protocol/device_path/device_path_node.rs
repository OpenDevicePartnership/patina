//! This module defines device path nodes and methods for creating and parsing them.

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

use super::nodes;

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
        let mut offset = 0;
        dest.gwrite_with(self.r#type, &mut offset, ctx)?;
        dest.gwrite_with(self.sub_type, &mut offset, ctx)?;
        dest.gwrite_with(self.length as u16, &mut offset, ctx)?;
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

/// Trait that every device path node must implement.
pub trait DevicePathNode: Debug + Display {
    /// Return the header of the device path node.
    fn header(&self) -> Header;

    /// Return true if this device path node has the same type and sub_type.
    fn is_type(r#type: u8, sub_type: u8) -> bool
    where
        Self: Sized;

    /// Write the device path node into the buffer and return the number of bytes written.
    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error>;
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
    /// Cast the Unknown device path to a dyn DevicePathNode of the right type.
    pub fn cast_to_dyn_device_path_node(self) -> Box<dyn DevicePathNode + 'a> {
        nodes::cast_to_dyn_device_path_node(self)
    }
}

impl DevicePathNode for UnknownDevicePathNode<'_> {
    fn header(&self) -> Header {
        self.header
    }

    fn is_type(_type: u8, _sub_type: u8) -> bool {
        // An unknown device type can represent every type so it always return true.
        true
    }

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error> {
        let mut offset = 0;
        buffer.gwrite_with(self, &mut offset, Endian::Little)?;
        Ok(offset)
    }
}

impl TryIntoCtx<scroll::Endian> for UnknownDevicePathNode<'_> {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], _ctx: scroll::Endian) -> Result<usize, Self::Error> {
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
        if header.length > from.len() {
            return Err(scroll::Error::TooBig { size: header.length, len: from.len() });
        }
        let this = UnknownDevicePathNode { header, data: &from[offset..header.length] };
        Ok((this, header.length))
    }
}

impl Display for UnknownDevicePathNode<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!("Path({}, {},", &self.header.r#type, &self.header.sub_type))?;
        for b in self.data {
            f.write_fmt(format_args!(" {:02X}", b))?;
        }
        f.write_char(')')
    }
}

/// This macro is used to define a device path node struct and implement device path node traits for it.
/// To configure the type and subtype of this node, use an attribute `@[DevicePathNode(DevicePathType::Type, SubType::MySubtype)]`
/// where the Type and subtype are enum paths where the value can be expressed as u8.a
///
/// Some additional traits can be implemented with `@[DevicePathNodeDerive(...)]`
/// Currently supported traits are: Debug and Display.
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
        device_path_node!(@Derive; $struct_name, $($field_name),*; $($($derive_trait),*)?);
    };
    // Match an empty struct.
    (
        $(#[$struct_attr_1:meta])*
        @[DevicePathNode( $device_path_type:path, $device_path_sub_type:path)]
        $(@[DevicePathNodeDerive( $($derive_trait:ident),* )])?
        $(#[$struct_attr_2:meta])*
        $struct_vis:vis struct $struct_name:ident $($empty_field:ident),*;
    ) => {
        $(#[$struct_attr_1])*
        $(#[$struct_attr_2])*
        $struct_vis struct $struct_name; $($empty_field),* // no empty field expected, the variable is there because we need it to be empty.

        device_path_node!(@ImplDevicePathNode; $device_path_type, $device_path_sub_type, $struct_name);
        device_path_node!(@Derive; $struct_name, $($empty_field),*; $($($derive_trait),*)?);
    };
    // Internal Matching to implement the device path node trait.
    (@ImplDevicePathNode; $device_path_type:path, $device_path_sub_type:path, $struct_name:ident) => {
        impl $crate::uefi_protocol::device_path::device_path_node::DevicePathNode for $struct_name
        {
            fn header(&self) -> $crate::uefi_protocol::device_path::device_path_node::Header {
                $crate::uefi_protocol::device_path::device_path_node::Header {
                    r#type: $device_path_type as u8,
                    sub_type: $device_path_sub_type as u8,
                    length: $crate::uefi_protocol::device_path::device_path_node::Header::size_of_header() + core::mem::size_of::<$struct_name>()

                }
            }

            fn is_type(r#type: u8, sub_type: u8) -> bool {
                r#type == $device_path_type as u8 && sub_type == $device_path_sub_type as u8
            }

            fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error> {
                let header = self.header();
                debug_assert!(header.length >= buffer.len(), "Buffer to small, can write the device path node.");

                let mut offset = 0;
                buffer.gwrite_with(header, &mut offset, scroll::Endian::Little)?;
                buffer.gwrite_with(self, &mut offset, scroll::Endian::Little)?;
                Ok(offset)
            }
        }
    };
    // Internal Matching to implement the debug trait.
    (@Derive; $struct_name:ident, $($field_name:ident),*; Debug) => {
        impl core::fmt::Debug for $struct_name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                let header = self.header();
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
    (@Derive; $struct_name:ident, $($field_name:ident),*; Display) => {
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
    // Internal matching to add a compilation error in case of an unsupported derive trait.
    (@Derive; $struct_name:ident, $($field_name:ident),*; $trait:ident) => {
        compile_error!("An unsupported derive trait was specified.");
    };
    // Internal matching to call all the derive to add their impl to the struct.
    (@Derive; $struct_name:ident, $($field_name:ident),*; $head:ident, $($derive_trait:ident),+) => {
        device_path_node!(@Derive; $struct_name, $($field_name),*; $head);
        device_path_node!(@Derive; $struct_name, $($field_name),*; $($derive_trait),+);
    };
    // Internal matching: do nothing if no derive specified.
    (@Derive; $struct_name:ident, $($field_name:ident),*; ) => {};
}
