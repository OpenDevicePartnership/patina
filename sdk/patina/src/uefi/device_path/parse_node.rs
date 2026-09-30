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

/// Trait that every device path node must implement.
///
/// Nodes defined with [`device_path_node!`](crate::device_path_node) get this impl
/// from the macro, which can override [`measure_size`](Self::measure_size),
/// [`validate`](Self::validate), and [`header`](Self::header) with
/// `@[DevicePathNodeOverride(...)]`.
pub trait DevicePathNode: Debug + Display {
    /// Return the device path type identifying this node kind.
    fn r#type() -> u8
    where
        Self: Sized;

    /// Return the device path sub-type identifying this node kind.
    fn sub_type() -> u8
    where
        Self: Sized;

    /// Return the encoded payload size in bytes, excluding the four-byte node header.
    ///
    /// It is used to compute the header length, so it must equal the number of bytes
    /// written by the node's payload encoder.
    fn measure_size(&self) -> usize;

    /// Check the node's field values against the UEFI specification.
    ///
    /// Scroll conversions only encode and decode bytes. Device path read and write
    /// entry points call `validate` so that nodes breaking a specification rule, such
    /// as a reserved value, are rejected. The default accepts every value, for nodes
    /// whose fields have no specification rules.
    ///
    /// # Errors
    ///
    /// Returns a description of the first rule the node breaks.
    fn validate(&self) -> Result<(), &'static str> {
        Ok(())
    }

    /// Return the header of the device path node.
    ///
    /// The default combines the node's type, sub-type, and
    /// [`measure_size`](Self::measure_size). If the node is too large for a 16-bit
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
    /// checked with [`validate`](Self::validate) first.
    ///
    /// # Errors
    ///
    /// Returns an error if the node fails validation, the buffer is too small, the
    /// header length does not fit in 16 bits, or the encoded payload size differs
    /// from [`measure_size`](Self::measure_size).
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
    /// trailing bytes, or the node fails [`validate`](Self::validate).
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
    T: DevicePathNode + TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
{
    let (node, consumed) = T::try_from_ctx(data, scroll::LE)?;
    if consumed != data.len() {
        return Err(invalid("unexpected trailing device path payload"));
    }
    node.validate().map_err(invalid)?;
    Ok(node)
}

