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

pub const PROTOCOL_GUID: Guid =
    Guid::from_fields(0x220e73b6, 0x6bdb, 0x4413, 0x84, 0x5, &[0xb9, 0x74, 0xb1, 0x8, 0x61, 0x9a]);

pub type EfiFvWritePolicy = u32;

#[repr(C)]
pub struct EfiFvWriteFileData {
    name_guid: *mut Guid,
    file_type: EfiFvFileType,
    file_attributes: EfiFvFileAttributes,
    buffer: *mut c_void,
    buffer_size: u32,
}

pub type GetVolumeAttributes = extern "efiapi" fn(*const Protocol, *mut EfiFvAttributes) -> Status;

pub type SetVolumeAttributes = extern "efiapi" fn(*const Protocol, *mut EfiFvAttributes) -> Status;

pub type ReadFile = extern "efiapi" fn(
    *const Protocol,
    *const Guid,
    *mut *mut c_void,
    *mut usize,
    *mut EfiFvFileType,
    *mut EfiFvFileAttributes,
    *mut u32,
) -> Status;

pub type ReadSection = extern "efiapi" fn(
    *const Protocol,
    *const Guid,
    EfiSectionType,
    usize,
    *mut *mut c_void,
    *mut usize,
    *mut u32,
) -> Status;

pub type WriteFile = extern "efiapi" fn(*const Protocol, u32, EfiFvWritePolicy, *mut EfiFvWriteFileData) -> Status;

pub type GetNextFile = extern "efiapi" fn(
    *const Protocol,
    *mut c_void,
    *mut EfiFvFileType,
    *mut Guid,
    *mut EfiFvFileAttributes,
    *mut usize,
) -> Status;

pub type GetInfo = extern "efiapi" fn(*const Protocol, *const Guid, *mut usize, *mut c_void) -> Status;

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
    pub get_volume_attributes: GetVolumeAttributes,
    pub set_volume_attributes: SetVolumeAttributes,
    pub read_file: ReadFile,
    pub read_section: ReadSection,
    pub write_file: WriteFile,
    pub get_next_file: GetNextFile,
    pub key_size: u32,
    pub parent_handle: Handle,
    pub get_info: GetInfo,
    pub set_info: SetInfo,
}
