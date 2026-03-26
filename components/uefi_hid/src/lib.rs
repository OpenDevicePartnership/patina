//! UEFI HID - Human Interface Device support as a Patina component.
//!
//! This crate provides a Patina component that consumes the HidIo protocol and
//! produces UEFI input protocols (SimpleTextInput, SimpleTextInputEx,
//! AbsolutePointer) for keyboard and pointer HID devices.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
#![cfg_attr(not(test), no_std)]
#![feature(coverage_attribute)]

extern crate alloc;

pub mod hid;
pub mod hid_io;
pub mod keyboard;
pub mod pointer;

use alloc::boxed::Box;

use r_efi::efi;

use patina::{
    BinaryGuid,
    boot_services::{BootServices, StandardBootServices},
    component::{component, params},
    driver_binding::UefiDriverBinding,
    error::Result,
    uefi_protocol::ProtocolInterface,
};

#[cfg(feature = "ctrl-alt-del")]
use core::sync::atomic::AtomicPtr;

#[cfg(feature = "ctrl-alt-del")]
use patina::runtime_services::StandardRuntimeServices;

/// Global pointer to UEFI Runtime Services, used by the Ctrl-Alt-Del reset callback.
#[cfg(feature = "ctrl-alt-del")]
pub static RUNTIME_SERVICES: AtomicPtr<efi::RuntimeServices> = AtomicPtr::new(core::ptr::null_mut());

/// Zero-sized marker protocol used to create a dedicated driver binding handle.
#[repr(C)]
struct UefiHidMarker;

// SAFETY: UefiHidMarker is a ZST whose GUID uniquely identifies this component.
unsafe impl ProtocolInterface for UefiHidMarker {
    const PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("122ffcfd-f8f8-46d6-81de-333e2419ebcb");
}

/// UEFI HID Patina component.
///
/// When dispatched, installs a UEFI Driver Binding that consumes HidIo
/// protocol instances and produces keyboard and pointer input protocols.
pub struct UefiHidComponent;

#[component]
impl UefiHidComponent {
    #[cfg(feature = "ctrl-alt-del")]
    fn entry_point(
        self,
        boot_services: StandardBootServices,
        image_handle: params::Handle,
        runtime_services: StandardRuntimeServices,
    ) -> Result<()> {
        RUNTIME_SERVICES.store(runtime_services.as_mut_ptr(), core::sync::atomic::Ordering::SeqCst);
        let boot_services: &'static StandardBootServices = Box::leak(Box::new(boot_services));
        install_hid_driver_binding(boot_services, *image_handle)
    }

    #[cfg(not(feature = "ctrl-alt-del"))]
    fn entry_point(self, boot_services: StandardBootServices, image_handle: params::Handle) -> Result<()> {
        let boot_services: &'static StandardBootServices = Box::leak(Box::new(boot_services));
        install_hid_driver_binding(boot_services, *image_handle)
    }
}

/// Installs the HID driver binding using the provided boot services.
///
/// Separated from the component entry point to allow testing with
/// `MockBootServices`.
fn install_hid_driver_binding<T: BootServices + Clone + 'static>(
    boot_services: &'static T,
    image_handle: efi::Handle,
) -> Result<()> {
    // Patina component model has a single image handle. Create a separate driver_binding handle for the driver binding
    // to avoid conflict on the image handle.
    let (driver_binding_handle, _marker_key) =
        boot_services.install_protocol_interface(None, Box::new(UefiHidMarker))?;

    let driver_binding = hid::HidDriver::new(boot_services, driver_binding_handle);

    let mut driver_binding =
        UefiDriverBinding::new_with_driver_handle(driver_binding, image_handle, driver_binding_handle, boot_services);

    driver_binding.install().map_err(patina::error::EfiError::from)?;
    // driver_binding is intentionally leaked by install() — it lives forever.

    Ok(())
}

#[cfg(test)]
mod test {
    use patina::boot_services::{MockBootServices, c_ptr::CPtr};

    use super::*;

    #[test]
    fn install_hid_binding_should_install_a_binding() {
        let boot_services = MockBootServices::new();
        let boot_services = Box::leak(Box::new(boot_services));

        boot_services.expect_install_protocol_interface::<UefiHidMarker, Box<UefiHidMarker>>().returning(
            |handle, protocol_interface| {
                assert_eq!(handle, None, "Expected no handle for marker protocol installation");
                Ok((0x5678 as efi::Handle, protocol_interface.metadata()))
            },
        );

        boot_services.expect_install_protocol_interface_unchecked().returning(|handle, protocol, interface| {
            if protocol == &efi::protocols::driver_binding::PROTOCOL_GUID {
                assert!(
                    handle.is_some_and(|handle| handle as usize == 0x5678),
                    "Expected correct handle for driver binding protocol"
                );
                assert!(!interface.is_null(), "Expected non-null interface for driver binding protocol");
                return Ok(0x9abc as efi::Handle);
            }
            panic!("Unexpected protocol installation: {:?}", protocol);
        });

        let mock_image_handle = 0x1234 as efi::Handle;
        install_hid_driver_binding(boot_services, mock_image_handle).expect("install should succeed");
    }

    #[test]
    fn install_hid_binding_handles_failure() {
        let boot_services = Box::leak(Box::new(MockBootServices::new()));

        boot_services
            .expect_install_protocol_interface::<UefiHidMarker, Box<UefiHidMarker>>()
            .returning(|_, _| Err(efi::Status::OUT_OF_RESOURCES));

        let mock_image_handle = 0x1234 as efi::Handle;
        assert_eq!(
            install_hid_driver_binding(boot_services, mock_image_handle),
            Err(efi::Status::OUT_OF_RESOURCES.into())
        );

        let boot_services = Box::leak(Box::new(MockBootServices::new()));

        boot_services
            .expect_install_protocol_interface::<UefiHidMarker, Box<UefiHidMarker>>()
            .returning(|_, protocol_interface| Ok((0x5678 as efi::Handle, protocol_interface.metadata())));

        boot_services.expect_install_protocol_interface_unchecked().returning(|handle, protocol, interface| {
            if protocol == &efi::protocols::driver_binding::PROTOCOL_GUID {
                assert!(
                    handle.is_some_and(|handle| handle as usize == 0x5678),
                    "Expected correct handle for driver binding protocol"
                );
                assert!(!interface.is_null(), "Expected non-null interface for driver binding protocol");
                return Err(efi::Status::OUT_OF_RESOURCES);
            }
            panic!("Unexpected protocol installation: {:?}", protocol);
        });

        assert_eq!(
            install_hid_driver_binding(boot_services, mock_image_handle),
            Err(efi::Status::OUT_OF_RESOURCES.into())
        );
    }
}
