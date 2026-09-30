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
/// from the macro, which can override [`measure_size`](Self::measure_size) and
/// [`validate`](Self::validate) in its `@[DevicePathNode(...)]` attribute.
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
    /// as a reserved value, are rejected. Nodes whose fields have no specification
    /// rules return `Ok(())`.
    ///
    /// # Errors
    ///
    /// Returns a description of the first rule the node breaks.
    fn validate(&self) -> Result<(), &'static str>;

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
/// Tuple structs are not supported.
///
/// ### `@[DevicePathNode(<type>, <sub_type>, (<override fn> = <fn>),*)]`
///
/// #### Parameters
///
/// - `<type>` (required): device path type, an enum value that converts to `u8`, from
///   [`node_types`](crate::uefi::device_path::node::node_types).
/// - `<sub_type>` (required): device path sub-type, an enum value that converts to `u8`,
///   from [`node_types`](crate::uefi::device_path::node::node_types).
/// - `<function to override> = <function to use>` (optional): replaces the default
///   implementation of a [`DevicePathNode`] method with a function that takes the node
///   by reference. Can be repeated, in any order.
///
///   The methods that can be overridden are:
///
///   | Method         | Default                                                        |
///   |----------------|----------------------------------------------------------------|
///   | `measure_size` | Sum of the fields' sizes; only correct for fixed-size fields.  |
///   | `validate`     | `Ok(())`: every value is accepted.                             |
///
/// ### `@[DevicePathNodeDerive((<Trait>,)*)]`
///
/// #### Parameters
///
/// - `<Trait>` (optional): trait to implement. Can be repeated. The traits that can be
///   implemented are:
///
///   | Trait     | Output                          |
///   |-----------|---------------------------------|
///   | `Debug`   | The header, then every field.   |
///   | `Display` | `Name(field, ...)`              |
///
/// # Example
///
/// ```
/// use patina::{
///     device_path_node,
///     uefi::device_path::{
///         node::{DevicePathType, HardwareSubType},
///         parse_node::DevicePathNode,
///     },
/// };
/// use scroll::{Pread, Pwrite};
///
/// device_path_node! {
///     /// Foo device path node.
///     @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Vendor, validate = foo_validate)]
///     @[DevicePathNodeDerive(Debug, Display)]
///     #[derive(Pread, Pwrite, Clone)]
///     pub struct Foo {
///         /// Bar value.
///         pub bar: u16,
///         /// Baz value, which must not be zero.
///         pub baz: u8,
///     }
/// }
///
/// fn foo_validate(node: &Foo) -> Result<(), &'static str> {
///     if node.baz == 0 {
///         return Err("baz must not be zero");
///     }
///     Ok(())
/// }
///
/// // measure_size is not overridden, so it is the sum of the field sizes.
/// assert_eq!(Foo { bar: 1, baz: 2 }.measure_size(), 3);
/// assert_eq!(Foo { bar: 1, baz: 0 }.validate(), Err("baz must not be zero"));
/// ```
#[macro_export]
macro_rules! device_path_node {
    // Entry point of the macro.
    //
    // Match the definition of a struct with decorators that configure the device path node.
    // This supports structs with fields and unit structs. Tuple structs are not supported.
    //
    // @[DevicePathNode(type, sub_type, method = path::to::function, ...)]
    // This is used to configure the device path node. The `method = function` pairs are
    // optional, can come in any order, and override `measure_size` or `validate`.
    //
    // @[DevicePathNodeDerive(Trait, ...)]
    // This is used to implement traits with a default implementation for device path nodes.
    // The default implementations and supported traits are in the matching `@Derive <Trait>` arms.
    //
    // `$unit_struct` is always empty: nothing can follow the `;` of a unit struct. It only
    // gives the output a variable to repeat, so that the `;` is emitted for unit structs.
    (
        $(#[$struct_attr_1:meta])*
        @[DevicePathNode( $device_path_type:path, $device_path_sub_type:path $(, $override_method:ident = $func:path)* $(,)? )]
        $(@[DevicePathNodeDerive( $($derive_trait:ident),* )])?
        $(#[$struct_attr_2:meta])*
        $struct_vis:vis struct $struct_name:ident
        $({
            $(
                $(#[$field_attr:meta])*
                $field_vis:vis $field_name:ident: $field_type:ty
            ),* $(,)?
        })?
        $(; $($unit_struct:tt)?)?
    ) => {
        $(#[$struct_attr_1])*
        $(#[$struct_attr_2])*
        $struct_vis struct $struct_name
        $({
            $(
                $(#[$field_attr])*
                $field_vis $field_name: $field_type
            ),*
        })?
        $(; $($unit_struct)?)?

        impl $crate::uefi::device_path::parse_node::DevicePathNode for $struct_name {
            fn r#type() -> u8 {
                $device_path_type as u8
            }

            fn sub_type() -> u8 {
                $device_path_sub_type as u8
            }

            // The other methods are the user-defined overrides, or the defaults when not overridden.
            $(device_path_node!(@Override $override_method = $func);)*
            device_path_node!(@Default measure_size [$($($field_name: $field_type),*)?], $($override_method)*);
            device_path_node!(@Default validate [$($($field_name: $field_type),*)?], $($override_method)*);
        }

        // Add the implementation of each requested trait.
        device_path_node!(
            @Derives $struct_name, [$($($field_name: $field_type),*)?], $($($derive_trait),*)?
        );
    };
    // Call @Derive once per derive trait. The fields arrive as one token tree ($fields)
    // so they can be repeated for each trait, which the entry arm cannot do directly.
    (@Derives $struct_name:ident, $fields:tt, $($derive_trait:ident),*) => {
        $(device_path_node!(@Derive $derive_trait $struct_name, $fields);)*
    };

    // ################################################################################
    // @Override <device path node trait method> = <path to function>
    //
    // This is used to allow a custom function to be used in the implementation of the
    // DevicePathNode trait. The generated method calls the function with the node.
    //
    // ################################################################################

    // Match the override of `measure_size`: `measure_size = function`.
    (@Override measure_size = $func:path) => {
        fn measure_size(&self) -> usize {
            $func(self)
        }
    };
    // Match the override of `validate`: `validate = function`.
    (@Override validate = $func:path) => {
        fn validate(&self) -> Result<(), &'static str> {
            $func(self)
        }
    };
    // Match any other method name ($other), which is a compile error.
    (@Override $other:ident = $func:path) => {
        compile_error!(concat!("`", stringify!($other), "` cannot be overridden in device_path_node!"));
    };

    // ################################################################################
    // @Default
    //
    // This is used to implement the default behavior for DevicePathNode trait.
    //
    // `@Default <device path node trait method> <fields>, <overridden methods>` is called
    // once per method, and walks the list of overridden methods:
    // - noop: the method is next in the list, so @Override already implements it.
    // - skip: another method is next in the list, so keep walking.
    // - default: the list is empty, so emit the default implementation.
    //
    // ################################################################################

    // Match the default for a device path method that has been overridden,
    // and do nothing since the implementation is added by @Override.
    (@Default measure_size $fields:tt, measure_size $($rest:ident)*) => {};
    (@Default validate     $fields:tt, validate     $($rest:ident)*) => {};

    // Match `measure_size` default implementation.
    //
    // The default implementation is the size of every field.
    (@Default measure_size [$($field_name:ident: $field_type:ty),*],) => {
        fn measure_size(&self) -> usize {
            0 $(+ core::mem::size_of::<$field_type>())*
        }
    };

    // Match `validate` default implementation.
    //
    // By default the node is considered valid.
    (@Default validate $fields:tt,) => {
        fn validate(&self) -> Result<(), &'static str> {
            Ok(())
        }
    };

    // Match an override for another method: skip it and keep checking the rest of the
    // override list.
    //
    // Must stay after the noop matches above, which catch the method being overridden.
    (@Default $method:ident $fields:tt, $override_method:ident $($rest:ident)*) => {
        device_path_node!(@Default $method $fields, $($rest)*);
    };

    // ################################################################################
    // @Derive <Trait> <name of the node struct>, [<field name>: <field type>, ...]
    //
    // This is used to implement traits with a default behavior for device path nodes.
    //
    // ################################################################################

    // Match `Debug`. Prints the header followed by every field.
    (@Derive Debug $struct_name:ident, [$($field_name:ident: $field_type:ty),*]) => {
        impl core::fmt::Debug for $struct_name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                let header = $crate::uefi::device_path::parse_node::DevicePathNode::header(self);
                // Header is packed, so copy the length out before borrowing it.
                let length = header.length;
                f.debug_struct(stringify!($struct_name))
                    .field("type", &header.r#type)
                    .field("sub_type", &header.sub_type)
                    .field("length", &length)
                    $(.field(stringify!($field_name), &self.$field_name))*
                    .finish()
            }
        }
    };
    // Match `Display`. Prints `Name(field, ...)`.
    (@Derive Display $struct_name:ident, [$($field_name:ident: $field_type:ty),*]) => {
        impl core::fmt::Display for $struct_name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.debug_tuple(stringify!($struct_name))
                    $(.field(&self.$field_name))*
                    .finish()
            }
        }
    };
    // Match any other trait name ($other), which is a compile error.
    (@Derive $other:ident $struct_name:ident, $fields:tt) => {
        compile_error!(concat!("`", stringify!($other), "` cannot be derived by device_path_node!"));
    };
}

#[cfg(test)]
mod tests {
    use super::DevicePathNode;
    use crate::uefi::device_path::node::{DevicePathType, HardwareSubType};
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
        @[DevicePathNode(
            DevicePathType::Hardware, HardwareSubType::Vendor,
            validate = checked_validate,
        )]
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
        @[DevicePathNode(
            DevicePathType::Hardware, HardwareSubType::Vendor,
            validate = overridden_validate,
            measure_size = overridden_measure_size,
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
    }
}
