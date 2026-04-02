//! Connect Controller service interface.
//!
//! Defines the [`ConnectController`] trait for pluggable controller connection
//! strategies during device enumeration.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{boot_services::StandardBootServices, error::Result};

/// Service interface for controller connection strategy.
///
/// Implementations define *which* controllers to connect during device
/// enumeration. The default implementation ([`ConnectAllStrategy`](crate::ConnectAllStrategy))
/// connects all controllers recursively, preserving the previous `connect_all()` behavior.
///
/// Platforms provide custom implementations to control enumeration scope,
/// for example connecting only PCI controllers or skipping USB.
///
/// # Example
///
/// ```rust,ignore
/// use patina_boot::connect_controller::ConnectController;
/// use patina::boot_services::StandardBootServices;
///
/// struct PciOnlyStrategy;
///
/// impl ConnectController for PciOnlyStrategy {
///     fn connect(&self, boot_services: &StandardBootServices) -> patina::error::Result<()> {
///         // Connect only PCI root bridge handles
///         todo!()
///     }
/// }
/// ```
pub trait ConnectController: Send + Sync + 'static {
    /// Connect controllers according to this strategy.
    ///
    /// Called by [`interleave_connect_and_dispatch()`](crate::helpers::interleave_connect_and_dispatch)
    /// in place of `connect_all()`. Each invocation should perform one connection
    /// pass. The caller handles looping and interleaving with DXE dispatch.
    fn connect(&self, boot_services: &StandardBootServices) -> Result<()>;
}
