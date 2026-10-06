//! UEFI device path node definitions, grouped by category (UEFI 2.10 Section 10.3).
//!
//! A device path is a series of variable-length nodes. Each node starts at the end
//! of the previous one, so nodes are byte-packed and their fields are unaligned.
//! The six node types are hardware, ACPI, messaging,
//! media, BIOS Boot Specification, and end of hardware device path.
//!
//! Every node starts with the same 4-byte [`Header`]: a type, a sub-type whose
//! meaning depends on the type, and the length of the node in bytes, including the
//! header. The specific device path data follows. [`GenericDevicePathNode`] holds a
//! node with its data kept as raw bytes (Section 10.3.1, Table 10.1).
//!
//! [`Header`]: crate::uefi::device_path::node::Header
//! [`GenericDevicePathNode`]: crate::uefi::device_path::node::GenericDevicePathNode
//!
//! <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#device-path-nodes>
//!
//! ## Defined and custom nodes
//!
//! The nodes of this module are the complete set the specification defines: every other
//! type and sub-type value is reserved
//! ([10.3.1. Generic Device Path Structures](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#generic-device-path-structures)).
//! Do not create nodes with other type or sub-type values.
//!
//! A custom node is a vendor node with its own GUID: [`HardwareVendor`], [`MessagingVendor`],
//! or [`MediaVendor`]. The GUID, allocated by the vendor, defines the data that follows it,
//! and never conflicts with future versions of the specification
//! ([10.3.2.4. Vendor Device Path](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vendor-device-path),
//! [10.4.6. Other Rules](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#other-rules)).
//! [`UartFlowControl`] and [`Sas`] are vendor nodes that the specification itself defines.
//!
//! A device path read from firmware may still hold a node of a type that is not known,
//! such as one added by a later version of the specification. It is kept as a
//! [`GenericDevicePathNode`] and can be skipped using its length
//! ([10.4.1. Housekeeping Rules](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#housekeeping-rules)).
//!
//! [`HardwareVendor`]: crate::uefi::device_path::node::HardwareVendor
//! [`MessagingVendor`]: crate::uefi::device_path::node::MessagingVendor
//! [`MediaVendor`]: crate::uefi::device_path::node::MediaVendor
//! [`UartFlowControl`]: crate::uefi::device_path::node::UartFlowControl
//! [`Sas`]: crate::uefi::device_path::node::Sas
//!
//! ## Conversions
//!
//! Binary conversions use `DevicePathNode` and the `scroll` conversion traits. Nodes
//! display in the UEFI text representation; see [format flags](crate::uefi::device_path#format-flags).
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

// cspell:ignore elems rsplit BCDEFGHIJ
pub mod acpi;
pub mod bios;
pub mod end;
pub mod hardware;
pub mod media;
pub mod messaging;
pub mod node_types;

pub use acpi::*;
pub use bios::*;
pub use end::*;
pub use hardware::*;
pub use media::*;
pub use messaging::*;
pub use node_types::*;

use super::{DevicePath, encoding::invalid};
use crate::BinaryGuid;
use alloc::boxed::Box;
use core::fmt::{self, Debug, Display};
use scroll::{
    Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

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
    /// length, the length saturates at `u16::MAX` and appending the node fails.
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
}

/// Define a device path node struct and implement [`DevicePathNode`] for it.
///
/// The macro also implements `TryFrom<&GenericDevicePathNode>`, which checks the node
/// type, decodes the payload, rejects trailing bytes, and calls
/// [`validate`](DevicePathNode::validate). The struct must implement scroll's
/// `TryFromCtx`, for example with `#[derive(Pread)]`.
///
/// Tuple structs are not supported.
///
/// ### `@[DevicePathNode(<type>, <sub_type>, (<override fn> = <fn>),*)]`
///
/// #### Parameters
///
/// - `<type>` (required): device path type, an enum value that converts to `u8`, from
///   [`node_types`].
/// - `<sub_type>` (required): device path sub-type, an enum value that converts to `u8`,
///   from [`node_types`].
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
///     uefi::device_path::node::{DevicePathNode, DevicePathType, HardwareSubType},
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

        impl $crate::uefi::device_path::node::DevicePathNode for $struct_name {
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

        // Decode the node from a generic node: check the type, decode the payload, which
        // must be consumed completely, then check the specification rules.
        impl core::convert::TryFrom<&$crate::uefi::device_path::node::GenericDevicePathNode> for $struct_name {
            type Error = ::scroll::Error;

            fn try_from(
                node: &$crate::uefi::device_path::node::GenericDevicePathNode,
            ) -> Result<Self, Self::Error> {
                use $crate::uefi::device_path::node::DevicePathNode as _;
                let header = node.header;
                if !Self::is_type(header.r#type, header.sub_type) {
                    return Err(::scroll::Error::BadInput { size: 0, msg: "device path node type does not match" });
                }
                let (decoded, consumed) =
                    <Self as ::scroll::ctx::TryFromCtx<::scroll::Endian>>::try_from_ctx(&node.data, ::scroll::LE)?;
                if consumed != node.data.len() {
                    return Err(::scroll::Error::BadInput {
                        size: node.data.len(),
                        msg: "unexpected trailing device path payload",
                    });
                }
                decoded.validate().map_err(|msg| ::scroll::Error::BadInput { size: 0, msg })?;
                Ok(decoded)
            }
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
                let header = $crate::uefi::device_path::node::DevicePathNode::header(self);
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

#[cfg(target_endian = "big")]
compile_error!("UEFI device path node headers use native u16 fields and require a little-endian target");

/// Header common to every device path node (UEFI 2.10 Table 10.1).
///
/// The type, the sub-type whose meaning depends on the type, and the node length.
/// The UEFI wire format stores the length as little-endian; this in-memory type
/// uses a native `u16` and therefore assumes a little-endian target.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
pub struct Header {
    /// Type of the device path node.
    pub r#type: u8,
    /// Subtype of the device path node.
    pub sub_type: u8,
    /// Total length in bytes of the device path node, including the header.
    pub length: u16,
}

impl Header {
    /// Creates a new [`Header`].
    pub const fn new(r#type: u8, sub_type: u8, length: u16) -> Self {
        Self { r#type, sub_type, length }
    }

    /// Return the size of the header in bytes
    pub const fn size_of_header() -> usize {
        core::mem::size_of::<Self>()
    }
}

impl TryIntoCtx<scroll::Endian> for Header {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        if usize::from(self.length) < Self::size_of_header() {
            return Err(scroll::Error::BadInput {
                size: usize::from(self.length),
                msg: "device path node is shorter than its header",
            });
        }
        let mut offset = 0;
        dest.gwrite_with(self.r#type, &mut offset, ctx)?;
        dest.gwrite_with(self.sub_type, &mut offset, ctx)?;
        dest.gwrite_with(self.length, &mut offset, ctx)?;
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
                length: from.gread_with::<u16>(&mut offset, ctx)?,
            },
            offset,
        ))
    }
}