/// Define a device path node struct and implement [`DevicePathNode`] for it.
///
/// - `@[DevicePathNode(DevicePathType::Type, SubType::MySubType)]` sets the node's type
///   and sub-type. Both are enum paths whose values convert to `u8`.
/// - `@[DevicePathNodeOverride(method = path, ...)]` is optional. Each listed
///   [`DevicePathNode`] method calls the named function, which takes the node by reference, instead of using its default.
///   The methods that can be overridden are `measure_size`, `validate`, and `header`.
///   Without an override, `measure_size` is the sum of the fields' Rust sizes, which is
///   only correct for fixed-width integer and byte-array fields; the other methods use
///   the trait defaults.
/// - `@[DevicePathNodeDerive(...)]` is optional and implements `Debug` and `Display`.
///
/// # Example
///
/// ```ignore
/// device_path_node! {
///     /// VLAN device path node.
///     @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Vlan)]
///     @[DevicePathNodeOverride(validate = vlan_validate)]
///     @[DevicePathNodeDerive(Debug, Display)]
///     #[derive(Pread, Pwrite, Clone)]
///     pub struct Vlan {
///         /// VLAN identifier, from 0 through 4094.
///         pub vlan_id: u16,
///     }
/// }
///
/// fn vlan_validate(node: &Vlan) -> Result<(), &'static str> {
///     if node.vlan_id > 4094 {
///         return Err("invalid VLAN identifier");
///     }
///     Ok(())
/// }
/// ```
#[macro_export]
macro_rules! device_path_node {
    // Match a struct with fields.
    (
        $(#[$struct_attr_1:meta])*
        @[DevicePathNode( $device_path_type:path, $device_path_sub_type:path )]
        $(@[DevicePathNodeOverride( $($method:ident = $func:path),* $(,)? )])?
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

        device_path_node!(
            @ImplDevicePathNode $struct_name, $device_path_type, $device_path_sub_type,
            fields [$($field_type),*],
            overrides [$($($method = $func),*)?]
        );
        device_path_node!(@Derive; $struct_name, $($field_name: $field_type),*; $($($derive_trait),*)?);
    };
    // Match a unit struct.
    (
        $(#[$struct_attr_1:meta])*
        @[DevicePathNode( $device_path_type:path, $device_path_sub_type:path )]
        $(@[DevicePathNodeOverride( $($method:ident = $func:path),* $(,)? )])?
        $(@[DevicePathNodeDerive( $($derive_trait:ident),* )])?
        $(#[$struct_attr_2:meta])*
        $struct_vis:vis struct $struct_name:ident;
    ) => {
        $(#[$struct_attr_1])*
        $(#[$struct_attr_2])*
        $struct_vis struct $struct_name;

        device_path_node!(
            @ImplDevicePathNode $struct_name, $device_path_type, $device_path_sub_type,
            fields [],
            overrides [$($($method = $func),*)?]
        );
        device_path_node!(@Derive; $struct_name, ; $($($derive_trait),*)?);
    };
    // Implement DevicePathNode: type and sub-type, the listed overrides, then the default
    // measure_size unless it is overridden.
    (
        @ImplDevicePathNode $struct_name:ident, $device_path_type:path, $device_path_sub_type:path,
        fields [$($field_type:ty),*],
        overrides [$($method:ident = $func:path),*]
    ) => {
        impl $crate::uefi::device_path::parse_node::DevicePathNode for $struct_name {
            fn r#type() -> u8 {
                $device_path_type as u8
            }

            fn sub_type() -> u8 {
                $device_path_sub_type as u8
            }

            $(device_path_node!(@Override $method = $func);)*

            device_path_node!(@DefaultMeasureSize [$($field_type),*] $($method)*);
        }
    };
    // Overridable DevicePathNode methods. Each one calls the named function.
    (@Override measure_size = $func:path) => {
        fn measure_size(&self) -> usize {
            $func(self)
        }
    };
    (@Override validate = $func:path) => {
        fn validate(&self) -> Result<(), &'static str> {
            $func(self)
        }
    };
    (@Override header = $func:path) => {
        fn header(&self) -> $crate::uefi::device_path::node::Header
        where
            Self: Sized,
        {
            $func(self)
        }
    };
    (@Override $other:ident = $func:path) => {
        compile_error!(concat!("`", stringify!($other), "` cannot be overridden in device_path_node!"));
    };
    // Default measure_size: the sum of the fields' Rust sizes, unless `measure_size` is
    // one of the overridden methods.
    (@DefaultMeasureSize [$($field_type:ty),*] measure_size $($rest:ident)*) => {};
    (@DefaultMeasureSize [$($field_type:ty),*] $other:ident $($rest:ident)*) => {
        device_path_node!(@DefaultMeasureSize [$($field_type),*] $($rest)*);
    };
    (@DefaultMeasureSize [$($field_type:ty),*]) => {
        fn measure_size(&self) -> usize {
            0 $(+ core::mem::size_of::<$field_type>())*
        }
    };
    // Implement Debug.
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
    // Implement Display.
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
    // Reject unsupported derive traits.
    (@Derive; $struct_name:ident, $($field_name:ident: $field_type:ty),*; $trait:ident) => {
        compile_error!("An unsupported derive trait was specified.");
    };
    // Apply each derive trait in turn.
    (@Derive; $struct_name:ident, $($field_name:ident: $field_type:ty),*; $head:ident, $($derive_trait:ident),+) => {
        device_path_node!(@Derive; $struct_name, $($field_name: $field_type),*; $head);
        device_path_node!(@Derive; $struct_name, $($field_name: $field_type),*; $($derive_trait),+);
    };
    // No derive traits.
    (@Derive; $struct_name:ident, $($field_name:ident: $field_type:ty),*; ) => {};
}

#[cfg(test)]
mod tests {
    use super::DevicePathNode;
    use crate::uefi::device_path::node::{DevicePathType, HardwareSubType, Header};
    use scroll::{Pread, Pwrite};

    device_path_node! {
        @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Vendor)]
        @[DevicePathNodeDerive(Debug, Display)]
        #[derive(Pread, Pwrite, Clone)]
        struct Plain {
            value: u16,
            flags: u8,
        }
    }

    device_path_node! {
        @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Vendor)]
        @[DevicePathNodeOverride(validate = checked_validate)]
        @[DevicePathNodeDerive(Debug, Display)]
        #[derive(Pread, Pwrite, Clone)]
        struct Checked {
            value: u16,
        }
    }

    fn checked_validate(node: &Checked) -> Result<(), &'static str> {
        if node.value == 0 {
            return Err("value must not be zero");
        }
        Ok(())
    }

    device_path_node! {
        @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Vendor)]
        @[DevicePathNodeOverride(
            header = overridden_header,
            validate = overridden_validate,
            measure_size = overridden_measure_size
        )]
        @[DevicePathNodeDerive(Debug, Display)]
        #[derive(Pread, Pwrite, Clone)]
        struct Overridden {
            value: u16,
        }
    }

    fn overridden_measure_size(_node: &Overridden) -> usize {
        7
    }

    fn overridden_validate(_node: &Overridden) -> Result<(), &'static str> {
        Err("always rejected")
    }

    fn overridden_header(_node: &Overridden) -> Header {
        Header::new(0x42, 0x24, 11)
    }

    #[test]
    fn test_device_path_node_macro_defaults() {
        let node = Plain { value: 0x1234, flags: 5 };
        assert_eq!(node.measure_size(), 3);
        assert_eq!(node.validate(), Ok(()));

        let mut bytes = [0; 7];
        assert_eq!(node.write_into(&mut bytes).unwrap(), 7);
        assert_eq!(bytes, [1, 4, 7, 0, 0x34, 0x12, 5]);
    }

    #[test]
    fn test_device_path_node_macro_validate_override() {
        assert_eq!(Checked { value: 1 }.validate(), Ok(()));
        assert_eq!(Checked { value: 1 }.measure_size(), 2);

        let invalid = Checked { value: 0 };
        assert_eq!(invalid.validate(), Err("value must not be zero"));
        let mut bytes = [0xa5; 6];
        assert!(invalid.write_into(&mut bytes).is_err());
        assert_eq!(bytes, [0xa5; 6]);
    }

    #[test]
    fn test_device_path_node_macro_overrides_in_any_order() {
        let node = Overridden { value: 1 };
        assert_eq!(node.measure_size(), 7);
        assert_eq!(node.validate(), Err("always rejected"));
        assert_eq!(node.header(), Header::new(0x42, 0x24, 11));
    }
}
