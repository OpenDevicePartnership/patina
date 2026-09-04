//! PCD Provider Component
//!
//! Locates `PCD_PROTOCOL` and registers the `PcdServices` service so other components can get and
//! set Dynamic and `DynamicEx` PCDs.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{ffi::c_void, mem::size_of};

use alloc::vec::Vec;

use patina::{
    component::{
        component,
        params::Commands,
        protocol::Protocol,
        service::{
            IntoService,
            pcd::{PcdError, PcdServices, PcdToken},
        },
    },
    error::{EfiError, Result as PatinaResult},
    pi::protocol::pcd::PcdProtocol,
    standard::efi,
};

/// Locates `PCD_PROTOCOL` and produces the `PcdServices` service.
#[derive(Debug, Default, IntoService)]
#[service(dyn PcdServices)]
pub struct PcdProvider {
    protocol: Option<Protocol<PcdProtocol>>,
}

impl PcdProvider {
    /// Creates a new, not-yet-initialized `PcdProvider`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the located PCD protocol.
    ///
    /// # Panics
    ///
    /// Panics if called before `entry_point` runs. This cannot happen through normal dispatch,
    /// since `PcdProvider` is only registered as a service after `entry_point` stores the
    /// protocol.
    fn protocol(&self) -> Protocol<PcdProtocol> {
        self.protocol.expect("PcdProvider must be dispatched before its service methods are called")
    }
}

#[component]
impl PcdProvider {
    fn entry_point(mut self, protocol: Protocol<PcdProtocol>, mut commands: Commands) -> PatinaResult<()> {
        self.protocol = Some(protocol);
        commands.add_service(self);
        Ok(())
    }
}

fn raw_get_size(pcd: &PcdProtocol, token: PcdToken) -> usize {
    match token {
        PcdToken::Dynamic(n) => (pcd.get_size)(n),
        PcdToken::DynamicEx(guid, n) => (pcd.get_size_ex)(guid.as_efi_guid(), n),
    }
}

fn raw_get8(pcd: &PcdProtocol, token: PcdToken) -> u8 {
    match token {
        PcdToken::Dynamic(n) => (pcd.get8)(n),
        PcdToken::DynamicEx(guid, n) => (pcd.get8_ex)(guid.as_efi_guid(), n),
    }
}

fn raw_get16(pcd: &PcdProtocol, token: PcdToken) -> u16 {
    match token {
        PcdToken::Dynamic(n) => (pcd.get16)(n),
        PcdToken::DynamicEx(guid, n) => (pcd.get16_ex)(guid.as_efi_guid(), n),
    }
}

fn raw_get32(pcd: &PcdProtocol, token: PcdToken) -> u32 {
    match token {
        PcdToken::Dynamic(n) => (pcd.get32)(n),
        PcdToken::DynamicEx(guid, n) => (pcd.get32_ex)(guid.as_efi_guid(), n),
    }
}

fn raw_get64(pcd: &PcdProtocol, token: PcdToken) -> u64 {
    match token {
        PcdToken::Dynamic(n) => (pcd.get64)(n),
        PcdToken::DynamicEx(guid, n) => (pcd.get64_ex)(guid.as_efi_guid(), n),
    }
}

fn raw_get_bool(pcd: &PcdProtocol, token: PcdToken) -> efi::Boolean {
    match token {
        PcdToken::Dynamic(n) => (pcd.get_bool)(n),
        PcdToken::DynamicEx(guid, n) => (pcd.get_bool_ex)(guid.as_efi_guid(), n),
    }
}

fn raw_get_ptr(pcd: &PcdProtocol, token: PcdToken) -> *mut c_void {
    match token {
        PcdToken::Dynamic(n) => (pcd.get_ptr)(n),
        PcdToken::DynamicEx(guid, n) => (pcd.get_ptr_ex)(guid.as_efi_guid(), n),
    }
}

fn raw_set8(pcd: &PcdProtocol, token: PcdToken, value: u8) -> efi::Status {
    match token {
        PcdToken::Dynamic(n) => (pcd.set8)(n, value),
        PcdToken::DynamicEx(guid, n) => (pcd.set8_ex)(guid.as_efi_guid(), n, value),
    }
}