/// Generic device path node (UEFI 2.10 Table 10.1).
///
/// A node whose payload is kept as raw bytes: the specific device path data that its
/// type and sub-type define. It represents any node, including nodes that are not
/// decoded into a specific type or whose type is not defined by the specification.
///
/// # Examples
///
/// ```
/// use patina::uefi::device_path::node::GenericDevicePathNode;
///
/// let bytes = [9, 2, 6, 0, 0xaa, 0xbb];
/// let (node, remaining) = GenericDevicePathNode::from_bytes(&bytes).unwrap();
/// assert!(remaining.is_empty());
/// assert_eq!(node.to_string(), "Path(9,2,aabb)");
/// // Display-only (`-`), without shortcuts (`#`), and both.
/// assert_eq!(format!("{node:-}"), "Path(9,2,aabb)");
/// assert_eq!(format!("{node:#}"), "Path(9,2,aabb)");
/// assert_eq!(format!("{node:-#}"), "Path(9,2,aabb)");
/// ```
#[repr(C)]
#[derive(Debug, Eq, PartialEq, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
pub struct GenericDevicePathNode {
    /// Header of the device path node.
    pub header: Header,
    /// Data of the device path node, could be anything depending on the header.
    pub data: [u8],
}

impl GenericDevicePathNode {
    /// Borrows the first device path node from `bytes` and returns the remaining bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the buffer is shorter than a header, the header length is
    /// shorter than the header or longer than the buffer, or an end node has an
    /// invalid subtype or length.
    pub fn from_bytes(bytes: &[u8]) -> Result<(&Self, &[u8]), scroll::Error> {
        let header = bytes.pread_with::<Header>(0, scroll::LE)?;
        let length = usize::from(header.length);
        if length < Header::size_of_header() {
            return Err(scroll::Error::BadInput { size: length, msg: "device path node is shorter than its header" });
        }
        if length > bytes.len() {
            return Err(scroll::Error::TooBig { size: length, len: bytes.len() });
        }
        if header.r#type == 0x7f && (length != Header::size_of_header() || !matches!(header.sub_type, 1 | 0xff)) {
            return Err(scroll::Error::BadInput { size: length, msg: "invalid end device path node" });
        }
        <Self as zerocopy::FromBytes>::ref_from_prefix_with_elems(bytes, length - Header::size_of_header())
            .map_err(|_| scroll::Error::BadInput { size: length, msg: "invalid device path node layout" })
    }

    /// Returns this node's exact in-memory bytes.
    pub fn as_bytes(&self) -> &[u8] {
        <Self as zerocopy::IntoBytes>::as_bytes(self)
    }
}
fn write_generic(f: &mut fmt::Formatter<'_>, node: &GenericDevicePathNode) -> fmt::Result {
    let name = match node.header.r#type {
        1 => "HardwarePath",
        2 => "AcpiPath",
        3 => "Msg",
        4 => "MediaPath",
        5 => "BbsPath",
        _ => "Path",
    };
    if name == "Path" {
        write!(f, "Path({},{},", node.header.r#type, node.header.sub_type)?;
    } else {
        write!(f, "{name}({},", node.header.sub_type)?;
    }
    write_hex_bytes(f, &node.data)?;
    f.write_str(")")
}

