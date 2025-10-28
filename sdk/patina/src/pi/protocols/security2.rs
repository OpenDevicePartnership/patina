//! Security2 Architectural Protocol
//!
//! Abstracts security-specific functions from the DXE Foundation of UEFI Image Verification, Trusted Computing Group
//! (TCG) measured boot, and User Identity policy for image loading and consoles. This protocol must be produced by a
//! boot service or runtime DXE driver.
//!
//! This protocol is optional and must be published prior to the EFI_SECURITY_ARCH_PROTOCOL. As a result, the same
//! driver must publish both of these interfaces.
//!
//! When both Security and Security2 Architectural Protocols are published, LoadImage must use them in accordance with
//! the following rules:
//! - The Security2 protocol must be used on every image being loaded.
//! - The Security protocol must be used after the Security2 protocol and only on images that have been read using
//!   Firmware Volume protocol.
//! - When only Security architectural protocol is published, LoadImage must use it on every image being loaded.
//!
//! See <https://uefi.org/specs/PI/1.8A/V2_DXE_Architectural_Protocols.html#security2-architectural-protocol>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;

use r_efi::efi;

/// Security2 Arch Protocol GUID.
pub const PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0x94ab2f58, 0x1438, 0x4ef1, 0x91, 0x52, &[0x18, 0x94, 0x1a, 0x3a, 0x0e, 0x68]);

/// The DXE Foundation uses this service to measure and/or verify a UEFI image.
///
/// This service abstracts the invocation of Trusted Computing Group (TCG) measured boot, UEFI
/// Secure boot, and UEFI User Identity infrastructure. For the former two, the DXE Foundation
/// invokes the FileAuthentication() with a DevicePath and corresponding image in
/// FileBuffer memory. The TCG measurement code will record the FileBuffer contents into the
/// appropriate PCR. The image verification logic will confirm the integrity and provenance of the
/// image in FileBuffer of length FileSize . The origin of the image will be DevicePath in
/// these cases.
/// If the FileBuffer is NULL, the interface will determine if the DevicePath can be connected
/// in order to support the User Identification policy.
///
/// @param  this             The EFI_SECURITY2_ARCH_PROTOCOL instance.
/// @param  file             A pointer to the device path of the file that is
///                          being dispatched. This will optionally be used for logging.
/// @param  file_buffer      A pointer to the buffer with the UEFI file image.
/// @param  file_size        The size of the file.
/// @param  boot_policy      A boot policy that was used to call LoadImage() UEFI service. If
///                          FileAuthentication() is invoked not from the LoadImage(),
///                          BootPolicy must be set to FALSE.
///
/// @retval Status::SUCCESS         The file specified by DevicePath and non-NULL
///                                 FileBuffer did authenticate, and the platform policy dictates
///                                 that the DXE Foundation may use the file.
/// @retval Status::SUCCESS         The device path specified by NULL device path DevicePath
///                                 and non-NULL FileBuffer did authenticate, and the platform
///                                 policy dictates that the DXE Foundation may execute the image in
///                                 FileBuffer.
/// @retval Status::SUCCESS         FileBuffer is NULL and current user has permission to start
///                                 UEFI device drivers on the device path specified by DevicePath.
/// @retval Status::SECURITY_VIOLATION  The file specified by DevicePath and FileBuffer did not
///                                     authenticate, and the platform policy dictates that the file should be
///                                     placed in the untrusted state. The image has been added to the file
///                                     execution table.
/// @retval Status::ACCESS_DENIED       The file specified by File and FileBuffer did not
///                                     authenticate, and the platform policy dictates that the DXE
///                                     Foundation may not use File.
/// @retval Status::SECURITY_VIOLATION  FileBuffer is NULL and the user has no
///                                     permission to start UEFI device drivers on the device path specified
///                                     by DevicePath.
/// @retval Status::SECURITY_VIOLATION  FileBuffer is not NULL and the user has no permission to load
///                                     drivers from the device path specified by DevicePath. The
///                                     image has been added into the list of the deferred images.
pub type EfiSecurity2FileAuthentication = extern "efiapi" fn(
    this: *mut Protocol,
    file: *mut efi::protocols::device_path::Protocol,
    file_buffer: *mut c_void,
    file_size: usize,
    boot_policy: bool,
) -> efi::Status;

/// The EFI_SECURITY2_ARCH_PROTOCOL is used to abstract platform-specific policy from the
/// DXE Foundation. This includes measuring the PE/COFF image prior to invoking, comparing the
/// image against a policy (whether a white-list/black-list of public image verification keys
/// or registered hashes).
#[repr(C)]
pub struct Protocol {
    /// Function to check file authentication state. See [`EfiSecurity2FileAuthentication`] for more details.
    pub file_authentication: EfiSecurity2FileAuthentication,
}