fn raw_set16(pcd: &PcdProtocol, token: PcdToken, value: u16) -> efi::Status {
    match token {
        PcdToken::Dynamic(n) => (pcd.set16)(n, value),
        PcdToken::DynamicEx(guid, n) => (pcd.set16_ex)(guid.as_efi_guid(), n, value),
    }
}

fn raw_set32(pcd: &PcdProtocol, token: PcdToken, value: u32) -> efi::Status {
    match token {
        PcdToken::Dynamic(n) => (pcd.set32)(n, value),
        PcdToken::DynamicEx(guid, n) => (pcd.set32_ex)(guid.as_efi_guid(), n, value),
    }
}

fn raw_set64(pcd: &PcdProtocol, token: PcdToken, value: u64) -> efi::Status {
    match token {
        PcdToken::Dynamic(n) => (pcd.set64)(n, value),
        PcdToken::DynamicEx(guid, n) => (pcd.set64_ex)(guid.as_efi_guid(), n, value),
    }
}

fn raw_set_bool(pcd: &PcdProtocol, token: PcdToken, value: efi::Boolean) -> efi::Status {
    match token {
        PcdToken::Dynamic(n) => (pcd.set_bool)(n, value),
        PcdToken::DynamicEx(guid, n) => (pcd.set_bool_ex)(guid.as_efi_guid(), n, value),
    }
}

fn raw_set_ptr(pcd: &PcdProtocol, token: PcdToken, size_of_buffer: &mut usize, buffer: *const c_void) -> efi::Status {
    match token {
        PcdToken::Dynamic(n) => (pcd.set_ptr)(n, size_of_buffer, buffer),
        PcdToken::DynamicEx(guid, n) => (pcd.set_ptr_ex)(guid.as_efi_guid(), n, size_of_buffer, buffer),
    }
}

impl PcdServices for PcdProvider {
    unsafe fn get_size(&self, token: PcdToken) -> usize {
        let protocol = self.protocol();
        raw_get_size(&protocol, token)
    }

    unsafe fn get_u8(&self, token: PcdToken) -> Result<u8, PcdError> {
        let protocol = self.protocol();
        let pcd = &*protocol;
        let actual = raw_get_size(pcd, token);
        if actual != size_of::<u8>() {
            return Err(PcdError::SizeMismatch { actual, expected: size_of::<u8>() });
        }
        Ok(raw_get8(pcd, token))
    }

    unsafe fn get_u16(&self, token: PcdToken) -> Result<u16, PcdError> {
        let protocol = self.protocol();
        let pcd = &*protocol;
        let actual = raw_get_size(pcd, token);
        if actual != size_of::<u16>() {
            return Err(PcdError::SizeMismatch { actual, expected: size_of::<u16>() });
        }
        Ok(raw_get16(pcd, token))
    }

    unsafe fn get_u32(&self, token: PcdToken) -> Result<u32, PcdError> {
        let protocol = self.protocol();
        let pcd = &*protocol;
        let actual = raw_get_size(pcd, token);
        if actual != size_of::<u32>() {
            return Err(PcdError::SizeMismatch { actual, expected: size_of::<u32>() });
        }
        Ok(raw_get32(pcd, token))
    }

    unsafe fn get_u64(&self, token: PcdToken) -> Result<u64, PcdError> {
        let protocol = self.protocol();
        let pcd = &*protocol;
        let actual = raw_get_size(pcd, token);
        if actual != size_of::<u64>() {
            return Err(PcdError::SizeMismatch { actual, expected: size_of::<u64>() });
        }
        Ok(raw_get64(pcd, token))
    }

    unsafe fn get_bool(&self, token: PcdToken) -> Result<bool, PcdError> {
        let protocol = self.protocol();
        let pcd = &*protocol;
        let actual = raw_get_size(pcd, token);
        if actual != size_of::<bool>() {
            return Err(PcdError::SizeMismatch { actual, expected: size_of::<bool>() });
        }
        Ok(bool::from(raw_get_bool(pcd, token)))
    }

