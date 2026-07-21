//! Image services for Patina components.
//!
//! [`ImageServices`] exposes the UEFI image services such as loading, starting, and unloading UEFI
//! images. Images are referred to by the opaque [`Handle`] token, and image contents are supplied as
//! a byte slice rather than a raw pointer.
//!
//! Images can be loaded either from an in-memory buffer ([`ImageServices::load_image`]) or, when the
//! `unstable-device-path` feature is enabled, by device path.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::base::error::EfiError;
#[cfg(feature = "unstable-device-path")]
use crate::uefi::device_path::paths::DevicePath;

pub use super::handle::Handle;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// Errors that can occur when using [`ImageServices`].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ImageError {
    /// A provided handle or parameter was invalid.
    InvalidParameter,
    /// The image (or a resource it requires) was not found.
    NotFound,
    /// The image failed to load.
    LoadError,
    /// The image loaded but failed authentication.
    SecurityViolation,
    /// The image was not loaded or started due to platform policy.
    AccessDenied,
    /// The operation is not supported.
    Unsupported,
    /// An unexpected internal error occurred.
    Internal,
}

impl From<ImageError> for EfiError {
    fn from(value: ImageError) -> Self {
        match value {
            ImageError::InvalidParameter => EfiError::InvalidParameter,
            ImageError::NotFound => EfiError::NotFound,
            ImageError::LoadError => EfiError::LoadError,
            ImageError::SecurityViolation => EfiError::SecurityViolation,
            ImageError::AccessDenied => EfiError::AccessDenied,
            ImageError::Unsupported => EfiError::Unsupported,
            ImageError::Internal => EfiError::DeviceError,
        }
    }
}

impl From<EfiError> for ImageError {
    fn from(value: EfiError) -> Self {
        match value {
            EfiError::InvalidParameter => ImageError::InvalidParameter,
            EfiError::NotFound => ImageError::NotFound,
            EfiError::LoadError => ImageError::LoadError,
            EfiError::SecurityViolation => ImageError::SecurityViolation,
            EfiError::AccessDenied => ImageError::AccessDenied,
            EfiError::Unsupported => ImageError::Unsupported,
            _ => ImageError::Internal,
        }
    }
}

/// UEFI image services: load, start, and unload images.
///
/// This service is implemented by the Patina DXE Core. Components consume it by adding a
/// [`Service<dyn ImageServices>`](crate::component::service::Service) parameter to their entry
/// point.
///
/// # Examples
///
/// ```rust,no_run
/// use patina::component::service::{Service, uefi_services::image::{ImageServices, Handle}};
/// use patina::error::Result;
///
/// fn entry_point(images: Service<dyn ImageServices>, parent: Handle, pe_image: &[u8]) -> Result<()> {
///     let image = images.load_image(parent, pe_image)?;
///     images.start_image(image)?;
///     Ok(())
/// }
/// ```
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait ImageServices {
    /// Loads a UEFI image from an in-memory buffer.
    ///
    /// `parent` must be a valid image handle (for example, an image handle the caller already
    /// holds). The loaded image's handle is returned. It can then be started with
    /// [`Self::start_image`].
    ///
    /// This is a simpler alternative to `load_image_from_device_path` when the image is
    /// already available in memory that sets `boot_policy` to `false`. If a device path needs
    /// to be specified or `boot_policy` needs to be `true`, use `load_image_from_device_path`
    /// instead. `load_image_from_device_path` is only available when the `unstable-device-path`
    /// feature is enabled.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::LoadError`] if the image could not be loaded,
    /// [`ImageError::SecurityViolation`] if it failed authentication, or
    /// [`ImageError::AccessDenied`] if platform policy prevented loading.
    fn load_image(&self, parent: Handle, source: &[u8]) -> Result<Handle, ImageError>;

    /// Starts a previously loaded image, transferring control to its entry point.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::InvalidParameter`] if `image` is not a loaded, not started image.
    fn start_image(&self, image: Handle) -> Result<(), ImageError>;

    /// Unloads a previously loaded image.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::InvalidParameter`] if `image` is not a valid image handle.
    fn unload_image(&self, image: Handle) -> Result<(), ImageError>;

    /// Loads a UEFI image located by a device path (for example, a file on a file system or a
    /// `LoadFile`/`LoadFile2` provider).
    ///
    /// `parent` must be a valid image handle. When `boot_policy` is `true`, the request is treated
    /// as originating from the boot manager (matching the UEFI `LoadImage` `BootPolicy` parameter);
    /// most callers pass `false`.
    ///
    /// This method is available only when the `unstable-device-path` feature is enabled, as it
    /// depends on the SDK's unstable device path API.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::NotFound`] if no provider could produce the image for the given device
    /// path, or the other [`ImageError`] variants as for [`Self::load_image`].
    #[cfg(feature = "unstable-device-path")]
    fn load_image_from_device_path(
        &self,
        parent: Handle,
        device_path: &DevicePath,
        boot_policy: bool,
    ) -> Result<Handle, ImageError>;
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
    fn test_image_services_error_to_efi() {
        assert_eq!(EfiError::from(ImageError::LoadError), EfiError::LoadError);
        assert_eq!(EfiError::from(ImageError::SecurityViolation), EfiError::SecurityViolation);
        assert_eq!(EfiError::from(ImageError::AccessDenied), EfiError::AccessDenied);
        assert_eq!(EfiError::from(ImageError::Internal), EfiError::DeviceError);
    }

    #[test]
    fn test_image_services_error_from_efi() {
        assert_eq!(ImageError::from(EfiError::LoadError), ImageError::LoadError);
        assert_eq!(ImageError::from(EfiError::SecurityViolation), ImageError::SecurityViolation);
        assert_eq!(ImageError::from(EfiError::NotFound), ImageError::NotFound);
        assert_eq!(ImageError::from(EfiError::DeviceError), ImageError::Internal);
    }

    #[test]
    fn test_image_services_mock_flow() {
        let mut mock = MockImageServices::new();
        mock.expect_load_image().times(1).returning(|_, source| {
            assert_eq!(source, b"pe");
            Ok(Handle::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap())
        });
        mock.expect_start_image().times(1).returning(|_| Ok(()));
        mock.expect_unload_image().times(1).returning(|_| Err(ImageError::InvalidParameter));

        let parent = dummy_handle();
        let image = mock.load_image(parent, b"pe").unwrap();
        assert!(mock.start_image(image).is_ok());
        assert_eq!(mock.unload_image(image), Err(ImageError::InvalidParameter));
    }

    #[cfg(feature = "unstable-device-path")]
    #[test]
    fn test_image_services_mock_load_from_device_path() {
        use crate::uefi::device_path::paths::DevicePath;

        // Minimal valid device path: a single End-of-Entire node (type 0x7F, sub-type 0xFF, len 4).
        let bytes = [0x7Fu8, 0xFF, 0x04, 0x00];
        // SAFETY: `bytes` is a valid, well-formed device path (a single End-of-Entire node) that
        // outlives the returned reference.
        let device_path = unsafe { DevicePath::try_from_ptr(bytes.as_ptr()) }.unwrap();

        let mut mock = MockImageServices::new();
        mock.expect_load_image_from_device_path().times(1).returning(|_, _, boot_policy| {
            assert!(!boot_policy);
            Ok(Handle::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap())
        });

        assert!(mock.load_image_from_device_path(dummy_handle(), device_path, false).is_ok());
    }
}
