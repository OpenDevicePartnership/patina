//! Temporary local implementation for the `EFI_COMPONENT_NAME_PROTOCOL` and `EFI_COMPONENT_NAME2_PROTOCOL`
//! protocols, until they are added to `r-efi`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// Component Name Protocol
#[allow(clippy::module_inception)]
pub mod component_name {
    /// The GUID for the `EFI_COMPONENT_NAME_PROTOCOL`.
    pub const PROTOCOL_GUID: r_efi::base::Guid =
        r_efi::base::Guid::from_fields(0x107a772c, 0xd5e1, 0x11d4, 0x9a, 0x46, &[0x00, 0x90, 0x27, 0x3f, 0xc1, 0x4d]);

    /// Retrieves the driver's user readable name, in the given `Language`.
    pub type ProtocolGetDriverName = unsafe extern "efiapi" fn(
        *mut Protocol,
        *mut r_efi::base::Char8,
        *mut *mut r_efi::base::Char16,
    ) -> r_efi::base::Status;

    /// Retrieves the user readable name of the controller (and optional child handle) managed by the
    /// driver, in the given `Language`.
    pub type ProtocolGetControllerName = unsafe extern "efiapi" fn(
        *mut Protocol,
        r_efi::base::Handle,
        r_efi::base::Handle,
        *mut r_efi::base::Char8,
        *mut *mut r_efi::base::Char16,
    ) -> r_efi::base::Status;

    /// The `EFI_COMPONENT_NAME_PROTOCOL` structure.
    #[repr(C)]
    pub struct Protocol {
        /// Retrieves the driver's user readable name.
        pub get_driver_name: ProtocolGetDriverName,
        /// Retrieves a controller's user readable name.
        pub get_controller_name: ProtocolGetControllerName,
        /// A Null-terminated ASCII string array of the ISO 639-2 language codes this driver supports.
        pub supported_languages: *mut r_efi::base::Char8,
    }
}

/// Component Name 2 Protocol
pub mod component_name2 {
    /// The GUID for the `EFI_COMPONENT_NAME2_PROTOCOL`.
    pub const PROTOCOL_GUID: r_efi::base::Guid =
        r_efi::base::Guid::from_fields(0x6a7a5cff, 0xe8d9, 0x4f70, 0xba, 0xda, &[0x75, 0xab, 0x30, 0x25, 0xce, 0x14]);

    /// Retrieves the driver's user readable name, in the given `Language`.
    pub type ProtocolGetDriverName = unsafe extern "efiapi" fn(
        *mut Protocol,
        *mut r_efi::base::Char8,
        *mut *mut r_efi::base::Char16,
    ) -> r_efi::base::Status;

    /// Retrieves the user readable name of the controller (and optional child handle) managed by the
    /// driver, in the given `Language`.
    pub type ProtocolGetControllerName = unsafe extern "efiapi" fn(
        *mut Protocol,
        r_efi::base::Handle,
        r_efi::base::Handle,
        *mut r_efi::base::Char8,
        *mut *mut r_efi::base::Char16,
    ) -> r_efi::base::Status;

    /// The `EFI_COMPONENT_NAME2_PROTOCOL` structure.
    #[repr(C)]
    pub struct Protocol {
        /// Retrieves the driver's user readable name.
        pub get_driver_name: ProtocolGetDriverName,
        /// Retrieves a controller's user readable name.
        pub get_controller_name: ProtocolGetControllerName,
        /// A Null-terminated ASCII string array of the RFC 4646 language codes this driver supports.
        pub supported_languages: *mut r_efi::base::Char8,
    }
}
