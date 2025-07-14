//! This module provides way of implementing driver binding and installing and uninstalling the driver binding protocol.
//!
//! # Example
//!
//! ```rust, no_run
//! use core::{mem::MaybeUninit, ptr::NonNull};
//!
//! use r_efi::efi::{self, protocols::device_path::Protocol as EfiDevicePathProtocol};
//!
//! use patina_sdk::boot_services::{BootServices, StandardBootServices};
//! use patina_sdk::driver_binding::{DriverBinding, UefiDriverBinding};
//!
//! struct MockDriverBinding {/* ... */}
//!
//! impl DriverBinding for MockDriverBinding {
//!     fn driver_binding_supported<T: BootServices + 'static>(
//!         &self,
//!         boot_services: &'static T,
//!         controller: efi::Handle,
//!         remaining_device_path: Option<NonNull<EfiDevicePathProtocol>>,
//!     ) -> Result<bool, efi::Status> {
//!         // ...
//!         Ok(true)
//!     }
//!
//!     fn driver_binding_start<T: BootServices + 'static>(
//!         &self,
//!         boot_services: &'static T,
//!         controller: efi::Handle,
//!         remaining_device_path: Option<NonNull<EfiDevicePathProtocol>>,
//!     ) -> Result<(), efi::Status> {
//!         // ...
//!         Ok(())
//!     }
//!
//!     fn driver_binding_stop<T: BootServices + 'static>(
//!         &self,
//!         boot_services: &'static T,
//!         controller: efi::Handle,
//!         number_of_children: usize,
//!         child_handle_buffer: Option<NonNull<efi::Handle>>,
//!     ) -> Result<(), efi::Status> {
//!         // ...
//!         Ok(())
//!     }
//! }
//!
//! let handle = 0 as usize as efi::Handle;
//! static BOOT_SERVICES: StandardBootServices = StandardBootServices::new_uninit();
//!
//! let driver = MockDriverBinding {};
//! let mut uefi_driver_binding = UefiDriverBinding::new(driver, handle, &BOOT_SERVICES);
//! uefi_driver_binding.install().unwrap();
//!
//! ```

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

extern crate alloc;

use alloc::boxed::Box;
use core::{
    mem::{self, ManuallyDrop},
    ptr::NonNull,
};

use r_efi::{
    efi,
    protocols::{device_path::Protocol as EfiDevicePathProtocol, driver_binding::Protocol as EfiDriverBindingProtocol},
};

use crate::boot_services::{
    BootServices,
    c_ptr::{CPtr, PtrMetadata},
};

/// Driver binding protocol interface to enable mocking in tests.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait DriverBinding {
    /// Tests to see if this driver supports a given controller.
    /// If a child device is provided, it further tests to see if this driver supports creating a handle for the specified child device.
    fn driver_binding_supported<T: BootServices + 'static>(
        &self,
        boot_services: &'static T,
        controller: efi::Handle,
        remaining_device_path: Option<NonNull<EfiDevicePathProtocol>>,
    ) -> Result<bool, efi::Status>;

    /// Starts a device controller or a bus controller.
    fn driver_binding_start<T: BootServices + 'static>(
        &self,
        boot_services: &'static T,
        controller: efi::Handle,
        remaining_device_path: Option<NonNull<EfiDevicePathProtocol>>,
    ) -> Result<(), efi::Status>;

    /// Stops a device controller or a bus controller.
    fn driver_binding_stop<T: BootServices + 'static>(
        &self,
        boot_services: &'static T,
        controller: efi::Handle,
        number_of_children: usize,
        child_handle_buffer: Option<NonNull<efi::Handle>>,
    ) -> Result<(), efi::Status>;
}

