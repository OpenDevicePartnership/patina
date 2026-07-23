//! DXE Core implementation of [`DriverServices`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::vec::Vec;

use patina::component::service::{
    IntoService,
    uefi_services::driver::{DriverError, DriverServices, Handle},
};

use crate::driver_services::{core_connect_controller, core_disconnect_controller};

/// Core implementation of [`DriverServices`], delegating to the core driver model by calling the internal
/// `core_*` Rust APIs.
#[derive(IntoService)]
#[service(dyn DriverServices)]
pub(crate) struct CoreDriverServices;

impl DriverServices for CoreDriverServices {
    fn connect_controller(&self, controller: Handle, recursive: bool) -> Result<(), DriverError> {
        // SAFETY: No remaining device path is passed, so there is no device-path pointer whose
        // validity the caller must uphold. Handles are validated inside the core routine.
        unsafe { core_connect_controller(controller.as_raw(), Vec::new(), None, recursive) }.map_err(DriverError::from)
    }

    fn disconnect_controller(
        &self,
        controller: Handle,
        driver: Option<Handle>,
        child: Option<Handle>,
    ) -> Result<(), DriverError> {
        // SAFETY: All handles are validated inside `core_disconnect_controller`.
        unsafe {
            core_disconnect_controller(controller.as_raw(), driver.map(|h| h.as_raw()), child.map(|h| h.as_raw()))
        }
        .map_err(DriverError::from)
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::{protocols::PROTOCOL_DB, test_support};
    use patina::standard::efi;

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            test_support::init_test_logger();
            // SAFETY: Called within the global test lock.
            unsafe {
                test_support::init_test_protocol_db();
            }
            f();
        })
        .unwrap();
    }

    #[test]
    fn test_connect_controller_no_driver_binding_returns_not_found() {
        with_locked_state(|| {
            let (raw_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x1111usize as *mut core::ffi::c_void,
                )
                .unwrap();
            let handle = Handle::from_raw(raw_handle).unwrap();

            let result = CoreDriverServices.connect_controller(handle, false);

            assert_eq!(result, Err(DriverError::NotFound));
        });
    }

    #[test]
    fn test_disconnect_controller_with_no_managing_driver_is_a_no_op() {
        with_locked_state(|| {
            let (raw_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x2222usize as *mut core::ffi::c_void,
                )
                .unwrap();
            let handle = Handle::from_raw(raw_handle).unwrap();

            // core_disconnect_controller treats "no drivers currently managing the controller" as
            // success, so this shows that the driver/child Option<Handle> -> Option<raw> translation
            // compiles and reaches that path rather than an error.
            let result = CoreDriverServices.disconnect_controller(handle, None, None);

            assert_eq!(result, Ok(()));
        });
    }

    #[test]
    fn test_connect_controller_invalid_handle_returns_invalid_parameter() {
        with_locked_state(|| {
            let handle = Handle::from_raw(0x9999 as efi::Handle).unwrap();

            let result = CoreDriverServices.connect_controller(handle, false);

            assert_eq!(result, Err(DriverError::InvalidParameter));
        });
    }

    #[test]
    fn test_disconnect_controller_invalid_handle_returns_invalid_parameter() {
        with_locked_state(|| {
            let handle = Handle::from_raw(0x9999 as efi::Handle).unwrap();

            let result = CoreDriverServices.disconnect_controller(handle, None, None);

            assert_eq!(result, Err(DriverError::InvalidParameter));
        });
    }
}
