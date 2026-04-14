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

use patina::{
    boot_services::{BootServices, StandardBootServices},
    error::Result,
};

/// Pluggable controller connection strategy for device enumeration.
///
/// The default [`ConnectAllStrategy`](crate::ConnectAllStrategy) connects all
/// controllers recursively. Platforms can implement this trait on a struct or
/// pass a closure directly (blanket impl for `Fn(&B) -> Result<()> + Send + Sync + 'static`):
///
/// ```rust,ignore
/// SimpleBootManager::with_connect_strategy(config, |bs: &StandardBootServices| {
///     connect_pci(bs)?;
///     connect_usb(bs)
/// });
/// ```
pub trait ConnectController<B: BootServices = StandardBootServices>: Send + Sync + 'static {
    /// Perform one connection pass. The caller handles looping and
    /// interleaving with DXE dispatch.
    fn connect(&self, boot_services: &B) -> Result<()>;
}

impl<B, F> ConnectController<B> for F
where
    B: BootServices,
    F: Fn(&B) -> Result<()> + Send + Sync + 'static,
{
    fn connect(&self, boot_services: &B) -> Result<()> {
        self(boot_services)
    }
}
