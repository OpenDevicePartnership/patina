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

use crate::pi::{fw_fs::EfiFvbAttributes2, hob::EfiPhysicalAddress};

/// GUID for the Firmware Volume Block (FVB) Protocol.
///
/// This protocol provides control over block-oriented firmware devices.
/// It abstracts the block-oriented nature of firmware volumes to allow consumers
/// to read, write, and erase firmware volume blocks uniformly.
pub const PROTOCOL_GUID: Guid =
    Guid::from_fields(0x8f644fa9, 0xe850, 0x4db1, 0x9c, 0xe2, &[0xb, 0x44, 0x69, 0x8e, 0x8d, 0xa4]);

/// Retrieves the current attributes and capabilities of a firmware volume.
///
/// On input, Attributes is a pointer to a caller-allocated EFI_FVB_ATTRIBUTES_2 in
/// which the current attributes and capabilities are returned.
pub type GetAttributes = extern "efiapi" fn(*mut Protocol, *mut EfiFvbAttributes2) -> Status;

/// Sets firmware volume attributes and returns new attributes.
///
/// Modifies the current attributes of the firmware volume according to the input parameter,
/// then returns the new attributes value in the same parameter.
pub type SetAttributes = extern "efiapi" fn(*mut Protocol, *mut EfiFvbAttributes2) -> Status;

/// Retrieves the physical address of the device.
///
/// Retrieves the physical address of a memory mapped FV. This function should
/// only be called for memory mapped FVs.
pub type GetPhysicalAddress = extern "efiapi" fn(*mut Protocol, *mut EfiPhysicalAddress) -> Status;

/// Gets the size of a specific block within a firmware volume.
///
/// Retrieves the size, in bytes, of a specific block within a firmware volume and
/// the number of similar-sized blocks in the firmware volume.
pub type GetBlockSize = extern "efiapi" fn(*mut Protocol, Lba, *mut usize, *mut usize) -> Status;

/// Reads data beginning at the specified offset.
///
/// Reads the specified number of bytes into the provided buffer from the specified block offset.
/// The read operation may be performed on all the blocks in the firmware volume.
pub type Read = extern "efiapi" fn(*mut Protocol, Lba, usize, *mut usize, *mut c_void) -> Status;

/// Writes data beginning at the specified offset.
///
/// Writes the specified number of bytes from the input buffer to the block. The write
/// operation may be performed on all the blocks in the firmware volume.
pub type Write = extern "efiapi" fn(*mut Protocol, Lba, usize, *mut usize, *mut c_void) -> Status;

/// Erases and initializes specified firmware volume blocks.
///
/// The variable argument list is a list of tuples that specify logical block addresses and
/// the number of blocks to erase. The list is terminated with EFI_LBA_LIST_TERMINATOR.
pub type EraseBlocks = extern "efiapi" fn(
    *mut Protocol,
    //... //TODO: variadic functions and eficall! do not mix presently.
) -> Status;

/// Provides low-level access to a firmware volume.
///
/// The Firmware Volume Block Protocol is the low-level interface used to access
/// a firmware volume. File-level access to a firmware volume should not be done
/// using the Firmware Volume Block Protocol. The Firmware Volume Protocol should
/// be used for normal file-level access to a firmware volume.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section III-3.4.2.1
#[repr(C)]
pub struct Protocol {
    /// Returns the attributes and current settings of the firmware volume.
    pub get_attributes: GetAttributes,
    /// Modifies the current settings of the firmware volume according to the input parameter.
    pub set_attributes: SetAttributes,
    /// Retrieves the physical address of a memory-mapped firmware volume.
    pub get_physical_address: GetPhysicalAddress,
    ///Retrieves the size in bytes of a specific block within a firmware volume.
    pub get_block_size: GetBlockSize,
    /// Reads the specified number of bytes into a buffer from the specified block.
    pub read: Read,
    /// Writes the specified number of bytes from the input buffer to the block.
    pub write: Write,
    /// Erases and initializes a firmware volume block.
    pub erase_blocks: EraseBlocks,
    /// Handle of the parent firmware volume.
    pub parent_handle: Handle,
}
