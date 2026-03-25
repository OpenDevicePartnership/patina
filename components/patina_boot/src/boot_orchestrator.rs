//! Boot orchestrator trait definition.
//!
//! Defines the [`BootOrchestrator`] trait that platforms implement to customize
//! boot behavior. The [`BootDispatcher`](crate::BootDispatcher) component holds
//! a `Box<dyn BootOrchestrator>` and delegates to it when the DXE core invokes
//! the BDS architectural protocol.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina::{
    boot_services::StandardBootServices, component::service::dxe_dispatch::DxeDispatch, error::EfiError,
    runtime_services::StandardRuntimeServices,
};
use r_efi::efi;

/// Trait for boot orchestration.
///
/// Platforms implement this trait to define custom boot flows. The implementation
/// is passed to [`BootDispatcher::new()`](crate::BootDispatcher::new) and invoked
/// when the DXE core calls the BDS architectural protocol entry point.
///
/// ## Built-in Implementation
///
/// [`SimpleBootManager`](crate::SimpleBootManager) provides a default implementation
/// for platforms with straightforward boot topologies (primary/secondary devices,
/// optional hotkey).
///
/// ## Custom Implementation
///
/// ```rust,ignore
/// use patina_boot::BootOrchestrator;
///
/// struct MyCustomBoot { /* ... */ }
///
/// impl BootOrchestrator for MyCustomBoot {
///     fn execute(
///         &self,
///         boot_services: &StandardBootServices,
///         runtime_services: &StandardRuntimeServices,
///         dxe_services: &dyn DxeDispatch,
///         image_handle: efi::Handle,
///     ) -> Result<!, EfiError> {
///         // Custom boot flow...
///         // Return Err if all boot options are exhausted
///         Err(EfiError::NotFound)
///     }
/// }
/// ```
pub trait BootOrchestrator: Send + Sync + 'static {
    /// Execute the boot flow.
    ///
    /// Called by [`BootDispatcher`](crate::BootDispatcher) when the DXE core invokes
    /// the BDS architectural protocol. This method should:
    ///
    /// 1. Enumerate devices (e.g., `connect_all()`)
    /// 2. Signal BDS phase events (EndOfDxe, ReadyToBoot)
    /// 3. Attempt to boot from configured device paths
    /// 4. Handle boot failures
    ///
    /// A successful boot transfers control to the boot image and never returns.
    /// If all boot options are exhausted, the implementation returns
    /// `Err(EfiError)`. The `Ok` variant is uninhabitable (`!`), enforcing at
    /// the type level that this method can only "succeed" by not returning.
    fn execute(
        &self,
        boot_services: &StandardBootServices,
        runtime_services: &StandardRuntimeServices,
        dxe_services: &dyn DxeDispatch,
        image_handle: efi::Handle,
    ) -> Result<!, EfiError>;
}
