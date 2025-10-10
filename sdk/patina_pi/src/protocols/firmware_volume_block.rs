//! Firmware Volume Block (FVB) Protocol
//!
//! The Firmware Volume Block Protocol is the low-level interface to a firmware volume. File-level access to a firmware
//! volume should not be done using thE Firmware Volume Block Protocol. Normal access to a firmware volume must use
//! the Firmware Volume Protocol.
//!
//! See <https://uefi.org/specs/PI/1.8A/V3_Code_Definitions.html#efi-firmware-volume-block2-protocol>.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;
use r_efi::efi::{Guid, Handle, Lba, Status};

use crate::{fw_fs::EfiFvbAttributes2, hob::EfiPhysicalAddress};

pub const PROTOCOL_GUID: Guid =
    Guid::from_fields(0x8f644fa9, 0xe850, 0x4db1, 0x9c, 0xe2, &[0xb, 0x44, 0x69, 0x8e, 0x8d, 0xa4]);

pub type GetAttributes = extern "efiapi" fn(*mut Protocol, *mut EfiFvbAttributes2) -> Status;

pub type SetAttributes = extern "efiapi" fn(*mut Protocol, *mut EfiFvbAttributes2) -> Status;

pub type GetPhysicalAddress = extern "efiapi" fn(*mut Protocol, *mut EfiPhysicalAddress) -> Status;

pub type GetBlockSize = extern "efiapi" fn(*mut Protocol, Lba, *mut usize, *mut usize) -> Status;

pub type Read = extern "efiapi" fn(*mut Protocol, Lba, usize, *mut usize, *mut c_void) -> Status;

pub type Write = extern "efiapi" fn(*mut Protocol, Lba, usize, *mut usize, *mut c_void) -> Status;

pub type EraseBlocks = extern "efiapi" fn(
    *mut Protocol,
    //... //TODO: variadic functions and eficall! do not mix presently.
) -> Status;

/// The Firmware Volume Block Protocol is the low-level interface to a firmware volume. File-level access to a firmware
/// volume should not be done using thE Firmware Volume Block Protocol. Normal access to a firmware volume must use
/// the Firmware Volume Protocol.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section III-3.4.2.1
#[repr(C)]
pub struct Protocol {
    pub get_attributes: GetAttributes,
    pub set_attributes: SetAttributes,
    pub get_physical_address: GetPhysicalAddress,
    pub get_block_size: GetBlockSize,
    pub read: Read,
    pub write: Write,
    pub erase_blocks: EraseBlocks,
    pub parent_handle: Handle,
}
