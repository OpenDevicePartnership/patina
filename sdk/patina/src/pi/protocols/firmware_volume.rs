//! Firmware Volume (FV) Protocol
//!
//! The Firmware Volume Protocol provides file-level access to the firmware volume. Each firmware volume driver must
//! produce an instance of the Firmware Volume Protocol if the firmware volume is to be visible to the system during
//! the DXE phase. The Firmware Volume Protocol also provides mechanisms for determining and modifying some attributes
//! of the firmware volume.
//!
//! See <https://uefi.org/specs/PI/1.8A/V3_Code_Definitions.html#efi-firmware-volume2-protocol>.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::pi::fw_fs;

use fw_fs::{
    ffs::section::EfiSectionType,
    fv::{EfiFvFileType, attributes::EfiFvAttributes, file::EfiFvFileAttributes},
};

use core::ffi::c_void;
use r_efi::efi::{Guid, Handle, Status};

/// GUID for the Firmware Volume (FV) Protocol.
///
/// This protocol provides file-level access to firmware volumes. It abstracts
/// the complexity of the firmware volume format to provide simple file-based
/// read and write operations.
pub const PROTOCOL_GUID: Guid =
    Guid::from_fields(0x220e73b6, 0x6bdb, 0x4413, 0x84, 0x5, &[0xb9, 0x74, 0xb1, 0x8, 0x61, 0x9a]);

/// Enumeration of write policies for firmware volume operations.
pub type EfiFvWritePolicy = u32;

#[repr(C)]
/// Data structure for writing files to firmware volumes.
///
/// Contains the metadata and content information needed to write a file
/// to a firmware volume, including GUID, type, attributes, and data buffer.
pub struct EfiFvWriteFileData {
    name_guid: *mut Guid,
    file_type: EfiFvFileType,
    file_attributes: EfiFvFileAttributes,
    buffer: *mut c_void,
    buffer_size: u32,
}

/// Retrieves the current attributes and current settings of the firmware volume.
///
/// Gets the current attributes and status of the firmware volume. These attributes
/// control volume behavior and indicate current operational capabilities.
pub type GetVolumeAttributes = extern "efiapi" fn(*const Protocol, *mut EfiFvAttributes) -> Status;

/// Modifies the current settings of the firmware volume.
///
/// Sets the firmware volume attributes according to the input parameter, then
/// returns the new settings. Some attributes may not be modifiable after creation.
pub type SetVolumeAttributes = extern "efiapi" fn(*const Protocol, *mut EfiFvAttributes) -> Status;

/// Reads an entire file from the firmware volume.
///
/// Locates a file within a firmware volume and reads the entire file into a buffer.
/// The caller specifies the file to read by its GUID name.
pub type ReadFile = extern "efiapi" fn(
    *const Protocol,
    *const Guid,
    *mut *mut c_void,
    *mut usize,
    *mut EfiFvFileType,
    *mut EfiFvFileAttributes,
    *mut u32,
) -> Status;

/// Reads a specific section from a file within the firmware volume.
///
/// Locates a file by GUID and extracts a specific section by type and instance.
/// This allows selective reading of particular components within a file.
pub type ReadSection = extern "efiapi" fn(
    *const Protocol,
    *const Guid,
    EfiSectionType,
    usize,
    *mut *mut c_void,
    *mut usize,
    *mut u32,
) -> Status;

/// Writes a file to the firmware volume.
///
/// Writes data to the firmware volume according to the specified write policy.
/// The write policy determines how the operation handles existing files.
pub type WriteFile = extern "efiapi" fn(*const Protocol, u32, EfiFvWritePolicy, *mut EfiFvWriteFileData) -> Status;

/// Enumerates files in the firmware volume.
///
/// Retrieves the next file in the firmware volume. Repeated calls enumerate all
/// files within the volume, providing file metadata for each entry.
pub type GetNextFile = extern "efiapi" fn(
    *const Protocol,
    *mut c_void,
    *mut EfiFvFileType,
    *mut Guid,
    *mut EfiFvFileAttributes,
    *mut usize,
) -> Status;

/// Retrieves volume-specific information.
///
/// Returns information about the firmware volume. The information type is specified
/// by the InformationType GUID parameter.
pub type GetInfo = extern "efiapi" fn(*const Protocol, *const Guid, *mut usize, *mut c_void) -> Status;

/// Modifies volume-specific information.
///
/// Sets information about the firmware volume. The information type is specified
/// by the InformationType GUID parameter.
pub type SetInfo = extern "efiapi" fn(*const Protocol, *const Guid, usize, *const c_void) -> Status;

/// The Firmware Volume Protocol provides file-level access to the firmware volume. Each firmware volume driver must
/// produce an instance of the Firmware Volume Protocol if the firmware volume is to be visible to the system during
/// the DXE phase. The Firmware Volume Protocol also provides mechanisms for determining and modifying some attributes
/// of the firmware volume.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section III-3.4.1.1
#[repr(C)]
pub struct Protocol {
    /// Gets the current attributes of the firmware volume.
    pub get_volume_attributes: GetVolumeAttributes,
    /// Sets the attributes of the firmware volume.
    pub set_volume_attributes: SetVolumeAttributes,
    /// Reads a file from the firmware volume.
    pub read_file: ReadFile,
    /// Reads a section from a file in the firmware volume.
    pub read_section: ReadSection,
    /// Writes a file to the firmware volume.
    pub write_file: WriteFile,
    /// Finds the next file in the firmware volume.
    pub get_next_file: GetNextFile,
    /// Size of the search key for get_next_file.
    pub key_size: u32,
    /// Handle of the parent firmware volume.
    pub parent_handle: Handle,
    /// Gets information about the firmware volume.
    pub get_info: GetInfo,
    /// Sets information about the firmware volume.
    pub set_info: SetInfo,
}
