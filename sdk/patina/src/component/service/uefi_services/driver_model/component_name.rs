//! Component name protocol production for Patina components.
//!
//! A component can implement [`UefiDriverModelComponentName`] and install it with
//! [`install_uefi_driver_model_component_name`](crate::component::service::uefi_services::protocol::ProtocolServicesExt::install_uefi_driver_model_component_name).
//!
//! This produces both `EFI_COMPONENT_NAME_PROTOCOL` and `EFI_COMPONENT_NAME2_PROTOCOL` on the same
//! handle, computing each protocol's `SupportedLanguages` string from the driver name table
//! so each component does not have to maintain it separately.
//!
//! The type name chosen in this file is deliberately verbose because of the overlapping and unrelated
//! terminology of "Patina components" and the UEFI Driver Model "Component Name" protocols.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::boxed::Box;
use alloc::string::String;

use crate::base::guid::BinaryGuid;
use crate::base::protocol::ProtocolInterface;
use crate::string::{Char8Str, Char8String};
use crate::uefi::component_name::{component_name, component_name2};

use super::language::LanguageTable;
pub use crate::component::service::uefi_services::handle::Handle;

/// A component's user readable driver and controller names.
///
/// Implement this trait and pass it to
/// [`install_uefi_driver_model_component_name`](crate::component::service::uefi_services::protocol::ProtocolServicesExt::install_uefi_driver_model_component_name)
/// to publish both component name protocols from one implementation.
///
/// `controller_name` defaults to returning `None`, which reports that this driver does not
/// describe any controller by name. Most drivers only need `driver_name`. A bus driver that wants
/// to name the controllers it manages should override `controller_name` as well.
pub trait UefiDriverModelComponentName {
    /// Returns the table of names for this driver itself.
    fn driver_name(&self) -> &'static LanguageTable;

    /// Returns the table of names for a controller this driver manages, or `None` if this driver
    /// does not have a name for it.
    ///
    /// `child` is the child controller to name, if any, matching
    /// `EFI_COMPONENT_NAME2_PROTOCOL.GetControllerName`'s `ChildHandle` parameter.
    fn controller_name(&self, controller: Handle, child: Option<Handle>) -> Option<&'static LanguageTable> {
        let _ = (controller, child);
        None
    }
}

/// Shared state behind both the V1 and V2 wrappers for one component.
///
/// The `SupportedLanguages` strings are computed once here, from `component_name.driver_name()`,
/// rather than a value the component maintains separately.
struct UefiDriverModelComponentNameHolder<C: UefiDriverModelComponentName> {
    component_name: C,
    supported_languages_v1: Char8String,
    supported_languages_v2: Char8String,
}

/// Wraps the `EFI_COMPONENT_NAME_PROTOCOL` structure alongside a reference to the shared holder.
///
/// `protocol` is the first field so a pointer to this struct can be safely reinterpreted as a
/// pointer to `component_name::Protocol`.
#[repr(C)]
pub(in crate::component::service::uefi_services) struct ComponentNameShim<C: UefiDriverModelComponentName + 'static> {
    protocol: component_name::Protocol,
    holder: &'static UefiDriverModelComponentNameHolder<C>,
}

// SAFETY: `protocol` is the first field of `ComponentNameShim`, which is `#[repr(C)]`, so a
// pointer to this struct is also a valid pointer to `component_name::Protocol`.
unsafe impl<C: UefiDriverModelComponentName> ProtocolInterface for ComponentNameShim<C> {
    const PROTOCOL_GUID: BinaryGuid = BinaryGuid(component_name::PROTOCOL_GUID);
}

/// Wraps the `EFI_COMPONENT_NAME2_PROTOCOL` structure alongside a reference to the shared holder.
///
/// `protocol` is the first field so a pointer to this struct can be safely reinterpreted as a
/// pointer to `component_name2::Protocol`.
#[repr(C)]
pub(in crate::component::service::uefi_services) struct ComponentName2Shim<C: UefiDriverModelComponentName + 'static> {
    protocol: component_name2::Protocol,
    holder: &'static UefiDriverModelComponentNameHolder<C>,
}

// SAFETY: `protocol` is the first field of `ComponentName2Shim`, which is `#[repr(C)]`, so a
// pointer to this struct is also a valid pointer to `component_name2::Protocol`.
unsafe impl<C: UefiDriverModelComponentName> ProtocolInterface for ComponentName2Shim<C> {
    const PROTOCOL_GUID: BinaryGuid = BinaryGuid(component_name2::PROTOCOL_GUID);
}

