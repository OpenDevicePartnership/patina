//! Registers the default "simple font" glyph package with `EFI_HII_DATABASE_PROTOCOL`.
//!
//! The EDK II `GraphicsConsoleDxe` driver registered a default font package, watching for
//! `EFI_HII_DATABASE_PROTOCOL` to appear.  This module ports that registration into this
//! component.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use patina::{
    BinaryGuid,
    error::{EfiError, Result},
    standard::efi::{hii, protocols::hii_database},
};
use zerocopy::IntoBytes;

use super::font_data::NARROW_GLYPHS;

/// Reused from the EDK II `GraphicsConsoleDxe` driver for compatibility with anything that
/// looks up this exact package list.
const FONT_PACKAGE_LIST_GUID: BinaryGuid =
    BinaryGuid::from_fields(0xf5f219d3, 0x7006, 0x4648, 0xac, 0x8d, &[0xd6, 0x1d, 0xfb, 0x7b, 0xc6, 0xad]);

/// Ensures [`register`] only calls `NewPackageList()` once, regardless of how many GOP
/// controllers this component's driver binding starts.
static REGISTERED: AtomicBool = AtomicBool::new(false);

/// Appends an `EFI_HII_PACKAGE_HEADER`'s bitfield `Length:24, Type:8`.
fn push_package_header(buffer: &mut Vec<u8>, length: u32, package_type: u8) {
    buffer.extend_from_slice(&length.to_le_bytes()[..3]);
    buffer.push(package_type);
}

// Package layout (UEFI Specification 2.11 33.3): A `PackageListHeader`, followed by one or more
// packages (each a `PackageHeader` plus type-specific data), followed by an `END` package.

/// UEFI Specification 2.11 3.3.2.1: `EFI_HII_SIMPLE_FONT_PACKAGE_HDR`
const FONT_PACKAGE_HEADER_LEN: usize = 4 + 2 + 2; // PackageHeader + NumberOfNarrowGlyphs/WideGlyphs.
/// UEFI Specification 2.11 3.3.1.1: `EFI_HII_PACKAGE_HEADER`
const END_PACKAGE_LEN: usize = 4; // A bare PackageHeader with no data.
/// UEFI Specification 2.11 3.3.1.2: `EFI_HII_PACKAGE_LIST_HEADER`
const LIST_HEADER_LEN: usize = 16 + 4; // Guid + PackageLength.

/// Builds the raw `PackageListHeader` + `EFI_HII_PACKAGE_SIMPLE_FONTS` package + `END` package
/// byte buffer that [`register`] hands to `NewPackageList()`. Split out from `register()` so the
/// byte layout can be verified without an `EFI_HII_DATABASE_PROTOCOL` interface.
fn build_package_list() -> Vec<u8> {
    let glyphs_len = NARROW_GLYPHS.as_bytes().len();
    let font_package_len = FONT_PACKAGE_HEADER_LEN + glyphs_len;
    let total_len = LIST_HEADER_LEN + font_package_len + END_PACKAGE_LEN;

    let mut buffer = Vec::with_capacity(total_len);
    buffer.extend_from_slice(FONT_PACKAGE_LIST_GUID.as_bytes());
    buffer.extend_from_slice(&(total_len as u32).to_le_bytes());

    push_package_header(&mut buffer, font_package_len as u32, hii::PACKAGE_SIMPLE_FONTS);
    buffer.extend_from_slice(&(NARROW_GLYPHS.len() as u16).to_le_bytes()); // NumberOfNarrowGlyphs.
    buffer.extend_from_slice(&0u16.to_le_bytes()); // NumberOfWideGlyphs: no wide glyphs are shipped.
    buffer.extend_from_slice(NARROW_GLYPHS.as_bytes());

    push_package_header(&mut buffer, END_PACKAGE_LEN as u32, hii::PACKAGE_END);
    buffer
}

