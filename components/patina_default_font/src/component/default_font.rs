//! The default font component.
//!
//! [`DefaultFontProvider`] registers the default narrow-glyph "simple font" package with
//! EFI HII Database Protocol(`EFI_HII_DATABASE_PROTOCOL`) once it is installed.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use patina::{
    component::{component, protocol::Protocol},
    error::Result,
    standard::efi::protocols::hii_database,
};

use crate::font_package;

/// Registers the default narrow-glyph "simple font" package with the HII database.
///
/// Dispatched once `EFI_HII_DATABASE_PROTOCOL` is installed.
#[derive(Default)]
pub struct DefaultFontProvider;

#[component]
impl DefaultFontProvider {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, hii_database: Protocol<hii_database::Protocol>) -> Result<()> {
        if let Err(err) = font_package::register(&hii_database) {
            log::warn!("Failed to register default HII font package: {err:?}");
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina::standard::efi::{self, hii};

    #[test]
    fn test_default_font_provider_entry_point_registers_package() {
        let fake = FakeHiiDatabase::new(efi::Status::SUCCESS);

        let result = DefaultFontProvider::new().entry_point(Protocol::mock(fake.protocol()));

        assert_eq!(result, Ok(()));
        assert_eq!(fake.call_count(), 1);
    }

    #[test]
    fn test_default_font_provider_entry_point_ignores_registration_failure() {
        let fake = FakeHiiDatabase::new(efi::Status::DEVICE_ERROR);

        let result = DefaultFontProvider::new().entry_point(Protocol::mock(fake.protocol()));

        // A registration failure is only logged.
        assert_eq!(result, Ok(()));
        assert_eq!(fake.call_count(), 1);
    }

    /// A fake `EFI_HII_DATABASE_PROTOCOL` whose `NewPackageList()` always reports `result`.
    /// Every other method is unused by [`DefaultFontProvider::entry_point`] and stubbed out just
    /// to satisfy the protocol's layout.
    #[repr(C)]
    struct FakeHiiDatabase {
        protocol: hii_database::Protocol,
        call_count: core::cell::Cell<u32>,
        result: efi::Status,
    }

    impl FakeHiiDatabase {
        /// Builds a leaked, `'static` fake HII Database backing a real `Protocol<hii_database::Protocol>`.
        fn new(result: efi::Status) -> &'static FakeHiiDatabase {
            alloc::boxed::Box::leak(alloc::boxed::Box::new(FakeHiiDatabase {
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
                result,
            }))
        }

        fn protocol(&'static self) -> &'static hii_database::Protocol {
            &self.protocol
        }

        fn call_count(&self) -> u32 {
            self.call_count.get()
        }
    }

    unsafe extern "efiapi" fn fake_new_package_list(
        this: *const hii_database::Protocol,
        _package_list: *const hii::PackageListHeader,
        _driver_handle: efi::Handle,
        handle: *mut hii::Handle,
    ) -> efi::Status {
        // SAFETY: `this` always points to the `protocol` field of a `FakeHiiDatabase`, this
        // module's only caller, which is `#[repr(C)]` with `protocol` as its first field.
        let fake = unsafe { &*this.cast::<FakeHiiDatabase>() };
        fake.call_count.set(fake.call_count.get() + 1);

        if !handle.is_null() {
            // SAFETY: a valid out-parameter per `NewPackageList()`'s safety contract.
            unsafe { handle.write(core::ptr::null_mut()) };
        }
        fake.result
    }

    unsafe extern "efiapi" fn fake_remove_package_list(
        _this: *const hii_database::Protocol,
        _handle: hii::Handle,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn fake_update_package_list(
        _this: *const hii_database::Protocol,
        _handle: hii::Handle,
        _package_list: *const hii::PackageListHeader,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn fake_list_package_lists(
        _this: *const hii_database::Protocol,
        _package_type: u8,
        _package_guid: *const efi::Guid,
        _handle_buffer_length: *mut usize,
        _handle: *mut hii::Handle,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn fake_export_package_lists(
        _this: *const hii_database::Protocol,
        _handle: hii::Handle,
        _buffer_size: *mut usize,
        _buffer: *mut hii::PackageListHeader,
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
        _package_list_handle: hii::Handle,
        _driver_handle: *mut efi::Handle,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }
}
