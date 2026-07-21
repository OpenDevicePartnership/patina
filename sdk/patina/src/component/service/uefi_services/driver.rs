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
            DriverError::Internal => EfiError::DeviceError,
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
    fn connect_controller(&self, controller: Handle, recursive: bool) -> Result<(), DriverError>;

    /// Disconnects drivers from a controller handle.
    ///
    /// If `driver` is `Some`, only that driver is disconnected; otherwise all drivers are. If
    /// `child` is `Some`, only that child is destroyed; otherwise all children are.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::InvalidParameter`] if any handle is invalid, or
    /// [`DriverError::NotFound`] if the driver is not managing the controller.
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
        assert_eq!(DriverError::from(EfiError::AccessDenied), DriverError::AccessDenied);
        assert_eq!(DriverError::from(EfiError::OutOfResources), DriverError::Internal);
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
}
