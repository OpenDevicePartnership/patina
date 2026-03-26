//! Boot dispatcher component.
//!
//! [`BootDispatcher`] is the Patina component that installs the BDS architectural
//! protocol and delegates to a [`BootOrchestrator`]
//! implementation when invoked by the DXE core.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use alloc::boxed::Box;
use core::ffi::c_void;

use patina::{
    boot_services::{BootServices, StandardBootServices},
    component::{
        component,
        params::Handle,
        service::{Service, dxe_dispatch::DxeDispatch},
    },
    error::{EfiError, Result},
    pi::protocols::bds,
    runtime_services::StandardRuntimeServices,
};
use spin::Once;

use crate::boot_orchestrator::BootOrchestrator;

/// Context stored in a static for the BDS protocol callback to access.
struct BdsContext {
    orchestrator: Box<dyn BootOrchestrator>,
    dxe_dispatch: &'static dyn DxeDispatch,
    boot_services: StandardBootServices,
    runtime_services: StandardRuntimeServices,
    image_handle: r_efi::efi::Handle,
}

// SAFETY: BdsContext is only accessed from the BDS entry point which runs on the
// BSP (Bootstrap Processor) at TPL_APPLICATION. UEFI is single-threaded at this point.
unsafe impl Send for BdsContext {}
// SAFETY: BdsContext is stored in a spin::Once and only read after initialization.
// Access is single-threaded (BSP at TPL_APPLICATION during BDS phase).
unsafe impl Sync for BdsContext {}

/// Static storage for the BDS context. Initialized once during component dispatch,
/// consumed once when the DXE core invokes the BDS protocol.
static BDS_CONTEXT: Once<BdsContext> = Once::new();

/// Boot dispatcher component.
///
/// This is the single Patina component for driving boot orchestration. It:
/// - Accepts a [`BootOrchestrator`] implementation via [`BootDispatcher::new()`]
/// - Installs the BDS architectural protocol during component dispatch
/// - Consumes the [`DxeDispatch`] service via dependency injection
/// - When the DXE core invokes BDS: delegates to `orchestrator.execute()`
///
/// ## Usage
///
/// ```rust,ignore
/// use patina_boot::{BootDispatcher, SimpleBootManager, config::BootConfig};
///
/// // Minimal boot:
/// add.component(BootDispatcher::new(SimpleBootManager::new(
///     BootConfig::new(nvme_esp_path())
///         .with_device(nvme_recovery_path()),
/// )));
///
/// // Custom orchestrator:
/// add.component(BootDispatcher::new(MyCustomOrchestrator::new()));
/// ```
pub struct BootDispatcher {
    orchestrator: Box<dyn BootOrchestrator>,
}

impl BootDispatcher {
    /// Create a new `BootDispatcher` with the given orchestrator.
    ///
    /// The orchestrator is boxed internally — callers pass any type that
    /// implements [`BootOrchestrator`].
    pub fn new(orchestrator: impl BootOrchestrator) -> Self {
        Self { orchestrator: Box::new(orchestrator) }
    }
}

#[component]
impl BootDispatcher {
    /// Entry point: stores context and installs the BDS architectural protocol.
    ///
    /// The actual boot flow does not execute here. It executes later when the
    /// DXE core calls `bds_entry_point` after all architectural protocols are
    /// satisfied and all DXE drivers have been dispatched.
    #[coverage(off)] // Component integration — tested via integration tests
    fn entry_point(
        self,
        boot_services: StandardBootServices,
        runtime_services: StandardRuntimeServices,
        dxe_dispatch: Service<dyn DxeDispatch>,
        image_handle: Option<Handle>,
    ) -> Result<()> {
        let handle = image_handle.ok_or_else(|| {
            log::error!("Handle not provided — required for LoadImage parent handle");
            EfiError::InvalidParameter
        })?;

        // Store the orchestrator and services for the BDS callback
        BDS_CONTEXT.call_once(|| BdsContext {
            orchestrator: self.orchestrator,
            dxe_dispatch: *dxe_dispatch,
            boot_services: boot_services.clone(),
            runtime_services: runtime_services.clone(),
            image_handle: *handle,
        });

        // Install the BDS architectural protocol
        let protocol = Box::leak(Box::new(bds::Protocol { entry: bds_entry_point }));

        // SAFETY: protocol is a valid, leaked BDS protocol struct with a valid entry function pointer.
        // Using unchecked variant because bds::Protocol does not implement ProtocolInterface.
        unsafe {
            boot_services.as_ref().install_protocol_interface_unchecked(
                None,
                &bds::PROTOCOL_GUID,
                protocol as *mut _ as *mut c_void,
            )
        }
        .map_err(EfiError::from)?;

        Ok(())
    }
}

/// BDS architectural protocol entry point.
///
/// Called by the DXE core after all architectural protocols are installed and
/// all DXE drivers have been dispatched. Retrieves the stored context and
/// delegates to the orchestrator.
#[coverage(off)] // Extern "efiapi" callback — tested via integration tests
extern "efiapi" fn bds_entry_point(_this: *mut bds::Protocol) {
    let Some(context) = BDS_CONTEXT.get() else {
        panic!("BDS context not initialized — BootDispatcher entry_point was not called");
    };

    let Err(e) = context.orchestrator.execute(
        &context.boot_services,
        &context.runtime_services,
        context.dxe_dispatch,
        context.image_handle,
    );
    panic!("BootOrchestrator::execute() failed: {e:?}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use patina::{
        boot_services::StandardBootServices, component::service::dxe_dispatch::DxeDispatch,
        runtime_services::StandardRuntimeServices,
    };
    use r_efi::efi;

    struct MockOrchestrator;

    impl BootOrchestrator for MockOrchestrator {
        fn execute(
            &self,
            _boot_services: &StandardBootServices,
            _runtime_services: &StandardRuntimeServices,
            _dxe_dispatch: &dyn DxeDispatch,
            _image_handle: efi::Handle,
        ) -> core::result::Result<!, patina::error::EfiError> {
            Err(patina::error::EfiError::NotFound)
        }
    }

    #[test]
    fn test_new_boot_dispatcher() {
        let _dispatcher = BootDispatcher::new(MockOrchestrator);
    }
}
