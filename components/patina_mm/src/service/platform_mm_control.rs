//! Platform Management Mode (MM) Service Trait
//!
//! An optional service that may be installed by a platform to initialize the MM environment prior to software
//! MMIs being enabled.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// Platform Management Mode (MM) Control Service
///
/// A platform may optionally produce this service if it needs to perform any platform-specific initialization of the
/// MM environment.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait PlatformMmControl {
    /// Platform-specific initialization of the MM environment.
    fn init(&self) -> patina_sdk::error::Result<()>;
}
