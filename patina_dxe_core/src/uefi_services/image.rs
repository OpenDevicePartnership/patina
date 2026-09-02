//! DXE Core implementation of [`ImageServices`].
//!
//! The UEFI image services live on the platform-generic `PiDispatcher<P>`. To register a single
//! non-generic service, [`CoreImageServices::new`] captures the dispatcher's platform-erased
//! operations as function pointers when the platform type `P` is known.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::component::service::{
    IntoService,
    uefi_services::image::{Handle, ImageError, ImageServices},
};
use patina::standard::efi;
#[cfg(feature = "unstable-device-path")]
use patina::uefi::device_path::paths::DevicePath;

#[cfg(feature = "unstable-device-path")]
use core::ptr::NonNull;

use crate::PlatformInfo;
use crate::pi_dispatcher::PiDispatcher;

/// Core implementation of [`ImageServices`].
///
/// Holds the dispatcher's image operations as platform-erased function pointers so the service can
/// be registered without carrying the platform generic `P`.
#[derive(IntoService)]
#[service(dyn ImageServices)]
pub(crate) struct CoreImageServices {
    load: fn(efi::Handle, &[u8]) -> Result<efi::Handle, ImageError>,
    start: fn(efi::Handle) -> Result<(), ImageError>,
    unload: fn(efi::Handle) -> Result<(), ImageError>,
    #[cfg(feature = "unstable-device-path")]
    load_from_device_path:
        fn(efi::Handle, NonNull<efi::protocols::device_path::Protocol>, bool) -> Result<efi::Handle, ImageError>,
}

impl CoreImageServices {
    /// Creates the service, binding the platform-specific dispatcher operations for platform `P`.
    pub(crate) fn new<P: PlatformInfo>() -> Self {
        Self {
            load: PiDispatcher::<P>::service_load_image,
            start: PiDispatcher::<P>::service_start_image,
            unload: PiDispatcher::<P>::service_unload_image,
            #[cfg(feature = "unstable-device-path")]
            load_from_device_path: PiDispatcher::<P>::service_load_image_from_device_path,
        }
    }
}

impl ImageServices for CoreImageServices {
    fn load_image(&self, parent: Handle, source: &[u8]) -> Result<Handle, ImageError> {
        let handle = (self.load)(parent.as_raw(), source)?;
        Handle::from_raw(handle).ok_or(ImageError::Internal)
    }

    fn start_image(&self, image: Handle) -> Result<(), ImageError> {
        (self.start)(image.as_raw())
    }

    fn unload_image(&self, image: Handle) -> Result<(), ImageError> {
        (self.unload)(image.as_raw())
    }

    #[cfg(feature = "unstable-device-path")]
    fn load_image_from_device_path(
        &self,
        parent: Handle,
        device_path: &DevicePath,
        boot_policy: bool,
    ) -> Result<Handle, ImageError> {
        // SAFETY-adjacent: `DevicePath` is a validated, well-formed device path (repr(transparent)
        // over its byte buffer), so the first byte is the first device path node header.
        let ptr = NonNull::new(device_path.as_bytes().as_ptr() as *mut efi::protocols::device_path::Protocol)
            .ok_or(ImageError::InvalidParameter)?;
        let handle = (self.load_from_device_path)(parent.as_raw(), ptr, boot_policy)?;
        Handle::from_raw(handle).ok_or(ImageError::Internal)
    }
}
