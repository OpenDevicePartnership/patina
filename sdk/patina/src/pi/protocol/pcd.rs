//! Platform Configuration Database (PCD) Protocol
//!
//! Provides get/set access to Dynamic PCDs (addressed by a token number alone, in the default
//! token space) and `DynamicEx` PCDs (addressed by a token space GUID together with a token
//! number). This protocol is produced by a platform's PCD DXE driver (not Patina).
//!
//! See <https://github.com/tianocore/edk2/blob/master/MdeModulePkg/Include/Protocol/Pcd.h>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;

use crate::base::protocol::ProtocolInterface;
use crate::standard::efi;

/// PCD Protocol GUID.
pub const PROTOCOL_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_fields(0x11b34006, 0xd85b, 0x4d0a, 0xa2, 0x90, &[0xd5, 0xa5, 0x71, 0x31, 0x0e, 0xf7]);

/// Sets the SKU used by subsequent calls to get or set a SKU-enabled PCD.
pub type PcdSetSku = extern "efiapi" fn(sku_id: usize);

/// Retrieves an 8-bit Dynamic PCD value.
pub type PcdGet8 = extern "efiapi" fn(token_number: usize) -> u8;

/// Retrieves a 16-bit Dynamic PCD value.
pub type PcdGet16 = extern "efiapi" fn(token_number: usize) -> u16;

/// Retrieves a 32-bit Dynamic PCD value.
pub type PcdGet32 = extern "efiapi" fn(token_number: usize) -> u32;

/// Retrieves a 64-bit Dynamic PCD value.
pub type PcdGet64 = extern "efiapi" fn(token_number: usize) -> u64;

/// Retrieves a pointer to a Dynamic PCD value. The pointer is not guaranteed to be aligned.
pub type PcdGetPtr = extern "efiapi" fn(token_number: usize) -> *mut c_void;

/// Retrieves a boolean Dynamic PCD value.
pub type PcdGetBool = extern "efiapi" fn(token_number: usize) -> efi::Boolean;

/// Retrieves the size, in bytes, of a Dynamic PCD value.
pub type PcdGetSize = extern "efiapi" fn(token_number: usize) -> usize;

/// Retrieves an 8-bit `DynamicEx` PCD value.
pub type PcdGet8Ex = extern "efiapi" fn(guid: *const efi::Guid, token_number: usize) -> u8;

/// Retrieves a 16-bit `DynamicEx` PCD value.
pub type PcdGet16Ex = extern "efiapi" fn(guid: *const efi::Guid, token_number: usize) -> u16;

/// Retrieves a 32-bit `DynamicEx` PCD value.
pub type PcdGet32Ex = extern "efiapi" fn(guid: *const efi::Guid, token_number: usize) -> u32;

/// Retrieves a 64-bit `DynamicEx` PCD value.
pub type PcdGet64Ex = extern "efiapi" fn(guid: *const efi::Guid, token_number: usize) -> u64;

/// Retrieves a pointer to a `DynamicEx` PCD value. The pointer is not guaranteed to be aligned.
pub type PcdGetPtrEx = extern "efiapi" fn(guid: *const efi::Guid, token_number: usize) -> *mut c_void;

/// Retrieves a boolean `DynamicEx` PCD value.
pub type PcdGetBoolEx = extern "efiapi" fn(guid: *const efi::Guid, token_number: usize) -> efi::Boolean;

/// Retrieves the size, in bytes, of a `DynamicEx` PCD value.
pub type PcdGetSizeEx = extern "efiapi" fn(guid: *const efi::Guid, token_number: usize) -> usize;

/// Sets an 8-bit Dynamic PCD value.
pub type PcdSet8 = extern "efiapi" fn(token_number: usize, value: u8) -> efi::Status;

/// Sets a 16-bit Dynamic PCD value.
pub type PcdSet16 = extern "efiapi" fn(token_number: usize, value: u16) -> efi::Status;

/// Sets a 32-bit Dynamic PCD value.
pub type PcdSet32 = extern "efiapi" fn(token_number: usize, value: u32) -> efi::Status;

/// Sets a 64-bit Dynamic PCD value.
pub type PcdSet64 = extern "efiapi" fn(token_number: usize, value: u64) -> efi::Status;

/// Sets a Dynamic PCD value from a buffer. `size_of_buffer` is updated to the size actually used.
pub type PcdSetPtr =
    extern "efiapi" fn(token_number: usize, size_of_buffer: *mut usize, buffer: *const c_void) -> efi::Status;

/// Sets a boolean Dynamic PCD value.
pub type PcdSetBool = extern "efiapi" fn(token_number: usize, value: efi::Boolean) -> efi::Status;

/// Sets an 8-bit `DynamicEx` PCD value.
pub type PcdSet8Ex = extern "efiapi" fn(guid: *const efi::Guid, token_number: usize, value: u8) -> efi::Status;

/// Sets a 16-bit `DynamicEx` PCD value.
pub type PcdSet16Ex = extern "efiapi" fn(guid: *const efi::Guid, token_number: usize, value: u16) -> efi::Status;

/// Sets a 32-bit `DynamicEx` PCD value.
pub type PcdSet32Ex = extern "efiapi" fn(guid: *const efi::Guid, token_number: usize, value: u32) -> efi::Status;