/// Registers the default narrow-glyph font package with the HII database.
/// A no-op after the first successful call.
pub(crate) fn register(hii_database: &hii_database::Protocol) -> Result<()> {
    if REGISTERED.swap(true, Ordering::AcqRel) {
        return Ok(());
    }

    let buffer = build_package_list();

    let mut hii_handle: hii::Handle = core::ptr::null_mut();
    // SAFETY: `hii_database` is an active `EFI_HII_DATABASE_PROTOCOL` interface found with `locate_protocol`.
    // `buffer` was just built above to match the `PackageListHeader` + packages + `END` package format the
    // UEFI Spec HII documentation requires, with its length matching `PackageListHeader.package_length`.
    // `NewPackageList` copies the package list internally rather than retaining the pointer. Using `NULL` for
    // the driver handle is done to reflect this as a system-wide package (not tied to a specific driver).
    let status = unsafe {
        (hii_database.new_package_list)(
            hii_database,
            buffer.as_ptr().cast::<hii::PackageListHeader>(),
            core::ptr::null_mut(),
            &raw mut hii_handle,
        )
    };
    EfiError::status_to_result(status)
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina::standard::efi;

    #[test]
    fn test_build_package_list_length_matches_declared_package_length() {
        let buffer = build_package_list();

        // `PackageListHeader.package_length` (the 4 bytes right after the 16-byte Guid) must equal
        // the buffer's own length, or `NewPackageList` has no way to know where the list ends.
        let declared_len = u32::from_le_bytes(buffer[16..20].try_into().unwrap());
        assert_eq!(declared_len as usize, buffer.len());
    }

    #[test]
    fn test_build_package_list_guid_matches_edk2() {
        let buffer = build_package_list();
        assert_eq!(&buffer[0..16], FONT_PACKAGE_LIST_GUID.as_bytes());
    }

    #[test]
    fn test_build_package_list_font_package_header_fields() {
        let buffer = build_package_list();

        // The simple font package header starts right after the 20-byte PackageListHeader.
        let package_type = buffer[16 + 4 + 3];
        assert_eq!(package_type, hii::PACKAGE_SIMPLE_FONTS);

        let number_of_narrow_glyphs = u16::from_le_bytes(buffer[24..26].try_into().unwrap());
        assert_eq!(number_of_narrow_glyphs as usize, NARROW_GLYPHS.len());

        let number_of_wide_glyphs = u16::from_le_bytes(buffer[26..28].try_into().unwrap());
        assert_eq!(number_of_wide_glyphs, 0);
    }

    #[test]
    fn test_build_package_list_ends_with_end_package() {
        let buffer = build_package_list();

        // The last 4 bytes are the bare `END` package header: a 3-byte length of 4, then the type.
        let end_package = &buffer[buffer.len() - END_PACKAGE_LEN..];
        assert_eq!(end_package, &[4, 0, 0, hii::PACKAGE_END]);
    }

    #[test]
    fn test_build_package_list_contains_glyph_bytes() {
        let buffer = build_package_list();
        let glyphs_start = LIST_HEADER_LEN + FONT_PACKAGE_HEADER_LEN;
        let glyphs_end = glyphs_start + NARROW_GLYPHS.as_bytes().len();
        assert_eq!(&buffer[glyphs_start..glyphs_end], NARROW_GLYPHS.as_bytes());
    }

    #[test]
    fn test_register_calls_new_package_list_once_then_is_a_no_op() {
        let fake = FakeHiiDatabase::new();

        register(fake.protocol()).unwrap();
        assert_eq!(fake.call_count(), 1);
        assert_eq!(fake.last_buffer(), build_package_list());

        register(fake.protocol()).unwrap();
        assert_eq!(fake.call_count(), 1);
    }

    /// A fake `EFI_HII_DATABASE_PROTOCOL` that records `NewPackageList()` calls. Every other
    /// method is unused by [`register`] and stubbed out just to satisfy the protocol's layout.
    struct FakeHiiDatabase {
        protocol: hii_database::Protocol,
        call_count: core::cell::Cell<u32>,
        last_buffer: core::cell::RefCell<Vec<u8>>,
    }

    impl FakeHiiDatabase {
        fn new() -> alloc::boxed::Box<FakeHiiDatabase> {
            alloc::boxed::Box::new(FakeHiiDatabase {
                protocol: hii_database::Protocol {
                    new_package_list: fake_new_package_list,
                    remove_package_list: fake_remove_package_list,
                    update_package_list: fake_update_package_list,
                    list_package_lists: fake_list_package_lists,
                    export_package_lists: fake_export_package_lists,
                    register_package_notify: fake_register_package_notify,
                    unregister_package_notify: fake_unregister_package_notify,
                    find_keyboard_layouts: fake_find_keyboard_layouts,
                    get_keyboard_layout: fake_get_keyboard_layout,
                    set_keyboard_layout: fake_set_keyboard_layout,
                    get_package_list_handle: fake_get_package_list_handle,
                },
                call_count: core::cell::Cell::new(0),
                last_buffer: core::cell::RefCell::new(Vec::new()),
            })
        }

        fn protocol(&self) -> &hii_database::Protocol {
            &self.protocol
        }

        fn call_count(&self) -> u32 {
            self.call_count.get()
        }

        fn last_buffer(&self) -> Vec<u8> {
            self.last_buffer.borrow().clone()
        }
    }

    unsafe extern "efiapi" fn fake_new_package_list(
        this: *const hii_database::Protocol,
        package_list: *const patina::standard::efi::hii::PackageListHeader,
        _driver_handle: efi::Handle,
        handle: *mut patina::standard::efi::hii::Handle,
    ) -> efi::Status {
        // SAFETY: `this` always points to the `protocol` field of a `FakeHiiDatabase`, this
        // module's only caller, which is `#[repr(C)]` with `protocol` as its first field.
        let fake = unsafe { &*this.cast::<FakeHiiDatabase>() };
        fake.call_count.set(fake.call_count.get() + 1);

        let expected_len = build_package_list().len();
        // SAFETY: `register` always builds a buffer of `expected_len` bytes and passes a
        // pointer to its start, per the call to `register`.
        let bytes = unsafe { core::slice::from_raw_parts(package_list.cast::<u8>(), expected_len) };
        fake.last_buffer.replace(bytes.to_vec());

        if !handle.is_null() {
            // SAFETY: a valid out-parameter per `NewPackageList()`'s safety contract.
            unsafe { handle.write(core::ptr::null_mut()) };
        }
        efi::Status::SUCCESS
    }

    unsafe extern "efiapi" fn fake_remove_package_list(
        _this: *const hii_database::Protocol,
        _handle: patina::standard::efi::hii::Handle,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn fake_update_package_list(
        _this: *const hii_database::Protocol,
        _handle: patina::standard::efi::hii::Handle,
        _package_list: *const patina::standard::efi::hii::PackageListHeader,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn fake_list_package_lists(
        _this: *const hii_database::Protocol,
        _package_type: u8,
        _package_guid: *const efi::Guid,
        _handle_buffer_length: *mut usize,
        _handle: *mut patina::standard::efi::hii::Handle,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn fake_export_package_lists(
        _this: *const hii_database::Protocol,
        _handle: patina::standard::efi::hii::Handle,
        _buffer_size: *mut usize,
        _buffer: *mut patina::standard::efi::hii::PackageListHeader,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn fake_register_package_notify(
        _this: *const hii_database::Protocol,
        _package_type: u8,
        _package_guid: *const efi::Guid,
        _package_notify_fn: hii_database::Notify,
        _package_notify_type: hii_database::NotifyType,
        _notify_handle: *mut efi::Handle,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn fake_unregister_package_notify(
        _this: *const hii_database::Protocol,
        _notify_handle: efi::Handle,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn fake_find_keyboard_layouts(
        _this: *const hii_database::Protocol,
        _key_guid_buffer_length: *mut u16,
        _key_guid_buffer: *mut efi::Guid,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn fake_get_keyboard_layout(
        _this: *const hii_database::Protocol,
        _key_guid: *const efi::Guid,
        _keyboard_layout_length: *mut u16,
        _keyboard_layout: *mut hii_database::KeyboardLayout,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn fake_set_keyboard_layout(
        _this: *const hii_database::Protocol,
        _key_guid: *mut efi::Guid,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn fake_get_package_list_handle(
        _this: *const hii_database::Protocol,
        _package_list_handle: patina::standard::efi::hii::Handle,
        _driver_handle: *mut efi::Handle,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }
}