/// Builds the `EFI_COMPONENT_NAME_PROTOCOL.SupportedLanguages` value for a driver name table.
///
/// ISO 639-2 codes are concatenated with no separator, matching the format EDK2 drivers use.
fn build_iso639_supported_languages(table: &LanguageTable) -> Char8String {
    let mut languages = String::new();
    for entry in table.0 {
        languages.push_str(core::str::from_utf8(entry.iso639).expect("ISO 639-2 codes must be ASCII"));
    }
    Char8String::try_from_str(&languages).expect("ISO 639-2 codes must be valid Latin-1 with no embedded NUL")
}

/// Builds the `EFI_COMPONENT_NAME2_PROTOCOL.SupportedLanguages` value for a driver name table.
///
/// RFC 4646 tags are joined with `;`, matching the format EDK2 drivers use.
fn build_rfc4646_supported_languages(table: &LanguageTable) -> Char8String {
    let mut languages = String::new();
    for (index, entry) in table.0.iter().enumerate() {
        if index > 0 {
            languages.push(';');
        }
        languages.push_str(entry.rfc4646);
    }
    Char8String::try_from_str(&languages).expect("RFC 4646 tags must be valid Latin-1 with no embedded NUL")
}

/// Reads a fixed three byte ISO 639-2 code from a raw, possibly non NUL terminated pointer.
///
/// # Safety
///
/// `language` must be non-null and point to at least three readable bytes, per
/// `EFI_COMPONENT_NAME_PROTOCOL.GetDriverName`/`GetControllerName`'s `Language` contract.
unsafe fn read_iso639(language: *const u8) -> [u8; 3] {
    // SAFETY: forwarded from the caller's contract on this function.
    unsafe { [*language, *language.add(1), *language.add(2)] }
}

extern "efiapi" fn get_driver_name_v1<C: UefiDriverModelComponentName + 'static>(
    this: *mut component_name::Protocol,
    language: *mut u8,
    driver_name: *mut *mut u16,
) -> r_efi::base::Status {
    // SAFETY: `this` always points to the `protocol` field of a `ComponentNameShim<C>` built by
    // `install_uefi_driver_model_component_name`, which is that struct's first field under
    // `#[repr(C)]`.
    let Some(shim) = (unsafe { (this as *const ComponentNameShim<C>).as_ref() }) else {
        return r_efi::base::Status::INVALID_PARAMETER;
    };
    if language.is_null() || driver_name.is_null() {
        return r_efi::base::Status::INVALID_PARAMETER;
    }
    // SAFETY: `language` was checked non-null above; the caller guarantees it is valid for reads
    // per `read_iso639`'s contract.
    let code = unsafe { read_iso639(language) };
    match shim.holder.component_name.driver_name().lookup_v1(code) {
        Some(name) => {
            // SAFETY: `driver_name` was checked non-null above, and `name` outlives this call.
            unsafe { *driver_name = name.as_ptr().cast_mut() };
            r_efi::base::Status::SUCCESS
        }
        None => r_efi::base::Status::UNSUPPORTED,
    }
}

extern "efiapi" fn get_controller_name_v1<C: UefiDriverModelComponentName + 'static>(
    this: *mut component_name::Protocol,
    controller_handle: r_efi::base::Handle,
    child_handle: r_efi::base::Handle,
    language: *mut u8,
    controller_name: *mut *mut u16,
) -> r_efi::base::Status {
    // SAFETY: as in `get_driver_name_v1`.
    let Some(shim) = (unsafe { (this as *const ComponentNameShim<C>).as_ref() }) else {
        return r_efi::base::Status::INVALID_PARAMETER;
    };
    let Some(controller) = Handle::from_raw(controller_handle) else {
        return r_efi::base::Status::INVALID_PARAMETER;
    };
    if language.is_null() || controller_name.is_null() {
        return r_efi::base::Status::INVALID_PARAMETER;
    }
    let child = Handle::from_raw(child_handle);
    // SAFETY: as in `get_driver_name_v1`.
    let code = unsafe { read_iso639(language) };
    let Some(table) = shim.holder.component_name.controller_name(controller, child) else {
        return r_efi::base::Status::UNSUPPORTED;
    };
    match table.lookup_v1(code) {
        Some(name) => {
            // SAFETY: as in `get_driver_name_v1`.
            unsafe { *controller_name = name.as_ptr().cast_mut() };
            r_efi::base::Status::SUCCESS
        }
        None => r_efi::base::Status::UNSUPPORTED,
    }
}

