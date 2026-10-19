//! Driver connection services for Patina components.
//!
//! [`DriverServices`] exposes the UEFI driver model's connect and disconnect operations, allowing a
//! component to bind drivers to a controller handle or tear those bindings down.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::base::error::EfiError;

pub use super::handle::Handle;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// Errors that can occur when using [`DriverServices`].
#[non_exhaustive]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum DriverError {
    /// A provided handle or parameter was invalid.
    InvalidParameter,
    /// The requested handle, driver, or child was not found.
    NotFound,
    /// No drivers could be connected to the controller.
    Unsupported,
    /// Access to the controller or one of its protocols was denied.
    AccessDenied,
    /// There were not enough resources to complete the operation.
    OutOfResources,
    /// A driver could not stop the controller because of a device error.
    DeviceError,
    /// An unexpected internal error occurred.
    Internal,
}

impl From<DriverError> for EfiError {
    fn from(value: DriverError) -> Self {
        match value {
            DriverError::InvalidParameter => EfiError::InvalidParameter,
            DriverError::NotFound => EfiError::NotFound,
            DriverError::Unsupported => EfiError::Unsupported,
            DriverError::AccessDenied => EfiError::AccessDenied,
            DriverError::OutOfResources => EfiError::OutOfResources,
            DriverError::DeviceError | DriverError::Internal => EfiError::DeviceError,
        }
    }
}

impl From<EfiError> for DriverError {
    fn from(value: EfiError) -> Self {
        match value {
            EfiError::InvalidParameter => DriverError::InvalidParameter,
            EfiError::NotFound => DriverError::NotFound,
            EfiError::Unsupported => DriverError::Unsupported,
            EfiError::AccessDenied => DriverError::AccessDenied,
            EfiError::OutOfResources => DriverError::OutOfResources,
            EfiError::DeviceError => DriverError::DeviceError,
            _ => DriverError::Internal,
        }
    }
}

/// Driver connection and disconnection services.
///
/// This service is implemented by the Patina DXE Core. Components consume it by adding a
/// [`Service<dyn DriverServices>`](crate::component::service::Service) parameter to their entry
/// point.
///
/// # Examples
///
/// ```rust,no_run
/// use patina::component::service::{
///     Service,
///     uefi_services::{
///         driver::DriverServices,
///         protocol::{ProtocolServices, ProtocolServicesExt},
///     },
/// };
/// use patina::error::Result;
/// use patina::standard::efi::protocols::block_io::Protocol as BlockIo;
///
/// fn entry_point(
///     protocols: Service<dyn ProtocolServices>,
///     drivers: Service<dyn DriverServices>,
/// ) -> Result<()> {
///     // Bind drivers to every Block I/O controller, recursing into child controllers.
///     if let Ok(controllers) = protocols.locate_handles_for::<BlockIo>() {
///         for controller in controllers {
///             let _ = drivers.connect_controller(controller, true);
///         }
///     }
///     Ok(())
/// }
/// ```
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait DriverServices {
    /// Connects one or more drivers to a controller handle.
    ///
    /// The platform's driver binding protocols are used to select and start the best matching
    /// drivers. When `recursive` is `true`, the newly created child controllers are connected as
    /// well.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::NotFound`] or [`DriverError::Unsupported`] if no driver could be
    /// connected.
    fn connect_controller(&self, controller: Handle, recursive: bool) -> Result<(), DriverError> {
        self.connect_controller_with(controller, &[], None, recursive)
    }

    /// Connects drivers to a controller handle, with the full set of `ConnectController()` parameters.
    ///
    /// `driver_images` are the image handles of drivers to try first, in order, before the platform's other
    /// driver sources (the `DriverImageHandle` list). `remaining_device_path` is the device path, in its binary
    /// form, of the child controller to create (for example, from the `devpath!` macro). A path holding only an
    /// end node asks a bus driver to start the controller without creating children. `None` asks bus drivers to
    /// create all children.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::InvalidParameter`] if a handle is invalid or `remaining_device_path` is not a
    /// well-formed device path ending in an end-of-entire-path node.
    /// Returns [`DriverError::NotFound`] if no driver could be connected.
    // mockall needs the named lifetime for a reference nested in `Option`.
    #[allow(clippy::needless_lifetimes)]
    fn connect_controller_with<'a>(
        &self,
        controller: Handle,
        driver_images: &[Handle],
        remaining_device_path: Option<&'a [u8]>,
        recursive: bool,
    ) -> Result<(), DriverError>;

    /// Disconnects drivers from a controller handle.
    ///
    /// If `driver` is `Some`, only that driver is disconnected; otherwise all drivers are. If
    /// `child` is `Some`, only that child is destroyed; otherwise all children are.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::InvalidParameter`] if any handle is invalid, or
    /// [`DriverError::NotFound`] if the driver is not managing the controller.
    /// If no driver could be disconnected because a driver's `Stop()` failed, returns that error, for example
    /// [`DriverError::DeviceError`] or [`DriverError::OutOfResources`].
    fn disconnect_controller(
        &self,
        controller: Handle,
        driver: Option<Handle>,
        child: Option<Handle>,
    ) -> Result<(), DriverError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_void;
    use core::ptr::NonNull;

    fn dummy_handle() -> Handle {
        Handle::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()
    }

    #[test]
    fn test_driver_services_error_conversions() {
        assert_eq!(EfiError::from(DriverError::InvalidParameter), EfiError::InvalidParameter);
        assert_eq!(EfiError::from(DriverError::NotFound), EfiError::NotFound);
        assert_eq!(EfiError::from(DriverError::Unsupported), EfiError::Unsupported);
        assert_eq!(EfiError::from(DriverError::AccessDenied), EfiError::AccessDenied);
        assert_eq!(EfiError::from(DriverError::Internal), EfiError::DeviceError);
        assert_eq!(EfiError::from(DriverError::DeviceError), EfiError::DeviceError);
        assert_eq!(EfiError::from(DriverError::OutOfResources), EfiError::OutOfResources);
        assert_eq!(DriverError::from(EfiError::AccessDenied), DriverError::AccessDenied);
        assert_eq!(DriverError::from(EfiError::OutOfResources), DriverError::OutOfResources);
        assert_eq!(DriverError::from(EfiError::DeviceError), DriverError::DeviceError);
        assert_eq!(DriverError::from(EfiError::BadBufferSize), DriverError::Internal);
    }

    #[test]
    fn test_driver_services_mock_flow() {
        let mut mock = MockDriverServices::new();
        mock.expect_connect_controller().times(1).returning(|_, recursive| {
            assert!(recursive);
            Ok(())
        });
        mock.expect_disconnect_controller().times(1).returning(|_, driver, child| {
            assert!(driver.is_none());
            assert!(child.is_none());
            Err(DriverError::NotFound)
        });

        let handle = dummy_handle();
        assert!(mock.connect_controller(handle, true).is_ok());
        assert_eq!(mock.disconnect_controller(handle, None, None), Err(DriverError::NotFound));
    }

    #[test]
    fn test_driver_services_mock_connect_controller_with() {
        let mut mock = MockDriverServices::new();
        mock.expect_connect_controller_with().times(1).returning(|_, images, path, recursive| {
            assert_eq!(images.len(), 1);
            assert_eq!(path, Some(&[0x7f_u8, 0xff, 4, 0][..]));
            assert!(!recursive);
            Ok(())
        });

        let handle = dummy_handle();
        let end = [0x7f_u8, 0xff, 4, 0];
        assert!(mock.connect_controller_with(handle, &[handle], Some(&end), false).is_ok());
    }
}
