//! Shared opaque handle type for the Patina UEFI Services.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;
use core::ptr::NonNull;

use crate::standard::efi;

/// An opaque handle to an object in the UEFI handle database (a device, driver, or image).
///
/// A handle is a copyable token. Components obtain handles from services (for example
/// [`ProtocolServices`](super::protocol::ProtocolServices)) and pass them back to other service
/// methods. They never construct or dereference the underlying pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle(NonNull<c_void>);

impl Handle {
    /// Wraps a raw handle produced by the service implementation.
    ///
    /// This is intended for use by service implementations, not component authors.
    #[doc(hidden)]
    pub fn from_raw(handle: efi::Handle) -> Option<Self> {
        NonNull::new(handle).map(Self)
    }

    /// Returns the raw handle for use by the service implementation.
    ///
    /// This is intended for use by service implementations, not component authors.
    #[doc(hidden)]
    #[cfg_attr(coverage, coverage(off))]
    pub fn as_raw(&self) -> efi::Handle {
        self.0.as_ptr()
    }
}
