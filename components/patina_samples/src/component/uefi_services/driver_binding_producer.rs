//! Driver Binding Producer Sample Component
//!
//! Demonstrates producing a `EFI_DRIVER_BINDING_PROTOCOL` from a component, using [`install_driver_binding`].
//!
//! [`install_driver_binding`]: patina::component::service::uefi_services::protocol::ProtocolServicesExt::install_driver_binding
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{
    component::{
        component,
        service::{
            Service,
            uefi_services::{
                driver_binding::DriverBinding,
                protocol::{Handle, OpenAttributes, ProtocolError, ProtocolServices, ProtocolServicesExt},
            },
        },
    },
    error::Result,
    protocol::ProtocolInterface,
};

use super::protocol_publisher::SampleVendorProtocol;

/// A minimal driver binding that releases whatever it opened when asked to stop.
///
/// `supported` and `start` use the default, permissive implementation. This sample only cares
/// about being a well-behaved `Stop()` target for the `ByDriver` usage it holds, not about
/// actually being connected to real controllers through `ConnectController()`.
struct SampleDriver {
    protocols: Service<dyn ProtocolServices>,
}

impl DriverBinding for SampleDriver {
    fn stop(&self, agent: Handle, controller: Handle, children: &[Handle]) -> core::result::Result<(), ProtocolError> {
        log::info!("SampleDriver: releasing {controller:?} ({} children)", children.len());
        // A real Stop() must close whatever it opened itself, using its own agent handle. Returning
        // Ok(()) alone does not release a ByDriver usage.
        self.protocols.close_interface(controller, SampleVendorProtocol::PROTOCOL_GUID, agent, Some(controller))
    }
}

/// Installs a driver binding protocol, then uses it to open a protocol `ByDriver` so the usage can
/// later be released or preempted through the driver binding's `Stop()`.
#[derive(Default)]
pub struct DriverBindingProducerSample;

#[component]
impl DriverBindingProducerSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, protocols: Service<dyn ProtocolServices>) -> Result<()> {
        // Installing the driver binding creates and returns this component's agent handle. The same
        // handle is reused below as the agent for the ByDriver open. The binding keeps its own copy
        // of protocols so stop() can close what it opened.
        let agent = protocols.install_driver_binding(SampleDriver { protocols })?;

        if let Ok(handle) = protocols.locate_first_handle::<SampleVendorProtocol>() {
            // ByDriver records that `agent` manages `handle`. DisconnectController()
            // (or an uninstall of SampleVendorProtocol) can call SampleDriver::stop and
            // release this usage.
            let guard = protocols.open_protocol::<SampleVendorProtocol>(
                handle,
                agent,
                OpenAttributes::ByDriver { controller: handle },
            )?;
            log::info!("ByDriver open() status is {:#x}", (guard.get_status)());
        }

        Ok(())
    }
}
