//! A module containing UEFI protocol definitions and their implementations of [ProtocolInterface].
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

#[cfg(feature = "unstable-device-path")]
pub mod device_path;
pub mod status_code;

extern crate alloc;

use r_efi::efi;

/// Define a binding between an Interface and the corresponding Guid
///
/// # Safety
///
/// Make sure that the Protocol Guid interface had the same layout that the implementer of this struct.
pub unsafe trait ProtocolInterface {
    /// The GUID of the UEFI protocol being implemented.
    const PROTOCOL_GUID: efi::Guid;
}

macro_rules! impl_r_efi_protocol {
    ($protocol:ident) => {
        unsafe impl ProtocolInterface for r_efi::efi::protocols::$protocol::Protocol {
            const PROTOCOL_GUID: r_efi::efi::Guid = r_efi::efi::protocols::$protocol::PROTOCOL_GUID;
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