    unsafe fn get_bytes(&self, token: PcdToken) -> Result<Vec<u8>, PcdError> {
        let protocol = self.protocol();
        let pcd = &*protocol;
        let size = raw_get_size(pcd, token);
        if size == 0 {
            return Ok(Vec::new());
        }
        let ptr = raw_get_ptr(pcd, token);
        if ptr.is_null() {
            return Err(PcdError::NotFound);
        }
        // SAFETY: `ptr` and `size` were just reported for the same token by the PCD protocol's
        // own GetPtr(Ex)/GetSize(Ex) functions.
        let bytes = unsafe { core::slice::from_raw_parts(ptr.cast::<u8>(), size) };
        Ok(bytes.to_vec())
    }

    unsafe fn set_u8(&self, token: PcdToken, value: u8) -> Result<(), PcdError> {
        let protocol = self.protocol();
        EfiError::status_to_result(raw_set8(&protocol, token, value)).map_err(PcdError::from)
    }

    unsafe fn set_u16(&self, token: PcdToken, value: u16) -> Result<(), PcdError> {
        let protocol = self.protocol();
        EfiError::status_to_result(raw_set16(&protocol, token, value)).map_err(PcdError::from)
    }

    unsafe fn set_u32(&self, token: PcdToken, value: u32) -> Result<(), PcdError> {
        let protocol = self.protocol();
        EfiError::status_to_result(raw_set32(&protocol, token, value)).map_err(PcdError::from)
    }

    unsafe fn set_u64(&self, token: PcdToken, value: u64) -> Result<(), PcdError> {
        let protocol = self.protocol();
        EfiError::status_to_result(raw_set64(&protocol, token, value)).map_err(PcdError::from)
    }

    unsafe fn set_bool(&self, token: PcdToken, value: bool) -> Result<(), PcdError> {
        let protocol = self.protocol();
        EfiError::status_to_result(raw_set_bool(&protocol, token, value.into())).map_err(PcdError::from)
    }