/// Internal struct of [`UefiDriverBinding`]. this is used as protocol interface for the driver binding protocol.
#[repr(C)]
pub struct _UefiDriverBinding<T, U>
where
    T: DriverBinding + 'static,
    U: BootServices + 'static,
{
    // This field need to be first and the struct repr C to keep the backward compatibility with efi driver binding when installing the protocol.
    // Here we need to do this hack because we do not have control over the efi driver binding protocol because it is defined in the spec.
    driver_binding_protocol: EfiDriverBindingProtocol,
    driver_binding: T,
    boot_services: &'static U,
}

impl<T, U> _UefiDriverBinding<T, U>
where
    T: DriverBinding + 'static,
    U: BootServices + 'static,
{
    const fn new(
        image_handle: efi::Handle,
        driver_binding_handle: efi::Handle,
        driver_binding: T,
        boot_services: &'static U,
    ) -> Self {
        Self {
            driver_binding_protocol: EfiDriverBindingProtocol {
                supported: Self::efi_driver_binding_supported,
                start: Self::efi_driver_binding_start,
                stop: Self::efi_driver_binding_stop,
                version: 1,
                image_handle,
                driver_binding_handle,
            },
            driver_binding,
            boot_services,
        }
    }

    extern "efiapi" fn efi_driver_binding_supported(
        this: *mut EfiDriverBindingProtocol,
        controller_handle: efi::Handle,
        remaining_device_path: *mut EfiDevicePathProtocol,
    ) -> efi::Status {
        // SAFETY Self is passed as the interface when installed and this pointer does not change.
        let this = unsafe { (this as *mut _UefiDriverBinding<T, U>).as_ref() }.unwrap();

        match this.driver_binding.driver_binding_supported(
            this.boot_services,
            controller_handle,
            NonNull::new(remaining_device_path),
        ) {
            Ok(true) => efi::Status::SUCCESS,
            Ok(false) => efi::Status::UNSUPPORTED,
            Err(status) => status,
        }
    }

    extern "efiapi" fn efi_driver_binding_start(
        this: *mut EfiDriverBindingProtocol,
        controller_handle: efi::Handle,
        remaining_device_path: *mut EfiDevicePathProtocol,
    ) -> efi::Status {
        // SAFETY Self is passed as the interface when installed and this pointer does not change.
        let this = unsafe { (this as *mut _UefiDriverBinding<T, U>).as_ref() }.unwrap();
        match this.driver_binding.driver_binding_start(
            this.boot_services,
            controller_handle,
            NonNull::new(remaining_device_path),
        ) {
            Ok(()) => efi::Status::SUCCESS,
            Err(status) => status,
        }
    }

    extern "efiapi" fn efi_driver_binding_stop(
        this: *mut EfiDriverBindingProtocol,
        controller_handle: efi::Handle,
        number_of_children: usize,
        child_handle_buffer: *mut efi::Handle,
    ) -> efi::Status {
        // SAFETY Self is passed as the interface when installed and this pointer does not change.
        let this = unsafe { (this as *mut _UefiDriverBinding<T, U>).as_ref() }.unwrap();
        match this.driver_binding.driver_binding_stop(
            this.boot_services,
            controller_handle,
            number_of_children,
            NonNull::new(child_handle_buffer),
        ) {
            Ok(()) => efi::Status::SUCCESS,
            Err(status) => status,
        }
    }
}