impl fmt::Display for GenericDevicePathNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_generic(f, self)
    }
}

/// Return true if the `-` flag selects the shorter display-only form, which may omit fields.
fn display_only(f: &fmt::Formatter<'_>) -> bool {
    f.sign_minus()
}

/// Return true unless the `#` flag disables the shortcut node names.
fn allow_shortcuts(f: &fmt::Formatter<'_>) -> bool {
    !f.alternate()
}

/// Write bytes as a contiguous lowercase hexadecimal string.
fn write_hex_bytes(f: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    for byte in bytes {
        write!(f, "{byte:02x}")?;
    }
    Ok(())
}

/// Write a GUID in UEFI device path text form.
fn write_guid(f: &mut fmt::Formatter<'_>, guid: &BinaryGuid) -> fmt::Result {
    write!(f, "{guid}")
}

/// Write a vendor node with its GUID and optional hexadecimal payload.
fn write_vendor(f: &mut fmt::Formatter<'_>, name: &str, guid: &BinaryGuid, data: &[u8]) -> fmt::Result {
    write!(f, "{name}(")?;
    write_guid(f, guid)?;
    if !data.is_empty() {
        f.write_str(",")?;
        write_hex_bytes(f, data)?;
    }
    f.write_str(")")
}

macro_rules! try_fmt_node {
    ($generic:expr, $f:expr, $($ty:ty),+ $(,)?) => {
        $(
            if let Ok(node) = <$ty>::try_from($generic) {
                return fmt::Display::fmt(&node, $f);
            }
        )+
    };
}

/// Format a node with the `Display` of its decoded type, or in the generic form if it is unknown.
pub(crate) fn fmt_generic_node(node: &GenericDevicePathNode, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    try_fmt_node!(
        node,
        f,
        Pci,
        PcCard,
        MemoryMapped,
        HardwareVendor,
        Controller,
        Bmc,
        Acpi,
        ExtendedAcpi,
        AcpiAdr,
        NvdimmAcpi,
        Atapi,
        Scsi,
        FiberChannel,
        FiberChannelEx,
        Ieee1394,
        Usb,
        I2oRandomBlockStorageClass,
        MacAddress,
        IpV4,
        IpV6,
        Vlan,
        InfiniBand,
        Uart,
        UartFlowControl,
        Sas,
        MessagingVendor,
        UsbWwid,
        UsbClass,
        DeviceLogicalUnit,
        Sata,
        Iscsi,
        SasEx,
        NvmExpress,
        Uri,
        Ufs,
        Sd,
        Bluetooth,
        WiFi,
        Emmc,
        BluetoothLE,
        Dns,
        Nvdimm,
        RestService,
        NvmeOf,
        MediaVendor,
        CdRom,
        MediaProtocol,
        PiwgFirmwareFile,
        PiwgFirmwareVolume,
        RelativeOffsetRange,
        RamDisk,
        HardDrive,
        FilePath,
        Bios,
        EndInstance,
        EndEntire,
    );
    fmt::Display::fmt(node, f)
}

impl DevicePathNode for &GenericDevicePathNode {
    /// Generic nodes have no fixed type. This placeholder is unused because
    /// `header()` and `is_type()` are overridden.
    fn r#type() -> u8 {
        0
    }

    /// Generic nodes have no fixed sub-type; see `r#type()`.
    fn sub_type() -> u8 {
        0
    }

    fn measure_size(&self) -> usize {
        self.data.len()
    }

    /// Generic payloads are not interpreted. Only the End node rule from the header is
    /// checked: an End node has sub-type 0x01 or 0xFF and no payload.
    fn validate(&self) -> Result<(), &'static str> {
        if self.header.r#type == 0x7f && (!self.data.is_empty() || !matches!(self.header.sub_type, 1 | 0xff)) {
            return Err("invalid end device path node");
        }
        Ok(())
    }

    fn header(&self) -> Header {
        self.header
    }

    fn is_type(_type: u8, _sub_type: u8) -> bool {
        // A generic node can represent every type, so it always returns true.
        true
    }
}

impl TryIntoCtx<scroll::Endian> for &GenericDevicePathNode {
    type Error = scroll::Error;

    /// Write the payload, like the typed nodes; the header is written by the caller.
    fn try_into_ctx(self, dest: &mut [u8], _ctx: scroll::Endian) -> Result<usize, Self::Error> {
        dest.pwrite_with(&self.data, 0, ())
    }
}

impl<'a> TryFromCtx<'a, scroll::Endian> for &'a GenericDevicePathNode {
    type Error = scroll::Error;

    fn try_from_ctx(from: &'a [u8], _ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let (node, _) = GenericDevicePathNode::from_bytes(from)?;
        Ok((node, usize::from(node.header.length)))
    }
}

fn check_length(size: usize) -> Result<(), scroll::Error> {
    if size > usize::from(u16::MAX) - 4 {
        return Err(invalid("device path payload exceeds the 16-bit node length"));
    }
    Ok(())
}

