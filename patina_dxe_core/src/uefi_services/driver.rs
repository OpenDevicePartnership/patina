//! DXE Core implementation of [`DriverServices`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::vec::Vec;
use core::ptr::NonNull;

use patina::component::service::{
    IntoService,
    uefi_services::driver::{DriverError, DriverServices, Handle},
};
use patina::standard::efi;

use crate::driver_services::{core_connect_controller, core_disconnect_controller};

/// Core implementation of [`DriverServices`], delegating to the core driver model by calling the internal
/// `core_*` Rust APIs.
#[derive(IntoService)]
#[service(dyn DriverServices)]
pub(crate) struct CoreDriverServices;

/// Checks that `bytes` holds a whole device path, ending in an end-of-entire-path node, within its bounds.
fn validate_device_path(bytes: &[u8]) -> Result<(), DriverError> {
    let mut offset = 0usize;
    loop {
        let header = bytes.get(offset..).and_then(|rest| rest.get(..4)).ok_or(DriverError::InvalidParameter)?;
        let &[node_type, sub_type, len_lo, len_hi] = header else {
            return Err(DriverError::InvalidParameter);
        };
        let length = usize::from(u16::from_le_bytes([len_lo, len_hi]));
        let end = offset.checked_add(length).ok_or(DriverError::InvalidParameter)?;
        if length < 4 || end > bytes.len() {
            return Err(DriverError::InvalidParameter);
        }
        if node_type == efi::protocols::device_path::TYPE_END
            && sub_type == efi::protocols::device_path::End::SUBTYPE_ENTIRE
        {
            return Ok(());
        }
        offset = end;
    }
}

impl DriverServices for CoreDriverServices {
    fn connect_controller_with(
        &self,
        controller: Handle,
        driver_images: &[Handle],
        remaining_device_path: Option<&[u8]>,
        recursive: bool,
    ) -> Result<(), DriverError> {
        let device_path = match remaining_device_path {
            Some(bytes) => {
                validate_device_path(bytes)?;
                NonNull::new(bytes.as_ptr().cast::<efi::protocols::device_path::Protocol>().cast_mut())
            }
            None => None,
        };
        let driver_handles: Vec<efi::Handle> = driver_images.iter().map(Handle::as_raw).collect();
        // SAFETY: `device_path` is either `None` or points to a device path validated above to be well-formed and
        // terminated within `bytes`, which outlives this call. Handles are validated inside the core routine.
        unsafe { core_connect_controller(controller.as_raw(), driver_handles, device_path, recursive) }
            .map_err(DriverError::from)
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

    #[test]
    fn test_validate_device_path() {
        assert_eq!(validate_device_path(&[0x7f, 0xff, 4, 0]), Ok(()));
        assert_eq!(validate_device_path(&patina::devpath!("PciRoot(0)/Pci(0x11,0)")), Ok(()));
        assert_eq!(validate_device_path(&[]), Err(DriverError::InvalidParameter));
        // Node length past the end of the buffer.
        assert_eq!(validate_device_path(&[0x01, 0x01, 8, 0, 0x7f, 0xff]), Err(DriverError::InvalidParameter));
        // Node length below the header size.
        assert_eq!(validate_device_path(&[0x01, 0x01, 2, 0, 0x7f, 0xff, 4, 0]), Err(DriverError::InvalidParameter));
        // End-of-instance node does not terminate the path.
        assert_eq!(validate_device_path(&[0x7f, 0x01, 4, 0]), Err(DriverError::InvalidParameter));
    }

    #[test]
    fn test_connect_controller_with_rejects_malformed_device_path() {
        with_locked_state(|| {
            let (raw_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x3333usize as *mut core::ffi::c_void,
                )
                .unwrap();
            let handle = Handle::from_raw(raw_handle).unwrap();

            let result = CoreDriverServices.connect_controller_with(handle, &[], Some(&[0x7f, 0xff]), false);

            assert_eq!(result, Err(DriverError::InvalidParameter));
        });
    }
}