/// This struct is used to install and uninstall driver binding.
/// If the UefiDriverBinding go out of scope and it wasn't install, the driver implementing [`DriverBinding`] will be drop.
/// If installed, the memory will be leaked and the driver binding will live indefinitely.
pub enum UefiDriverBinding<T, U>
where
    T: DriverBinding + 'static,
    U: BootServices + 'static,
{
    /// An owned, uninstalled driver binding.
    Uninstalled(Box<_UefiDriverBinding<T, U>>),
    /// A leaked, global, installed driver binding.
    Installed(PtrMetadata<'static, Box<_UefiDriverBinding<T, U>>>),
}

impl<T: DriverBinding + 'static, U: BootServices + 'static> UefiDriverBinding<T, U> {
    /// Create a new driver binding with image handle and driver binding handle set the the same value.
    pub fn new(driver_binding: T, handle: efi::Handle, boot_services: &'static U) -> Self {
        Self::new_with_driver_handle(driver_binding, handle, handle, boot_services)
    }

    /// Create a new driver binding with the option of choosing the value of image handle and driver binding handle.
    pub fn new_with_driver_handle(
        driver_binding: T,
        image_handle: efi::Handle,
        driver_binding_handle: efi::Handle,
        boot_services: &'static U,
    ) -> Self {
        Self::Uninstalled(Box::new(_UefiDriverBinding::new(
            image_handle,
            driver_binding_handle,
            driver_binding,
            boot_services,
        )))
    }

    /// Install the driver binding.
    pub fn install(&mut self) -> Result<(), efi::Status> {
        let Self::Uninstalled(uefi_driver_binding) = self else {
            // Already installed.
            return Ok(());
        };

        // SAFETY: This is safe because _UefiDriverBinding interface is compliant to the expected interface of driver binding guid.
        unsafe {
            uefi_driver_binding.boot_services.install_protocol_interface_unchecked(
                Some(uefi_driver_binding.driver_binding_protocol.driver_binding_handle),
                &efi::protocols::driver_binding::PROTOCOL_GUID,
                // Install the driver binding protocol interface as a _UefiDriverBinding.
                <Box<_> as CPtr>::as_ptr(uefi_driver_binding) as *mut _,
            )
        }?;

        let metadata = Box::metadata(uefi_driver_binding);
        match mem::replace(self, Self::Installed(metadata)) {
            UefiDriverBinding::Uninstalled(uefi_driver_binding) => _ = Box::leak(uefi_driver_binding),
            UefiDriverBinding::Installed(_) => (),
        }
        Ok(())
    }

    /// Uninstall the driver binding.
    pub fn uninstall(&mut self) -> Result<(), efi::Status> {
        let Self::Installed(ptr_metadata) = self else {
            // Already uninstalled.
            return Ok(());
        };

        // SAFETY: This is safe because the pointer behind this metada has been leak in install an is still valid.
        let uefi_driver_binding = ManuallyDrop::new(unsafe { PtrMetadata::clone(ptr_metadata).into_original_ptr() });

        // SAFETY: This is safe because _UefiDriverBinding interface is compliant to the expected interface of driver binding guid.
        unsafe {
            uefi_driver_binding.boot_services.uninstall_protocol_interface_unchecked(
                uefi_driver_binding.driver_binding_protocol.driver_binding_handle,
                &efi::protocols::driver_binding::PROTOCOL_GUID,
                uefi_driver_binding.as_ptr() as *mut _,
            )?;
        }

        *self = Self::Uninstalled(ManuallyDrop::into_inner(uefi_driver_binding));
        Ok(())
    }

    /// Returned weather or not the driver binding is installed.
    pub fn is_installed(&self) -> bool {
        match self {
            UefiDriverBinding::Uninstalled(_) => false,
            UefiDriverBinding::Installed(_) => true,
        }
    }
}

#[cfg(test)]
mod test {
    use core::{
        mem::MaybeUninit,
        ptr,
        sync::atomic::{AtomicBool, Ordering},
    };

    use crate::boot_services::MockBootServices;

    use super::*;

    #[test]
    fn test_install_driver_binding() {
        const TEST_HANDLE: efi::Handle = 12345_usize as efi::Handle;

        static mut BOOT_SERVICES_INIT: MaybeUninit<MockBootServices> = MaybeUninit::uninit();
        unsafe {
            let mut mock_boot_services = MockBootServices::new();
            mock_boot_services
                .expect_install_protocol_interface_unchecked()
                .once()
                .withf_st(|handle, protocol, interface| {
                    assert_eq!(&Some(TEST_HANDLE), handle);
                    assert_eq!(&efi::protocols::driver_binding::PROTOCOL_GUID, protocol);

                    let interface = (*interface as *const _UefiDriverBinding<MockDriverBinding, MockBootServices>)
                        .as_ref()
                        .unwrap();

                    assert_eq!(TEST_HANDLE, interface.driver_binding_protocol.image_handle);
                    assert_eq!(TEST_HANDLE, interface.driver_binding_protocol.driver_binding_handle);
                    assert_eq!(
                        BOOT_SERVICES as *const MockBootServices,
                        interface.boot_services as *const MockBootServices
                    );

                    true
                })
                .return_const_st(Ok(TEST_HANDLE));

            ptr::write(BOOT_SERVICES_INIT.as_mut_ptr(), mock_boot_services);
        }
        static BOOT_SERVICES: &MockBootServices = unsafe { BOOT_SERVICES_INIT.assume_init_ref() };

        let driver = MockDriverBinding::new();
        let mut uefi_driver_binding = UefiDriverBinding::new(driver, TEST_HANDLE, BOOT_SERVICES);
        assert!(!uefi_driver_binding.is_installed());
        uefi_driver_binding.install().unwrap();
        assert!(uefi_driver_binding.is_installed());
    }

    #[test]
    fn test_install_driver_binding_with_driver_handle() {
        const TEST_HANDLE: efi::Handle = 12345_usize as efi::Handle;
        const TEST_DRIVER_HANDLE: efi::Handle = 54321_usize as efi::Handle;

        static mut BOOT_SERVICES_INIT: MaybeUninit<MockBootServices> = MaybeUninit::uninit();
        unsafe {
            let mut mock_boot_services = MockBootServices::new();
            mock_boot_services
                .expect_install_protocol_interface_unchecked()
                .once()
                .withf_st(|handle, protocol, interface| {
                    assert_eq!(&Some(TEST_DRIVER_HANDLE), handle);
                    assert_eq!(&efi::protocols::driver_binding::PROTOCOL_GUID, protocol);

                    let interface = (*interface as *const _UefiDriverBinding<MockDriverBinding, MockBootServices>)
                        .as_ref()
                        .unwrap();

                    assert_eq!(TEST_HANDLE, interface.driver_binding_protocol.image_handle);
                    assert_eq!(TEST_DRIVER_HANDLE, interface.driver_binding_protocol.driver_binding_handle);
                    assert_eq!(
                        BOOT_SERVICES as *const MockBootServices,
                        interface.boot_services as *const MockBootServices
                    );

                    true
                })
                .return_const_st(Ok(TEST_DRIVER_HANDLE));

            ptr::write(BOOT_SERVICES_INIT.as_mut_ptr(), mock_boot_services);
        }
        static BOOT_SERVICES: &MockBootServices = unsafe { BOOT_SERVICES_INIT.assume_init_ref() };

        let driver = MockDriverBinding::new();
        let mut uefi_driver_binding: UefiDriverBinding<MockDriverBinding, MockBootServices> =
            UefiDriverBinding::new_with_driver_handle(driver, TEST_HANDLE, TEST_DRIVER_HANDLE, BOOT_SERVICES);
        assert!(!uefi_driver_binding.is_installed());
        uefi_driver_binding.install().unwrap();
        assert!(uefi_driver_binding.is_installed());
    }

    #[test]
    fn test_uninstall_driver_binding() {
        const TEST_HANDLE: efi::Handle = 12345_usize as efi::Handle;
        const TEST_DRIVER_HANDLE: efi::Handle = 54321_usize as efi::Handle;

        static mut BOOT_SERVICES_INIT: MaybeUninit<MockBootServices> = MaybeUninit::uninit();
        unsafe {
            let mut mock_boot_services = MockBootServices::new();
            mock_boot_services.expect_install_protocol_interface_unchecked().once().return_const_st(Ok(TEST_HANDLE));
            mock_boot_services
                .expect_uninstall_protocol_interface_unchecked()
                .once()
                .withf(|handle, protocol, interface| {
                    assert_eq!(&TEST_DRIVER_HANDLE, handle);
                    assert_eq!(&efi::protocols::driver_binding::PROTOCOL_GUID, protocol);

                    let interface = (*interface as *const _UefiDriverBinding<MockDriverBinding, MockBootServices>)
                        .as_ref()
                        .unwrap();

                    assert_eq!(TEST_HANDLE, interface.driver_binding_protocol.image_handle);
                    assert_eq!(TEST_DRIVER_HANDLE, interface.driver_binding_protocol.driver_binding_handle);
                    assert_eq!(
                        BOOT_SERVICES as *const MockBootServices,
                        interface.boot_services as *const MockBootServices
                    );

                    true
                })
                .return_const_st(Ok(()));

            ptr::write(BOOT_SERVICES_INIT.as_mut_ptr(), mock_boot_services);
        }
        static BOOT_SERVICES: &MockBootServices = unsafe { BOOT_SERVICES_INIT.assume_init_ref() };

        let driver = MockDriverBinding::new();
        let mut uefi_driver_binding: UefiDriverBinding<MockDriverBinding, MockBootServices> =
            UefiDriverBinding::new_with_driver_handle(driver, TEST_HANDLE, TEST_DRIVER_HANDLE, BOOT_SERVICES);
        uefi_driver_binding.install().unwrap();

        assert!(uefi_driver_binding.is_installed());
        uefi_driver_binding.uninstall().unwrap();
        assert!(!uefi_driver_binding.is_installed());
    }

    #[test]
    fn test_driver_binding_lifetime() {
        const TEST_HANDLE: efi::Handle = 12345_usize as efi::Handle;

        static mut BOOT_SERVICES_INIT: MaybeUninit<MockBootServices> = MaybeUninit::uninit();
        unsafe {
            let mut mock_boot_services = MockBootServices::new();
            mock_boot_services.expect_install_protocol_interface_unchecked().return_const_st(Ok(TEST_HANDLE));
            mock_boot_services.expect_uninstall_protocol_interface_unchecked().return_const_st(Ok(()));
            ptr::write(BOOT_SERVICES_INIT.as_mut_ptr(), mock_boot_services);
        }
        static BOOT_SERVICES: &MockBootServices = unsafe { BOOT_SERVICES_INIT.assume_init_ref() };

        struct MyDriverBinding;
        impl DriverBinding for MyDriverBinding {
            fn driver_binding_supported<T: BootServices + 'static>(
                &self,
                _boot_services: &'static T,
                _controller: efi::Handle,
                _remaining_device_path: Option<NonNull<EfiDevicePathProtocol>>,
            ) -> Result<bool, efi::Status> {
                Ok(true)
            }

            fn driver_binding_start<T: BootServices + 'static>(
                &self,
                _boot_services: &'static T,
                _controller: efi::Handle,
                _remaining_device_path: Option<NonNull<EfiDevicePathProtocol>>,
            ) -> Result<(), efi::Status> {
                Ok(())
            }

            fn driver_binding_stop<T: BootServices + 'static>(
                &self,
                _boot_services: &'static T,
                _controller: efi::Handle,
                _number_of_children: usize,
                _child_handle_buffer: Option<NonNull<efi::Handle>>,
            ) -> Result<(), efi::Status> {
                Ok(())
            }
        }

        impl Drop for MyDriverBinding {
            fn drop(&mut self) {
                MY_DRIVER_BINDING_DROPPED.store(true, Ordering::Relaxed);
            }
        }

        static MY_DRIVER_BINDING_DROPPED: AtomicBool = AtomicBool::new(false);

        {
            let mut uefi_driver_binding = UefiDriverBinding::new(MyDriverBinding, TEST_HANDLE, BOOT_SERVICES);
            uefi_driver_binding.install().unwrap();
        }

        assert!(!MY_DRIVER_BINDING_DROPPED.load(Ordering::Relaxed));

        {
            _ = UefiDriverBinding::new(MyDriverBinding, TEST_HANDLE, BOOT_SERVICES);
        }

        assert!(MY_DRIVER_BINDING_DROPPED.load(Ordering::Relaxed));
    }
}