extern "efiapi" fn get_driver_name_v2<C: UefiDriverModelComponentName + 'static>(
    this: *mut component_name2::Protocol,
    language: *mut u8,
    driver_name: *mut *mut u16,
) -> r_efi::base::Status {
    // SAFETY: `this` always points to the `protocol` field of a `ComponentName2Shim<C>` built by
    // `install_uefi_driver_model_component_name`, which is that struct's first field under
    // `#[repr(C)]`.
    let Some(shim) = (unsafe { (this as *const ComponentName2Shim<C>).as_ref() }) else {
        return r_efi::base::Status::INVALID_PARAMETER;
    };
    if language.is_null() || driver_name.is_null() {
        return r_efi::base::Status::INVALID_PARAMETER;
    }
    // SAFETY: the UEFI caller guarantees `language` points to a NUL terminated RFC 4646 tag, per
    // `EFI_COMPONENT_NAME2_PROTOCOL.GetDriverName`'s contract.
    let language = unsafe { Char8Str::from_ptr(language.cast_const()) };
    match shim.holder.component_name.driver_name().lookup_v2(language) {
        Some(name) => {
            // SAFETY: `driver_name` was checked non-null above, and `name` outlives this call.
            unsafe { *driver_name = name.as_ptr().cast_mut() };
            r_efi::base::Status::SUCCESS
        }
        None => r_efi::base::Status::UNSUPPORTED,
    }
}

extern "efiapi" fn get_controller_name_v2<C: UefiDriverModelComponentName + 'static>(
    this: *mut component_name2::Protocol,
    controller_handle: r_efi::base::Handle,
    child_handle: r_efi::base::Handle,
    language: *mut u8,
    controller_name: *mut *mut u16,
) -> r_efi::base::Status {
    // SAFETY: as in `get_driver_name_v2`.
    let Some(shim) = (unsafe { (this as *const ComponentName2Shim<C>).as_ref() }) else {
        return r_efi::base::Status::INVALID_PARAMETER;
    };
    let Some(controller) = Handle::from_raw(controller_handle) else {
        return r_efi::base::Status::INVALID_PARAMETER;
    };
    if language.is_null() || controller_name.is_null() {
        return r_efi::base::Status::INVALID_PARAMETER;
    }
    let child = Handle::from_raw(child_handle);
    // SAFETY: as in `get_driver_name_v2`.
    let language = unsafe { Char8Str::from_ptr(language.cast_const()) };
    let Some(table) = shim.holder.component_name.controller_name(controller, child) else {
        return r_efi::base::Status::UNSUPPORTED;
    };
    match table.lookup_v2(language) {
        Some(name) => {
            // SAFETY: as in `get_driver_name_v2`.
            unsafe { *controller_name = name.as_ptr().cast_mut() };
            r_efi::base::Status::SUCCESS
        }
        None => r_efi::base::Status::UNSUPPORTED,
    }
}

/// Builds the V1 and V2 shims for `names`, ready to install on the same handle.
///
/// The two shims share one leaked [`UefiDriverModelComponentNameHolder`], so `names` and the
/// computed `SupportedLanguages` strings are only built once.
pub(in crate::component::service::uefi_services) fn build_shims<C: UefiDriverModelComponentName + 'static>(
    names: C,
) -> (Box<ComponentNameShim<C>>, Box<ComponentName2Shim<C>>) {
    let table = names.driver_name();
    let supported_languages_v1 = build_iso639_supported_languages(table);
    let supported_languages_v2 = build_rfc4646_supported_languages(table);

    let holder: &'static UefiDriverModelComponentNameHolder<C> =
        Box::leak(Box::new(UefiDriverModelComponentNameHolder {
            component_name: names,
            supported_languages_v1,
            supported_languages_v2,
        }));

    let v1 = Box::new(ComponentNameShim {
        protocol: component_name::Protocol {
            get_driver_name: get_driver_name_v1::<C>,
            get_controller_name: get_controller_name_v1::<C>,
            supported_languages: holder.supported_languages_v1.as_ptr().cast_mut(),
        },
        holder,
    });
    let v2 = Box::new(ComponentName2Shim {
        protocol: component_name2::Protocol {
            get_driver_name: get_driver_name_v2::<C>,
            get_controller_name: get_controller_name_v2::<C>,
            supported_languages: holder.supported_languages_v2.as_ptr().cast_mut(),
        },
        holder,
    });
    (v1, v2)
}