    unsafe fn set_bytes(&self, token: PcdToken, value: &[u8]) -> Result<(), PcdError> {
        let protocol = self.protocol();
        let mut size_of_buffer = value.len();
        let status = raw_set_ptr(&protocol, token, &mut size_of_buffer, value.as_ptr().cast::<c_void>());
        if status == efi::Status::INVALID_PARAMETER && size_of_buffer < value.len() {
            return Err(PcdError::BufferTooLarge { max_size: size_of_buffer });
        }
        EfiError::status_to_result(status).map_err(PcdError::from)
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina::BinaryGuid;
    use patina::pi::protocol::pcd::PcdCallback;

    const TOKEN_U8: usize = 1;
    const TOKEN_U32: usize = 2;
    const TOKEN_BOOL: usize = 3;
    const TOKEN_BYTES: usize = 4;
    const TOKEN_WRONG_SIZE: usize = 5;
    const TOKEN_ZERO_SIZE: usize = 6;
    const TOKEN_NULL_PTR: usize = 7;
    const TOKEN_SET_OK: usize = 900;
    const TOKEN_SET_NOT_FOUND: usize = 901;
    const TOKEN_SET_BYTES_MAX4: usize = 902;

    const U32_VALUE: u32 = 0xDEAD_BEEF;
    static BYTES_VALUE: [u8; 5] = [1, 2, 3, 4, 5];

    const EX_GUID: BinaryGuid = BinaryGuid::from_string("6ba2ab5e-3e37-4a89-93ab-2be4b12657a1");
    const TOKEN_EX_U16: usize = 1;
    const U16_EX_VALUE: u16 = 0x1234;

    extern "efiapi" fn fake_set_sku(_sku_id: usize) {}

    extern "efiapi" fn fake_get8(token_number: usize) -> u8 {
        if token_number == TOKEN_U8 { 42 } else { 0 }
    }

    extern "efiapi" fn fake_get16(_token_number: usize) -> u16 {
        0
    }

    extern "efiapi" fn fake_get32(token_number: usize) -> u32 {
        if token_number == TOKEN_U32 { U32_VALUE } else { 0 }
    }

    extern "efiapi" fn fake_get64(_token_number: usize) -> u64 {
        0
    }

    extern "efiapi" fn fake_get_ptr(token_number: usize) -> *mut c_void {
        if token_number == TOKEN_BYTES {
            core::ptr::addr_of!(BYTES_VALUE).cast::<c_void>().cast_mut()
        } else {
            core::ptr::null_mut()
        }
    }

    extern "efiapi" fn fake_get_bool(token_number: usize) -> efi::Boolean {
        (token_number == TOKEN_BOOL).into()
    }

    extern "efiapi" fn fake_get_size(token_number: usize) -> usize {
        match token_number {
            TOKEN_U8 => size_of::<u8>(),
            // Deliberately not 1 byte, so get_u8 on this token exercises SizeMismatch.
            TOKEN_U32 | TOKEN_WRONG_SIZE => size_of::<u32>(),
            TOKEN_BOOL => size_of::<bool>(),
            TOKEN_BYTES => BYTES_VALUE.len(),
            // Non-zero size but GetPtr returns null, exercising get_bytes' defensive null check.
            TOKEN_NULL_PTR => 5,
            // Covers TOKEN_ZERO_SIZE and every other unlisted token.
            _ => 0,
        }
    }

    extern "efiapi" fn fake_get8_ex(_guid: *const efi::Guid, _token_number: usize) -> u8 {
        0
    }

    extern "efiapi" fn fake_get16_ex(guid: *const efi::Guid, token_number: usize) -> u16 {
        // SAFETY: test-only; `guid` always points to a valid, live efi::Guid for the call's duration.
        if unsafe { *guid } == EX_GUID.into_inner() && token_number == TOKEN_EX_U16 { U16_EX_VALUE } else { 0 }
    }

    extern "efiapi" fn fake_get32_ex(_guid: *const efi::Guid, _token_number: usize) -> u32 {
        0
    }

    extern "efiapi" fn fake_get64_ex(_guid: *const efi::Guid, _token_number: usize) -> u64 {
        0
    }

    extern "efiapi" fn fake_get_ptr_ex(_guid: *const efi::Guid, _token_number: usize) -> *mut c_void {
        core::ptr::null_mut()
    }

    extern "efiapi" fn fake_get_bool_ex(_guid: *const efi::Guid, _token_number: usize) -> efi::Boolean {
        false.into()
    }

    extern "efiapi" fn fake_get_size_ex(guid: *const efi::Guid, token_number: usize) -> usize {
        // SAFETY: test-only; `guid` always points to a valid, live efi::Guid for the call's duration.
        if unsafe { *guid } == EX_GUID.into_inner() && token_number == TOKEN_EX_U16 { size_of::<u16>() } else { 0 }
    }

    extern "efiapi" fn fake_set8(token_number: usize, _value: u8) -> efi::Status {
        match token_number {
            TOKEN_SET_OK => efi::Status::SUCCESS,
            TOKEN_SET_NOT_FOUND => efi::Status::NOT_FOUND,
            _ => efi::Status::INVALID_PARAMETER,
        }
    }

    extern "efiapi" fn fake_set16(_token_number: usize, _value: u16) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn fake_set32(_token_number: usize, _value: u32) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn fake_set64(_token_number: usize, _value: u64) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn fake_set_ptr(
        token_number: usize,
        size_of_buffer: *mut usize,
        _buffer: *const c_void,
    ) -> efi::Status {
        // SAFETY: test-only; `size_of_buffer` always points to a valid, live usize for the call's duration.
        let requested = unsafe { *size_of_buffer };
        if token_number == TOKEN_SET_BYTES_MAX4 && requested > 4 {
            // SAFETY: see above.
            unsafe { *size_of_buffer = 4 };
            efi::Status::INVALID_PARAMETER
        } else {
            efi::Status::SUCCESS
        }
    }

    extern "efiapi" fn fake_set_bool(_token_number: usize, _value: efi::Boolean) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn fake_set8_ex(_guid: *const efi::Guid, _token_number: usize, _value: u8) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn fake_set16_ex(_guid: *const efi::Guid, _token_number: usize, _value: u16) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn fake_set32_ex(_guid: *const efi::Guid, _token_number: usize, _value: u32) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn fake_set64_ex(_guid: *const efi::Guid, _token_number: usize, _value: u64) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn fake_set_ptr_ex(
        _guid: *const efi::Guid,
        _token_number: usize,
        _size_of_buffer: *mut usize,
        _buffer: *const c_void,
    ) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn fake_set_bool_ex(
        _guid: *const efi::Guid,
        _token_number: usize,
        _value: efi::Boolean,
    ) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn fake_callback(
        _guid: *const efi::Guid,
        _callback_token: usize,
        _token_data: *mut c_void,
        _token_data_size: usize,
    ) {
    }

    extern "efiapi" fn fake_callback_on_set(
        _guid: *const efi::Guid,
        _token_number: usize,
        _callback_function: PcdCallback,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    extern "efiapi" fn fake_cancel_callback(
        _guid: *const efi::Guid,
        _token_number: usize,
        _callback_function: PcdCallback,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    extern "efiapi" fn fake_get_next_token(_guid: *const efi::Guid, _token_number: *mut usize) -> efi::Status {
        efi::Status::NOT_FOUND
    }

    extern "efiapi" fn fake_get_next_token_space(_guid: *mut *const efi::Guid) -> efi::Status {
        efi::Status::NOT_FOUND
    }

    static FAKE_PROTOCOL: PcdProtocol = PcdProtocol {
        set_sku: fake_set_sku,
        get8: fake_get8,
        get16: fake_get16,
        get32: fake_get32,
        get64: fake_get64,
        get_ptr: fake_get_ptr,
        get_bool: fake_get_bool,
        get_size: fake_get_size,
        get8_ex: fake_get8_ex,
        get16_ex: fake_get16_ex,
        get32_ex: fake_get32_ex,
        get64_ex: fake_get64_ex,
        get_ptr_ex: fake_get_ptr_ex,
        get_bool_ex: fake_get_bool_ex,
        get_size_ex: fake_get_size_ex,
        set8: fake_set8,
        set16: fake_set16,
        set32: fake_set32,
        set64: fake_set64,
        set_ptr: fake_set_ptr,
        set_bool: fake_set_bool,
        set8_ex: fake_set8_ex,
        set16_ex: fake_set16_ex,
        set32_ex: fake_set32_ex,
        set64_ex: fake_set64_ex,
        set_ptr_ex: fake_set_ptr_ex,
        set_bool_ex: fake_set_bool_ex,
        callback_on_set: fake_callback_on_set,
        cancel_callback: fake_cancel_callback,
        get_next_token: fake_get_next_token,
        get_next_token_space: fake_get_next_token_space,
    };

    // Silences an otherwise-unused-item warning; `fake_callback` only needs to exist as a value
    // assignable to `PcdCallback`, which the (unused) fields above don't require constructing.
    const _: PcdCallback = fake_callback;

    fn provider() -> PcdProvider {
        PcdProvider { protocol: Some(Protocol::mock(&FAKE_PROTOCOL)) }
    }

    #[test]
    fn test_pcd_provider_new_and_default_match() {
        let _ = PcdProvider::new();
        let _ = PcdProvider::default();
    }

    #[test]
    #[should_panic(expected = "must be dispatched")]
    fn test_pcd_provider_panics_if_not_dispatched() {
        // SAFETY: expected to panic before the token is ever used.
        let _ = unsafe { PcdProvider::new().get_u8(PcdToken::Dynamic(TOKEN_U8)) };
    }

    #[test]
    fn test_pcd_provider_entry_point_registers_service() {
        let protocol = Protocol::mock(&FAKE_PROTOCOL);
        assert!(PcdProvider::new().entry_point(protocol, Commands::mock()).is_ok());
    }

    #[test]
    fn test_pcd_provider_get_size() {
        // SAFETY: TOKEN_BYTES is a well-known token understood by FAKE_PROTOCOL.
        assert_eq!(unsafe { provider().get_size(PcdToken::Dynamic(TOKEN_BYTES)) }, BYTES_VALUE.len());
    }

    #[test]
    fn test_pcd_provider_get_u8() {
        // SAFETY: TOKEN_U8 is a well-known token understood by FAKE_PROTOCOL.
        assert_eq!(unsafe { provider().get_u8(PcdToken::Dynamic(TOKEN_U8)) }, Ok(42));
    }

    #[test]
    fn test_pcd_provider_get_u8_size_mismatch() {
        // SAFETY: TOKEN_WRONG_SIZE is a well-known token understood by FAKE_PROTOCOL.
        let result = unsafe { provider().get_u8(PcdToken::Dynamic(TOKEN_WRONG_SIZE)) };
        assert_eq!(result, Err(PcdError::SizeMismatch { actual: size_of::<u32>(), expected: size_of::<u8>() }));
    }

    #[test]
    fn test_pcd_provider_get_u32() {
        // SAFETY: TOKEN_U32 is a well-known token understood by FAKE_PROTOCOL.
        assert_eq!(unsafe { provider().get_u32(PcdToken::Dynamic(TOKEN_U32)) }, Ok(U32_VALUE));
    }

    #[test]
    fn test_pcd_provider_get_bool() {
        // SAFETY: TOKEN_BOOL is a well-known token understood by FAKE_PROTOCOL.
        assert_eq!(unsafe { provider().get_bool(PcdToken::Dynamic(TOKEN_BOOL)) }, Ok(true));
    }

    #[test]
    fn test_pcd_provider_get_bytes() {
        // SAFETY: TOKEN_BYTES is a well-known token understood by FAKE_PROTOCOL.
        assert_eq!(unsafe { provider().get_bytes(PcdToken::Dynamic(TOKEN_BYTES)) }, Ok(BYTES_VALUE.to_vec()));
    }

    #[test]
    fn test_pcd_provider_get_bytes_zero_size_returns_empty() {
        // SAFETY: TOKEN_ZERO_SIZE is a well-known token understood by FAKE_PROTOCOL.
        assert_eq!(unsafe { provider().get_bytes(PcdToken::Dynamic(TOKEN_ZERO_SIZE)) }, Ok(Vec::new()));
    }

    #[test]
    fn test_pcd_provider_get_bytes_null_ptr_returns_not_found() {
        // SAFETY: TOKEN_NULL_PTR is a well-known token understood by FAKE_PROTOCOL.
        assert_eq!(unsafe { provider().get_bytes(PcdToken::Dynamic(TOKEN_NULL_PTR)) }, Err(PcdError::NotFound));
    }

    #[test]
    fn test_pcd_provider_dynamic_ex_get_u16() {
        // SAFETY: TOKEN_EX_U16/EX_GUID are well-known values understood by FAKE_PROTOCOL.
        assert_eq!(unsafe { provider().get_u16(PcdToken::DynamicEx(EX_GUID, TOKEN_EX_U16)) }, Ok(U16_EX_VALUE));
    }

    #[test]
    fn test_pcd_provider_set_u8_success() {
        // SAFETY: TOKEN_SET_OK is a well-known token understood by FAKE_PROTOCOL.
        assert_eq!(unsafe { provider().set_u8(PcdToken::Dynamic(TOKEN_SET_OK), 1) }, Ok(()));
    }

    #[test]
    fn test_pcd_provider_set_u8_not_found() {
        // SAFETY: TOKEN_SET_NOT_FOUND is a well-known token understood by FAKE_PROTOCOL.
        assert_eq!(unsafe { provider().set_u8(PcdToken::Dynamic(TOKEN_SET_NOT_FOUND), 1) }, Err(PcdError::NotFound));
    }

    #[test]
    fn test_pcd_provider_set_bytes_success() {
        // SAFETY: TOKEN_SET_BYTES_MAX4 is a well-known token understood by FAKE_PROTOCOL.
        assert_eq!(unsafe { provider().set_bytes(PcdToken::Dynamic(TOKEN_SET_BYTES_MAX4), &[1, 2, 3, 4]) }, Ok(()));
    }

    #[test]
    fn test_pcd_provider_set_bytes_too_large() {
        // SAFETY: TOKEN_SET_BYTES_MAX4 is a well-known token understood by FAKE_PROTOCOL.
        let result = unsafe { provider().set_bytes(PcdToken::Dynamic(TOKEN_SET_BYTES_MAX4), &[1, 2, 3, 4, 5]) };
        assert_eq!(result, Err(PcdError::BufferTooLarge { max_size: 4 }));
    }
}