/// Sets a 64-bit `DynamicEx` PCD value.
pub type PcdSet64Ex = extern "efiapi" fn(guid: *const efi::Guid, token_number: usize, value: u64) -> efi::Status;

/// Sets a `DynamicEx` PCD value from a buffer. `size_of_buffer` is updated to the size actually used.
pub type PcdSetPtrEx = extern "efiapi" fn(
    guid: *const efi::Guid,
    token_number: usize,
    size_of_buffer: *mut usize,
    buffer: *const c_void,
) -> efi::Status;

/// Sets a boolean `DynamicEx` PCD value.
pub type PcdSetBoolEx =
    extern "efiapi" fn(guid: *const efi::Guid, token_number: usize, value: efi::Boolean) -> efi::Status;

/// Callback invoked when the value of a watched PCD token is set.
pub type PcdCallback =
    extern "efiapi" fn(guid: *const efi::Guid, callback_token: usize, token_data: *mut c_void, token_data_size: usize);

/// Registers a callback for when the value of a PCD token is set.
pub type PcdCallbackOnSet =
    extern "efiapi" fn(guid: *const efi::Guid, token_number: usize, callback_function: PcdCallback) -> efi::Status;

/// Cancels a callback previously registered with [`PcdCallbackOnSet`].
pub type PcdCancelCallback =
    extern "efiapi" fn(guid: *const efi::Guid, token_number: usize, callback_function: PcdCallback) -> efi::Status;

/// Retrieves the next valid token number in a token space.
pub type PcdGetNextToken = extern "efiapi" fn(guid: *const efi::Guid, token_number: *mut usize) -> efi::Status;

/// Retrieves the next valid token space GUID.
pub type PcdGetNextTokenSpace = extern "efiapi" fn(guid: *mut *const efi::Guid) -> efi::Status;

/// The PCD Protocol (`PCD_PROTOCOL`), supporting both Dynamic and `DynamicEx` PCDs.
///
/// Field order and types exactly match the C `PCD_PROTOCOL` struct. This layout must not be
/// reordered independently of the upstream header.
#[repr(C)]
pub struct PcdProtocol {
    /// See [`PcdSetSku`].
    pub set_sku: PcdSetSku,
    /// See [`PcdGet8`].
    pub get8: PcdGet8,
    /// See [`PcdGet16`].
    pub get16: PcdGet16,
    /// See [`PcdGet32`].
    pub get32: PcdGet32,
    /// See [`PcdGet64`].
    pub get64: PcdGet64,
    /// See [`PcdGetPtr`].
    pub get_ptr: PcdGetPtr,
    /// See [`PcdGetBool`].
    pub get_bool: PcdGetBool,
    /// See [`PcdGetSize`].
    pub get_size: PcdGetSize,
    /// See [`PcdGet8Ex`].
    pub get8_ex: PcdGet8Ex,
    /// See [`PcdGet16Ex`].
    pub get16_ex: PcdGet16Ex,
    /// See [`PcdGet32Ex`].
    pub get32_ex: PcdGet32Ex,
    /// See [`PcdGet64Ex`].
    pub get64_ex: PcdGet64Ex,
    /// See [`PcdGetPtrEx`].
    pub get_ptr_ex: PcdGetPtrEx,
    /// See [`PcdGetBoolEx`].
    pub get_bool_ex: PcdGetBoolEx,
    /// See [`PcdGetSizeEx`].
    pub get_size_ex: PcdGetSizeEx,
    /// See [`PcdSet8`].
    pub set8: PcdSet8,
    /// See [`PcdSet16`].
    pub set16: PcdSet16,
    /// See [`PcdSet32`].
    pub set32: PcdSet32,
    /// See [`PcdSet64`].
    pub set64: PcdSet64,
    /// See [`PcdSetPtr`].
    pub set_ptr: PcdSetPtr,
    /// See [`PcdSetBool`].
    pub set_bool: PcdSetBool,
    /// See [`PcdSet8Ex`].
    pub set8_ex: PcdSet8Ex,
    /// See [`PcdSet16Ex`].
    pub set16_ex: PcdSet16Ex,
    /// See [`PcdSet32Ex`].
    pub set32_ex: PcdSet32Ex,
    /// See [`PcdSet64Ex`].
    pub set64_ex: PcdSet64Ex,
    /// See [`PcdSetPtrEx`].
    pub set_ptr_ex: PcdSetPtrEx,
    /// See [`PcdSetBoolEx`].
    pub set_bool_ex: PcdSetBoolEx,
    /// See [`PcdCallbackOnSet`].
    pub callback_on_set: PcdCallbackOnSet,
    /// See [`PcdCancelCallback`].
    pub cancel_callback: PcdCancelCallback,
    /// See [`PcdGetNextToken`].
    pub get_next_token: PcdGetNextToken,
    /// See [`PcdGetNextTokenSpace`].
    pub get_next_token_space: PcdGetNextTokenSpace,
}

// SAFETY: `PcdProtocol` is `#[repr(C)]` with a field layout that reflects the C `PCD_PROTOCOL`
// struct exactly, and `PROTOCOL_GUID` is `gPcdProtocolGuid`, which identifies that layout.
unsafe impl ProtocolInterface for PcdProtocol {
    const PROTOCOL_GUID: crate::BinaryGuid = PROTOCOL_GUID;
}