#[cfg(test)]
mod tests {
    use super::super::language::LanguageEntry;
    use super::*;
    use crate::char16;
    use crate::string::Char16Str;
    use core::ffi::c_void;
    use core::ptr::NonNull;

    static DRIVER_NAMES: LanguageTable = LanguageTable(&[
        LanguageEntry { iso639: b"eng", rfc4646: "en", name: char16!("Test Driver") },
        LanguageEntry { iso639: b"fra", rfc4646: "fr", name: char16!("Alternate Test Driver") },
    ]);

    static CONTROLLER_NAMES: LanguageTable =
        LanguageTable(&[LanguageEntry { iso639: b"eng", rfc4646: "en", name: char16!("Test Controller") }]);

    struct TestComponentName {
        controller_table: Option<&'static LanguageTable>,
    }

    impl UefiDriverModelComponentName for TestComponentName {
        fn driver_name(&self) -> &'static LanguageTable {
            &DRIVER_NAMES
        }

        fn controller_name(&self, _controller: Handle, _child: Option<Handle>) -> Option<&'static LanguageTable> {
            self.controller_table
        }
    }

    fn fake_handle() -> Handle {
        Handle::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()
    }

    fn shims(
        controller_table: Option<&'static LanguageTable>,
    ) -> (Box<ComponentNameShim<TestComponentName>>, Box<ComponentName2Shim<TestComponentName>>) {
        build_shims(TestComponentName { controller_table })
    }

    #[test]
    fn test_shim_layouts_match_protocol_first_field() {
        let (v1, v2) = shims(None);
        assert_eq!(
            core::ptr::from_ref::<ComponentNameShim<TestComponentName>>(v1.as_ref()) as usize,
            (&raw const v1.protocol) as usize,
            "protocol must be the first field"
        );
        assert_eq!(
            core::ptr::from_ref::<ComponentName2Shim<TestComponentName>>(v2.as_ref()) as usize,
            (&raw const v2.protocol) as usize,
            "protocol must be the first field"
        );
    }

    #[test]
    fn test_trampolines_null_this_is_invalid_parameter() {
        let mut out: *mut u16 = core::ptr::null_mut();
        let mut language = *b"eng\0";

        assert_eq!(
            get_driver_name_v1::<TestComponentName>(core::ptr::null_mut(), language.as_mut_ptr(), &raw mut out),
            r_efi::base::Status::INVALID_PARAMETER
        );
        assert_eq!(
            get_controller_name_v1::<TestComponentName>(
                core::ptr::null_mut(),
                fake_handle().as_raw(),
                core::ptr::null_mut(),
                language.as_mut_ptr(),
                &raw mut out
            ),
            r_efi::base::Status::INVALID_PARAMETER
        );
        assert_eq!(
            get_driver_name_v2::<TestComponentName>(core::ptr::null_mut(), language.as_mut_ptr(), &raw mut out),
            r_efi::base::Status::INVALID_PARAMETER
        );
        assert_eq!(
            get_controller_name_v2::<TestComponentName>(
                core::ptr::null_mut(),
                fake_handle().as_raw(),
                core::ptr::null_mut(),
                language.as_mut_ptr(),
                &raw mut out
            ),
            r_efi::base::Status::INVALID_PARAMETER
        );
    }

    #[test]
    fn test_trampolines_null_language_or_out_param_is_invalid_parameter() {
        let (v1, v2) = shims(None);
        let v1_this = (&raw const v1.protocol).cast_mut();
        let v2_this = (&raw const v2.protocol).cast_mut();
        let mut out: *mut u16 = core::ptr::null_mut();
        let mut language = *b"eng\0";

        assert_eq!(
            get_driver_name_v1::<TestComponentName>(v1_this, core::ptr::null_mut(), &raw mut out),
            r_efi::base::Status::INVALID_PARAMETER
        );
        assert_eq!(
            get_driver_name_v1::<TestComponentName>(v1_this, language.as_mut_ptr(), core::ptr::null_mut()),
            r_efi::base::Status::INVALID_PARAMETER
        );
        assert_eq!(
            get_driver_name_v2::<TestComponentName>(v2_this, core::ptr::null_mut(), &raw mut out),
            r_efi::base::Status::INVALID_PARAMETER
        );
        assert_eq!(
            get_driver_name_v2::<TestComponentName>(v2_this, language.as_mut_ptr(), core::ptr::null_mut()),
            r_efi::base::Status::INVALID_PARAMETER
        );
    }

    #[test]
    fn test_get_driver_name_v1_success_and_unsupported() {
        let (v1, _v2) = shims(None);
        let this = (&raw const v1.protocol).cast_mut();
        let mut out: *mut u16 = core::ptr::null_mut();

        let mut eng = *b"eng\0";
        assert_eq!(
            get_driver_name_v1::<TestComponentName>(this, eng.as_mut_ptr(), &raw mut out),
            r_efi::base::Status::SUCCESS
        );
        // SAFETY: `out` was just written by a successful call above.
        assert!(unsafe { Char16Str::from_ptr(out) }.unwrap() == "Test Driver");

        let mut deu = *b"deu\0";
        assert_eq!(
            get_driver_name_v1::<TestComponentName>(this, deu.as_mut_ptr(), &raw mut out),
            r_efi::base::Status::UNSUPPORTED
        );
    }

    #[test]
    fn test_get_driver_name_v2_case_insensitive_and_unsupported() {
        let (_v1, v2) = shims(None);
        let this = (&raw const v2.protocol).cast_mut();
        let mut out: *mut u16 = core::ptr::null_mut();

        let mut fr = *b"FR\0";
        assert_eq!(
            get_driver_name_v2::<TestComponentName>(this, fr.as_mut_ptr(), &raw mut out),
            r_efi::base::Status::SUCCESS
        );
        // SAFETY: `out` was just written by a successful call above.
        assert!(unsafe { Char16Str::from_ptr(out) }.unwrap() == "Alternate Test Driver");

        let mut de = *b"de\0";
        assert_eq!(
            get_driver_name_v2::<TestComponentName>(this, de.as_mut_ptr(), &raw mut out),
            r_efi::base::Status::UNSUPPORTED
        );
    }

    #[test]
    fn test_controller_name_defaults_to_unsupported() {
        let (v1, v2) = shims(None);
        let v1_this = (&raw const v1.protocol).cast_mut();
        let v2_this = (&raw const v2.protocol).cast_mut();
        let mut out: *mut u16 = core::ptr::null_mut();
        let mut eng = *b"eng\0";
        let mut en = *b"en\0";
        let controller = fake_handle().as_raw();

        assert_eq!(
            get_controller_name_v1::<TestComponentName>(
                v1_this,
                controller,
                core::ptr::null_mut(),
                eng.as_mut_ptr(),
                &raw mut out
            ),
            r_efi::base::Status::UNSUPPORTED
        );
        assert_eq!(
            get_controller_name_v2::<TestComponentName>(
                v2_this,
                controller,
                core::ptr::null_mut(),
                en.as_mut_ptr(),
                &raw mut out
            ),
            r_efi::base::Status::UNSUPPORTED
        );
    }

    #[test]
    fn test_controller_name_success_when_overridden() {
        let (v1, v2) = shims(Some(&CONTROLLER_NAMES));
        let v1_this = (&raw const v1.protocol).cast_mut();
        let v2_this = (&raw const v2.protocol).cast_mut();
        let mut out: *mut u16 = core::ptr::null_mut();
        let mut eng = *b"eng\0";
        let mut en = *b"en\0";
        let controller = fake_handle().as_raw();

        assert_eq!(
            get_controller_name_v1::<TestComponentName>(
                v1_this,
                controller,
                core::ptr::null_mut(),
                eng.as_mut_ptr(),
                &raw mut out
            ),
            r_efi::base::Status::SUCCESS
        );
        // SAFETY: `out` was just written by a successful call above.
        assert!(unsafe { Char16Str::from_ptr(out) }.unwrap() == "Test Controller");

        assert_eq!(
            get_controller_name_v2::<TestComponentName>(
                v2_this,
                controller,
                core::ptr::null_mut(),
                en.as_mut_ptr(),
                &raw mut out
            ),
            r_efi::base::Status::SUCCESS
        );
        // SAFETY: `out` was just written by a successful call above.
        assert!(unsafe { Char16Str::from_ptr(out) }.unwrap() == "Test Controller");
    }

    #[test]
    fn test_build_shims_computes_supported_languages() {
        let (v1, v2) = shims(None);
        // SAFETY: `supported_languages` always points to the holder's owned, NUL terminated
        // buffer for the lifetime of the shim.
        let v1_languages = unsafe { Char8Str::from_ptr(v1.protocol.supported_languages.cast_const()) };
        // SAFETY: as above.
        let v2_languages = unsafe { Char8Str::from_ptr(v2.protocol.supported_languages.cast_const()) };
        assert!(v1_languages == "engfra");
        assert!(v2_languages == "en;fr");
    }
}