/// Decode a generic node into the node type its header identifies.
///
/// Nodes whose type and sub-type are not defined by the specification are returned as
/// the generic node itself, without interpretation.
///
/// # Errors
///
/// Returns an error for inconsistent lengths, invalid payloads, or trailing bytes
/// in a recognized node.
impl<'a> TryFrom<&'a GenericDevicePathNode> for Box<dyn DevicePathNode + 'a> {
    type Error = scroll::Error;

    fn try_from(unknown: &'a GenericDevicePathNode) -> Result<Self, Self::Error> {
        fn decode_node<'a, T>(node: &'a GenericDevicePathNode) -> Result<Box<dyn DevicePathNode + 'a>, scroll::Error>
        where
            T: DevicePathNode + TryFrom<&'a GenericDevicePathNode, Error = scroll::Error> + 'a,
        {
            Ok(Box::new(T::try_from(node)?))
        }

        if usize::from(unknown.header.length) != Header::size_of_header() + unknown.data.len() {
            return Err(invalid("device path header and payload lengths differ"));
        }
        check_length(unknown.data.len())?;
        if unknown.header.r#type == 0x7f
            && (usize::from(unknown.header.length) != Header::size_of_header()
                || !matches!(unknown.header.sub_type, 1 | 0xff))
        {
            return Err(invalid("invalid end device path node"));
        }

        match unknown.header {
            // Hardware nodes.
            h if Pci::is_type(h.r#type, h.sub_type) => decode_node::<Pci>(unknown),
            h if PcCard::is_type(h.r#type, h.sub_type) => decode_node::<PcCard>(unknown),
            h if MemoryMapped::is_type(h.r#type, h.sub_type) => decode_node::<MemoryMapped>(unknown),
            h if Controller::is_type(h.r#type, h.sub_type) => decode_node::<Controller>(unknown),
            h if Bmc::is_type(h.r#type, h.sub_type) => decode_node::<Bmc>(unknown),
            h if HardwareVendor::is_type(h.r#type, h.sub_type) => decode_node::<HardwareVendor>(unknown),
            // ACPI nodes.
            h if Acpi::is_type(h.r#type, h.sub_type) => decode_node::<Acpi>(unknown),
            h if ExtendedAcpi::is_type(h.r#type, h.sub_type) => decode_node::<ExtendedAcpi>(unknown),
            h if AcpiAdr::is_type(h.r#type, h.sub_type) => decode_node::<AcpiAdr>(unknown),
            h if NvdimmAcpi::is_type(h.r#type, h.sub_type) => decode_node::<NvdimmAcpi>(unknown),
            // Messaging nodes.
            h if NvmExpress::is_type(h.r#type, h.sub_type) => decode_node::<NvmExpress>(unknown),
            h if Sata::is_type(h.r#type, h.sub_type) => decode_node::<Sata>(unknown),
            h if Atapi::is_type(h.r#type, h.sub_type) => decode_node::<Atapi>(unknown),
            h if Scsi::is_type(h.r#type, h.sub_type) => decode_node::<Scsi>(unknown),
            h if FiberChannel::is_type(h.r#type, h.sub_type) => decode_node::<FiberChannel>(unknown),
            h if FiberChannelEx::is_type(h.r#type, h.sub_type) => decode_node::<FiberChannelEx>(unknown),
            h if Ieee1394::is_type(h.r#type, h.sub_type) => decode_node::<Ieee1394>(unknown),
            h if Usb::is_type(h.r#type, h.sub_type) => decode_node::<Usb>(unknown),
            h if UsbWwid::is_type(h.r#type, h.sub_type) => decode_node::<UsbWwid>(unknown),
            h if UsbClass::is_type(h.r#type, h.sub_type) => decode_node::<UsbClass>(unknown),
            h if DeviceLogicalUnit::is_type(h.r#type, h.sub_type) => decode_node::<DeviceLogicalUnit>(unknown),
            h if I2oRandomBlockStorageClass::is_type(h.r#type, h.sub_type) => {
                decode_node::<I2oRandomBlockStorageClass>(unknown)
            }
            h if MacAddress::is_type(h.r#type, h.sub_type) => decode_node::<MacAddress>(unknown),
            h if IpV4::is_type(h.r#type, h.sub_type) => decode_node::<IpV4>(unknown),
            h if IpV6::is_type(h.r#type, h.sub_type) => decode_node::<IpV6>(unknown),
            h if Vlan::is_type(h.r#type, h.sub_type) => decode_node::<Vlan>(unknown),
            h if InfiniBand::is_type(h.r#type, h.sub_type) => decode_node::<InfiniBand>(unknown),
            h if Uart::is_type(h.r#type, h.sub_type) => decode_node::<Uart>(unknown),
            // Vendor nodes whose GUID the specification defines decode as their own type.
            h if UartFlowControl::is_type(h.r#type, h.sub_type)
                && unknown.data.starts_with(UartFlowControl::GUID.as_bytes()) =>
            {
                decode_node::<UartFlowControl>(unknown)
            }
            h if Sas::is_type(h.r#type, h.sub_type) && unknown.data.starts_with(Sas::GUID.as_bytes()) => {
                decode_node::<Sas>(unknown)
            }
            h if MessagingVendor::is_type(h.r#type, h.sub_type) => decode_node::<MessagingVendor>(unknown),
            h if SasEx::is_type(h.r#type, h.sub_type) => decode_node::<SasEx>(unknown),
            h if Iscsi::is_type(h.r#type, h.sub_type) => decode_node::<Iscsi>(unknown),
            h if Uri::is_type(h.r#type, h.sub_type) => decode_node::<Uri>(unknown),
            h if Ufs::is_type(h.r#type, h.sub_type) => decode_node::<Ufs>(unknown),
            h if Sd::is_type(h.r#type, h.sub_type) => decode_node::<Sd>(unknown),
            h if Bluetooth::is_type(h.r#type, h.sub_type) => decode_node::<Bluetooth>(unknown),
            h if WiFi::is_type(h.r#type, h.sub_type) => decode_node::<WiFi>(unknown),
            h if Emmc::is_type(h.r#type, h.sub_type) => decode_node::<Emmc>(unknown),
            h if BluetoothLE::is_type(h.r#type, h.sub_type) => decode_node::<BluetoothLE>(unknown),
            h if Dns::is_type(h.r#type, h.sub_type) => decode_node::<Dns>(unknown),
            h if Nvdimm::is_type(h.r#type, h.sub_type) => decode_node::<Nvdimm>(unknown),
            h if RestService::is_type(h.r#type, h.sub_type) => decode_node::<RestService>(unknown),
            h if NvmeOf::is_type(h.r#type, h.sub_type) => decode_node::<NvmeOf>(unknown),
            // Media nodes.
            h if FilePath::is_type(h.r#type, h.sub_type) => decode_node::<FilePath>(unknown),
            h if HardDrive::is_type(h.r#type, h.sub_type) => decode_node::<HardDrive>(unknown),
            h if CdRom::is_type(h.r#type, h.sub_type) => decode_node::<CdRom>(unknown),
            h if MediaVendor::is_type(h.r#type, h.sub_type) => decode_node::<MediaVendor>(unknown),
            h if MediaProtocol::is_type(h.r#type, h.sub_type) => decode_node::<MediaProtocol>(unknown),
            h if PiwgFirmwareFile::is_type(h.r#type, h.sub_type) => decode_node::<PiwgFirmwareFile>(unknown),
            h if PiwgFirmwareVolume::is_type(h.r#type, h.sub_type) => decode_node::<PiwgFirmwareVolume>(unknown),
            h if RelativeOffsetRange::is_type(h.r#type, h.sub_type) => decode_node::<RelativeOffsetRange>(unknown),
            h if RamDisk::is_type(h.r#type, h.sub_type) => decode_node::<RamDisk>(unknown),
            // BIOS nodes.
            h if Bios::is_type(h.r#type, h.sub_type) => decode_node::<Bios>(unknown),
            // End nodes.
            h if EndEntire::is_type(h.r#type, h.sub_type) => decode_node::<EndEntire>(unknown),
            h if EndInstance::is_type(h.r#type, h.sub_type) => decode_node::<EndInstance>(unknown),
            _ => Ok(Box::new(unknown)),
        }
    }
}

/// Device path node iterator.
pub struct Iter<'a> {
    device_path: &'a DevicePath,
    offset: usize,
}

impl<'a> Iter<'a> {
    pub(super) fn new(device_path: &'a DevicePath) -> Iter<'a> {
        Self { device_path, offset: 0 }
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a GenericDevicePathNode;

    fn next(&mut self) -> Option<Self::Item> {
        // No more byte to read in the device path.
        if self.offset == self.device_path.buffer.len() {
            return None;
        }

        let Ok(unknown_device_path) =
            self.device_path.buffer.gread_with::<&GenericDevicePathNode>(&mut self.offset, scroll::LE)
        else {
            debug_assert!(false, "The buffer is corrupted, could not read the device path node.");
            return None;
        };

        Some(unknown_device_path)
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use alloc::{format, vec, vec::Vec};
    use scroll::{Pread, Pwrite, ctx::TryIntoCtx};

    pub(crate) fn encode_node<T: DevicePathNode + TryIntoCtx<scroll::Endian, Error = scroll::Error>>(
        node: T,
    ) -> Result<Vec<u8>, scroll::Error> {
        let mut bytes = Vec::new();
        crate::uefi::device_path::DevicePathBuf::encode_node(&mut bytes, node)?;
        Ok(bytes)
    }

    pub(crate) fn generic_node_bytes(header: Header, data: &[u8]) -> Vec<u8> {
        let mut bytes = alloc::vec![header.r#type, header.sub_type];
        bytes.extend_from_slice(&header.length.to_le_bytes());
        bytes.extend_from_slice(data);
        bytes
    }

    pub(crate) fn raw_generic_node(bytes: &[u8]) -> &GenericDevicePathNode {
        <GenericDevicePathNode as zerocopy::FromBytes>::ref_from_bytes_with_elems(
            bytes,
            bytes.len() - Header::size_of_header(),
        )
        .unwrap()
    }

    /// Borrow the first node of `bytes` as a generic node, then decode it as `T`.
    pub(crate) fn read<T>(bytes: &[u8]) -> Result<T, scroll::Error>
    where
        T: for<'a> TryFrom<&'a GenericDevicePathNode, Error = scroll::Error>,
    {
        T::try_from(GenericDevicePathNode::from_bytes(bytes)?.0)
    }

    pub(crate) fn check_fixture<T>(node: T, wire: &[u8])
    where
        T: DevicePathNode + Clone + Eq,
        T: for<'a> TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
        T: for<'a> TryFrom<&'a GenericDevicePathNode, Error = scroll::Error>,
        T: TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        encode_node(node.clone()).unwrap();
        assert_eq!(usize::from(node.header().length), wire.len());
        assert!(T::is_type(wire[0], wire[1]));
        assert!(!T::is_type(wire[0] ^ 0x80, wire[1]));
        assert!(!T::is_type(wire[0], wire[1] ^ 0x80));

        assert_eq!(encode_node(node.clone()).unwrap(), wire);

        let unknown: &GenericDevicePathNode = wire.pread_with(0, scroll::LE).unwrap();
        assert_eq!(unknown.header, node.header());
        let mut trailing = wire.to_vec();
        trailing.extend_from_slice(&[0x7f, 0xff, 4, 0]);
        assert_eq!(read::<T>(&trailing).unwrap(), node);
        assert_eq!(T::try_from(unknown).unwrap(), node);
        let mut mismatched = wire.to_vec();
        mismatched[1] ^= 0x80;
        assert!(read::<T>(&mismatched).is_err());
        let dynamic = <Box<dyn DevicePathNode>>::try_from(unknown).unwrap();
        let type_name = core::any::type_name::<T>().rsplit("::").next().unwrap();
        let debug = format!("{dynamic:?}");
        assert!(debug.starts_with(&format!("{type_name} {{")), "unexpected decoded type: {debug}");
        assert_eq!(debug, format!("{node:?}"));
        assert!(!format!("{node}").is_empty());
        let decoded: T = wire.pread_with(4, scroll::LE).unwrap();
        assert_eq!(decoded, node);
        let mut offset = 0;
        let decoded: T = unknown.data.gread_with(&mut offset, scroll::LE).unwrap();
        assert_eq!(decoded, node);
        assert_eq!(offset, wire.len() - 4);
        let mut payload = vec![0xa5; wire.len() - 4];
        assert_eq!(payload.pwrite_with(node.clone(), 0, scroll::LE).unwrap(), wire.len() - 4);
        assert_eq!(payload, &wire[4..]);

        for length in 0..wire.len() {
            assert!(
                wire[..length].pread_with::<&GenericDevicePathNode>(0, scroll::LE).is_err(),
                "accepted truncated node at {length}"
            );
            assert!(read::<T>(&wire[..length]).is_err(), "read truncated node at {length}");
        }
        for length in 0..payload.len() {
            assert!(
                payload[..length].pread_with::<T>(0, scroll::LE).is_err(),
                "accepted truncated payload at {length}"
            );
            let mut short = vec![0xa5; length];
            assert!(short.pwrite_with(node.clone(), 0, scroll::LE).is_err());
        }

        // The scroll context affects scalar fields; opaque byte arrays retain wire order.
        let mut big_endian = vec![0; payload.len()];
        assert_eq!(big_endian.pwrite_with(node.clone(), 0, scroll::BE).unwrap(), payload.len());
        assert_eq!(big_endian.pread_with::<T>(0, scroll::BE).unwrap(), node);
    }

    pub(crate) fn check_invalid<T>(node: T, payload: &[u8])
    where
        T: DevicePathNode + Clone,
        T: for<'a> TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
        T: for<'a> TryFrom<&'a GenericDevicePathNode, Error = scroll::Error>,
        T: TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        assert!(node.validate().is_err());
        assert!(encode_node(node.clone()).is_err());

        assert_eq!(payload.len() + 4, usize::from(node.header().length));
        let mut wire = vec![0; 4];
        wire.pwrite_with(node.header(), 0, scroll::LE).unwrap();
        wire.extend_from_slice(payload);
        assert!(read::<T>(&wire).is_err());
        let bytes = generic_node_bytes(node.header(), payload);
        let unknown = raw_generic_node(&bytes);
        assert!(<Box<dyn DevicePathNode>>::try_from(unknown).is_err());
    }

    pub(crate) fn check_node<T>(node: T, expected: &[u8])
    where
        T: DevicePathNode + Clone + for<'a> TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
        T: TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        assert_eq!(node.measure_size() + 4, expected.len());
        assert_eq!(usize::from(node.header().length), expected.len());
        assert_eq!(encode_node(node.clone()).unwrap(), expected);
        for len in 0..expected.len() {
            assert!(expected[..len].pread_with::<&GenericDevicePathNode>(0, scroll::LE).is_err());
        }
        let (parsed, consumed) = T::try_from_ctx(&expected[4..], scroll::LE).unwrap();
        assert_eq!(consumed + 4, expected.len());
        assert_eq!(encode_node(parsed).unwrap(), expected);
        let raw = expected.pread_with::<&GenericDevicePathNode>(0, scroll::LE).unwrap();
        let decoded = <Box<dyn DevicePathNode>>::try_from(raw).unwrap();
        assert!(!format!("{decoded:?}").starts_with("GenericDevicePathNode"));
    }

    macro_rules! fixture {
        ($test:ident, $node:expr, [$($wire:expr),* $(,)?]) => {
            #[test]
            fn $test() {
                $crate::uefi::device_path::node::test_support::check_fixture($node, &[$($wire),*]);
            }
        };
    }

    pub(crate) use fixture;

    /// Check a node's text in the default, display-only (`-`) and no-shortcut (`#`) forms,
    /// and that the parseable forms parse back to the same bytes.
    macro_rules! check_text {
        ($input:literal, $text:literal, $display_only:literal, $no_shortcuts:literal) => {{
            let bytes = $crate::devpath!($input);
            let path = $crate::uefi::device_path::DevicePath::try_from_bytes(&bytes).expect($input);
            assert_eq!(alloc::format!("{path}"), $text, "default text of {}", $input);
            assert_eq!(alloc::format!("{path:-}"), $display_only, "display-only text of {}", $input);
            assert_eq!(alloc::format!("{path:#}"), $no_shortcuts, "text without shortcuts of {}", $input);
            assert_eq!($crate::devpath!($text), bytes, "default text of {} does not parse back", $input);
            assert_eq!(
                $crate::devpath!($no_shortcuts),
                bytes,
                "text without shortcuts of {} does not parse back",
                $input
            );
        }};
    }

    pub(crate) use check_text;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi::device_path::DevicePathBuf;
    use alloc::{format, vec};
    use core::{fmt, mem};
    use scroll::{Pread, Pwrite, ctx::TryIntoCtx};

    use super::test_support::{encode_node, generic_node_bytes, raw_generic_node};

    #[derive(Debug)]
    struct HugeNode;

    impl fmt::Display for HugeNode {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("HugeNode")
        }
    }

    impl DevicePathNode for HugeNode {
        fn r#type() -> u8 {
            6
        }

        fn sub_type() -> u8 {
            1
        }

        fn measure_size(&self) -> usize {
            usize::from(u16::MAX)
        }

        fn validate(&self) -> Result<(), &'static str> {
            Ok(())
        }
    }

    impl TryIntoCtx<scroll::Endian> for HugeNode {
        type Error = scroll::Error;

        fn try_into_ctx(self, _dest: &mut [u8], _ctx: scroll::Endian) -> Result<usize, Self::Error> {
            Ok(0)
        }
    }

    #[test]
    fn test_device_path_header_layout_is_c_packed() {
        assert_eq!(mem::align_of::<Header>(), 1);
        assert_eq!(mem::size_of::<Header>(), 4);
    }

    #[test]
    fn test_device_path_generic_node_borrows_odd_offset_bytes() {
        let bytes = [0xa5, 6, 1, 7, 0, 1, 2, 3, 0x5a];
        let (node, remaining) = GenericDevicePathNode::from_bytes(&bytes[1..]).unwrap();

        assert_eq!(usize::from(node.header.length), 7);
        assert_eq!(mem::size_of_val(node), 7);
        assert_eq!(node.as_bytes().as_ptr(), bytes[1..].as_ptr());
        assert_eq!(remaining, &[0x5a]);
    }

    #[test]
    fn test_device_path_oversized_default_header_saturates_and_append_fails() {
        let length = HugeNode.header().length;
        assert_eq!(length, u16::MAX);
        assert!(encode_node(HugeNode).is_err());

        assert!(DevicePathBuf::new().append(HugeNode).is_err());
    }

    #[test]
    fn test_device_path_measure_size() {
        // Derived sizes sum encoded fields rather than including Rust layout padding.
        let mapped = MemoryMapped { memory_type: 3, start_address: 0x1000, end_address: 0x1fff };
        assert_eq!(mapped.measure_size(), 20);
        assert_eq!(core::mem::size_of::<MemoryMapped>(), 24);
        assert_eq!(Bmc { interface_type: 1, base_address: 0 }.measure_size(), 9);
        assert_eq!(EndEntire.measure_size(), 0);
        assert_eq!(EndInstance.measure_size(), 0);

        for (hid, uid, cid) in [("", "", ""), ("PNP0A03", "", "PNP0A08"), ("A", "BCDEFGHIJ", "K")] {
            let node = ExtendedAcpi {
                hid: 0,
                uid: 0,
                cid: 0,
                hid_string: hid.into(),
                uid_string: uid.into(),
                cid_string: cid.into(),
            };
            let size = node.measure_size();
            let mut payload = vec![0; size];
            assert_eq!(payload.pwrite_with(node.clone(), 0, scroll::LE).unwrap(), size);
            assert_eq!(usize::from(node.header().length), size + 4);
        }

        let bytes = generic_node_bytes(Header::new(6, 1, 7), &[1, 2, 3]);
        let unknown = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
        assert_eq!(unknown.measure_size(), 3);
    }

    #[test]
    fn test_device_path_decoder_errors_and_unknown_nodes() {
        for bytes in [
            generic_node_bytes(Header::new(1, 1, 4), &[1]),
            generic_node_bytes(Header::new(1, 1, 5), &[1]),
            generic_node_bytes(Header::new(1, 1, 7), &[1, 2, 3]),
            generic_node_bytes(Header::new(3, 18, 10), &[255, 255, 255, 255, 0, 0]),
            generic_node_bytes(Header::new(3, 23, 16), &[0; 12]),
            generic_node_bytes(Header::new(4, 4, 6), &[b'A', 0]),
            generic_node_bytes(Header::new(0x7f, 2, 4), &[]),
            generic_node_bytes(Header::new(0x7f, 1, 5), &[0]),
            generic_node_bytes(Header::new(0x7f, 0xff, 5), &[0]),
        ] {
            let unknown = raw_generic_node(&bytes);
            assert!(<Box<dyn DevicePathNode>>::try_from(unknown).is_err());
        }
        for bytes in
            [generic_node_bytes(Header::new(6, 1, 6), &[1, 2]), generic_node_bytes(Header::new(1, 0xff, 6), &[1, 2])]
        {
            let unknown = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
            let expected = format!("{unknown:?}");
            let decoded = <Box<dyn DevicePathNode>>::try_from(unknown).unwrap();
            assert_eq!(format!("{decoded:?}"), expected);
        }
        let oversized = vec![0; usize::from(u16::MAX) - 3];
        let oversized_length = u16::try_from(4 + oversized.len()).unwrap_or(u16::MAX);
        let bytes = generic_node_bytes(Header::new(6, 1, oversized_length), &oversized);
        let unknown = raw_generic_node(&bytes);
        assert!(<Box<dyn DevicePathNode>>::try_from(unknown).is_err());
    }

    #[test]
    fn test_device_path_checked_lengths_and_dispatch_errors() {
        let mut output = [0u8; 4];
        assert!(Header::new(1, 1, 3).try_into_ctx(&mut output, scroll::LE).is_err());
        for length in 0..4 {
            assert!([1, 1, length, 0].pread_with::<&GenericDevicePathNode>(0, scroll::LE).is_err());
        }
        for data in [&[1u8][..], &[1, 2, 3][..]] {
            let bytes = generic_node_bytes(Header::new(1, 1, u16::try_from(data.len() + 4).unwrap()), data);
            let raw = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
            assert!(<Box<dyn DevicePathNode>>::try_from(raw).is_err());
        }
        let bytes = generic_node_bytes(Header::new(6, 1, 6), &[1, 2]);
        let unknown = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
        assert!(
            format!("{:?}", <Box<dyn DevicePathNode>>::try_from(unknown).unwrap()).starts_with("GenericDevicePathNode")
        );
        assert_eq!(encode_node(unknown).unwrap(), [6, 1, 6, 0, 1, 2]);
    }

    #[test]
    fn test_device_path_builder_propagates_errors() {
        let mut path = DevicePathBuf::new();
        let expected = path.as_bytes().to_vec();
        assert!(path.append(Sata::new(u16::MAX, 0, 0)).is_err());
        assert_eq!(path.as_bytes(), expected);
        assert!(path.append(FilePath::new("bad\0path")).is_err());
        assert_eq!(path.as_bytes(), expected);
    }
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

        assert_eq!(encode_node(node).unwrap(), [1, 4, 7, 0, 0x34, 0x12, 5]);
    }

    #[test]
    fn test_device_path_node_macro_validate_override() {
        assert_eq!(Checked { value: 1 }.validate(), Ok(()));
        assert_eq!(Checked { value: 1 }.measure_size(), 2);

        let invalid = Checked { value: 0 };
        assert_eq!(invalid.validate(), Err("value must not be zero"));
        assert!(encode_node(invalid).is_err());
    }

    #[test]
    fn test_device_path_node_macro_overrides_in_any_order() {
        let node = Overridden { value: 1 };
        assert_eq!(node.measure_size(), 7);
        assert_eq!(node.validate(), Err("always rejected"));
    }

    /// Check the text of each generic node in the three text forms, and that the parseable forms
    /// parse back to the same bytes (UEFI 2.10 Section 10.6.1.3).
    #[test]
    fn test_device_path_text_generic() {
        use crate::uefi::device_path::node::test_support::check_text;

        check_text!(r"Path(9,2,aabb)", r"Path(9,2,aabb)", r"Path(9,2,aabb)", r"Path(9,2,aabb)");
        check_text!(r"HardwarePath(9,aabb)", r"HardwarePath(9,aabb)", r"HardwarePath(9,aabb)", r"HardwarePath(9,aabb)");
        check_text!(r"AcpiPath(9,aabb)", r"AcpiPath(9,aabb)", r"AcpiPath(9,aabb)", r"AcpiPath(9,aabb)");
        check_text!(r"Msg(99,aabb)", r"Msg(99,aabb)", r"Msg(99,aabb)", r"Msg(99,aabb)");
        check_text!(r"MediaPath(99,aabb)", r"MediaPath(99,aabb)", r"MediaPath(99,aabb)", r"MediaPath(99,aabb)");
        check_text!(r"BbsPath(9,aabb)", r"BbsPath(9,aabb)", r"BbsPath(9,aabb)", r"BbsPath(9,aabb)");
    }
}
