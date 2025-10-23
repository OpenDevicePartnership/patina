//! Security Architectural Protocol
//!
//! Security Architectural Protocol:
//! Abstracts security-specific functions from the DXE Foundation for purposes of handling GUIDed section
//! encapsulations. This protocol must be produced by a boot service or runtime DXE driver and may only be consumed by
//! the DXE Foundation and any other DXE drivers that need to validate the authentication of files.
//!
//! See <https://uefi.org/specs/PI/1.8A/V2_DXE_Architectural_Protocols.html#efi-security-arch-protocol>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use r_efi::efi;

/// Security Arch Protocol GUID.
pub const PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0xA46423E3, 0x4617, 0x49f1, 0xB9, 0xFF, &[0xD1, 0xBF, 0xA9, 0x11, 0x58, 0x39]);

/// The EFI_SECURITY_ARCH_PROTOCOL (SAP) is used to abstract platform-specific
/// policy from the DXE core response to an attempt to use a file that returns a
/// given status for the authentication check from the section extraction protocol.
///
/// The possible responses in a given SAP implementation may include locking
/// flash upon failure to authenticate, attestation logging for all signed drivers,
/// and other exception operations.  The File parameter allows for possible logging
/// within the SAP of the driver.
///
/// If File is NULL, then EFI_INVALID_PARAMETER is returned.
///
/// If the file specified by File with an authentication status specified by
/// AuthenticationStatus is safe for the DXE Core to use, then EFI_SUCCESS is returned.
///
/// If the file specified by File with an authentication status specified by
/// AuthenticationStatus is not safe for the DXE Core to use under any circumstances,
/// then EFI_ACCESS_DENIED is returned.
///
/// If the file specified by File with an authentication status specified by
/// AuthenticationStatus is not safe for the DXE Core to use right now, but it
/// might be possible to use it at a future time, then EFI_SECURITY_VIOLATION is
/// returned.
///
/// @param  this             The EFI_SECURITY_ARCH_PROTOCOL instance.
/// @param  authentication_status
///                          This is the authentication type returned from the Section
///                          Extraction protocol. See the Section Extraction Protocol
///                          Specification for details on this type.
/// @param  file             This is a pointer to the device path of the file that is
///                          being dispatched. This will optionally be used for logging.
///
/// @retval Status::SUCCESS            The file specified by File did authenticate, and the
///                                    platform policy dictates that the DXE Core may use File.
/// @retval Status::INVALID_PARAMETER  Driver is NULL.
/// @retval Status::SECURITY_VIOLATION The file specified by File did not authenticate, and
///                                    the platform policy dictates that File should be placed
///                                    in the untrusted state. A file may be promoted from
///                                    the untrusted to the trusted state at a future time
///                                    with a call to the Trust() DXE Service.
/// @retval Status::ACCESS_DENIED      The file specified by File did not authenticate, and
///                                    the platform policy dictates that File should not be
///                                    used for any purpose.
pub type EfiSecurityFileAuthenticationState = extern "efiapi" fn(
    this: *mut Protocol,
    authentication_status: u32,
    file: *mut efi::protocols::device_path::Protocol,
) -> efi::Status;

/// The EFI_SECURITY_ARCH_PROTOCOL is used to abstract platform-specific policy
/// from the DXE core.  This includes locking flash upon failure to authenticate,
/// attestation logging, and other exception operations.
#[repr(C)]
pub struct Protocol {
    /// Function to check file authentication state. See [`EfiSecurityFileAuthenticationState`] for more details.
    pub file_authentication_state: EfiSecurityFileAuthenticationState,
}
