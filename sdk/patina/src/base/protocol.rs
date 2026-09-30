//! Protocol interface abstraction and implementations for spec-defined protocols.
//!
//! Defines the [`ProtocolInterface`] trait that binds a Rust protocol type to its UEFI GUID.
//! Also provides `ProtocolInterface` implementations for the canonical UEFI protocols declared
//! in the [`r-efi`](https://crates.io/crates/r-efi) crate.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::BinaryGuid;

/// Define a binding between an Interface and the corresponding Guid
///
/// # Safety
///
/// Make sure that the Protocol Guid interface had the same layout that the implementer of this struct.
pub unsafe trait ProtocolInterface {
    /// The GUID of the UEFI protocol being implemented.
    const PROTOCOL_GUID: BinaryGuid;
}

/// A marker protocol is a protocol installed with no interface data. Its presence in the protocol database
/// is often used as a signal.
///
/// This trait should be implemented for a zero-sized [`ProtocolInterface`] type that is intended to serve
/// as a marker protocol.
///
/// A marker carries no data, so `Self` must be a zero-sized type.
///
/// # Examples
///
/// ```rust
/// use patina::protocol::{MarkerProtocol, ProtocolInterface};
/// use patina::BinaryGuid;
///
/// struct MyMarker;
///
/// // SAFETY: `MyMarker` has no data, so any correctly-aligned pointer is a valid interface for it.
/// unsafe impl ProtocolInterface for MyMarker {
///     const PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("2b7c1c1e-6b5e-4b7b-9d9d-2c3b8e2f6a11");
/// }
///
/// impl MarkerProtocol for MyMarker {}
/// ```
pub trait MarkerProtocol: ProtocolInterface + Sized {
    /// Compile-time proof that `Self` is zero-sized.
    ///
    /// Marker-specific methods evaluate this to enforce the invariant this trait declares.
    /// Do not reference it directly.
    #[doc(hidden)]
    const ASSERT_ZERO_SIZED: () =
        assert!(core::mem::size_of::<Self>() == 0, "a marker protocol must be a zero-sized type");
}

macro_rules! impl_r_efi_protocol {
    ($protocol:ident) => {
        // SAFETY: This macro implements ProtocolInterface for r_efi protocol types. The PROTOCOL_GUID constant
        // from r_efi matches the protocol interface layout by design - r_efi provides the canonical UEFI
        // protocol definitions and GUIDs from the UEFI specification. The Protocol struct layout matches
        // the UEFI protocol interface requirements.
        unsafe impl ProtocolInterface for crate::standard::efi::protocols::$protocol::Protocol {
            const PROTOCOL_GUID: BinaryGuid = BinaryGuid(crate::standard::efi::protocols::$protocol::PROTOCOL_GUID);
        }
    };
}

impl_r_efi_protocol!(absolute_pointer);
impl_r_efi_protocol!(block_io);
impl_r_efi_protocol!(bus_specific_driver_override);
impl_r_efi_protocol!(debug_support);
impl_r_efi_protocol!(debugport);
impl_r_efi_protocol!(decompress);
impl_r_efi_protocol!(device_path);
impl_r_efi_protocol!(device_path_from_text);
impl_r_efi_protocol!(device_path_utilities);
impl_r_efi_protocol!(disk_io);
impl_r_efi_protocol!(disk_io2);
impl_r_efi_protocol!(driver_binding);
impl_r_efi_protocol!(driver_diagnostics2);
impl_r_efi_protocol!(driver_family_override);
// protocol file ???;
impl_r_efi_protocol!(graphics_output);
impl_r_efi_protocol!(hii_database);
impl_r_efi_protocol!(hii_font);
impl_r_efi_protocol!(hii_font_ex);
// protocol hii_package_list ???;
impl_r_efi_protocol!(hii_string);
impl_r_efi_protocol!(ip4);
impl_r_efi_protocol!(ip6);
impl_r_efi_protocol!(load_file);

// Clashing implementation
// impl_r_efi_protocol!(load_file2);
impl_r_efi_protocol!(loaded_image);

// Clashing implementation
// efi::protocols::loaded_image::Protocol,
// efi::protocols::loaded_image_device_path::PROTOCOL_GUID

impl_r_efi_protocol!(managed_network);
impl_r_efi_protocol!(mp_services);
impl_r_efi_protocol!(pci_io);
impl_r_efi_protocol!(platform_driver_override);
impl_r_efi_protocol!(rng);
// protocol service_binding ???
impl_r_efi_protocol!(shell);
impl_r_efi_protocol!(shell_dynamic_command);
impl_r_efi_protocol!(shell_parameters);
impl_r_efi_protocol!(simple_file_system);
impl_r_efi_protocol!(simple_network);
impl_r_efi_protocol!(simple_text_input);
impl_r_efi_protocol!(simple_text_input_ex);
impl_r_efi_protocol!(simple_text_output);
impl_r_efi_protocol!(tcp4);
impl_r_efi_protocol!(tcp6);
impl_r_efi_protocol!(timestamp);
impl_r_efi_protocol!(udp4);
impl_r_efi_protocol!(udp6);
