//! Driver Binding Producer Sample Component
//!
//! Demonstrates managing controllers through the UEFI Driver Model with a [`ControllerDriver`].
//!
//! The driver manages each controller that has [`SampleVendorProtocol`]. `ConnectController()` calls its
//! `supported()` and `start()`, and a later `DisconnectController()` or uninstall of the protocol calls its `stop()`.
//! The SDK opens and closes the protocol and keeps each controller's state, so the driver only implements what
//! is specific to it.
//!
//! [`ControllerDriver`]: patina::component::service::uefi_services::controller_driver::ControllerDriver
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
                controller_driver::{ControllerDriver, StartContext, SupportedContext, install_controller_driver},
                driver::DriverServices,
                protocol::{Handle, ProtocolError, ProtocolServices, ProtocolServicesExt},
            },
        },
    },
    error::Result,
};

use super::protocol_publisher::SampleVendorProtocol;

/// State the sample driver keeps for each controller it manages.
struct SampleState {
    last_status: u64,
    polls: u32,
}

/// Manages controllers that have [`SampleVendorProtocol`].
struct SampleDriver;

impl ControllerDriver for SampleDriver {
    type Protocol = SampleVendorProtocol;
    type State = SampleState;

    fn supported(&self, _context: &SupportedContext<'_, Self>) -> core::result::Result<(), ProtocolError> {
        Ok(())
    }

    fn start(&self, context: &StartContext<'_, Self>) -> core::result::Result<SampleState, ProtocolError> {
        let last_status = (context.protocol().get_status)();
        log::info!("SampleDriver: managing {:?}, status {last_status:#x}", context.controller());
        Ok(SampleState { last_status, polls: 0 })
    }

    fn stop(&self, controller: Handle, state: &mut SampleState) -> core::result::Result<(), ProtocolError> {
        log::info!("SampleDriver: releasing {controller:?} after {} polls", state.polls);
        Ok(())
    }
}

/// Installs `SampleDriver` and connects it to the controllers that have [`SampleVendorProtocol`].
#[derive(Default)]
pub struct DriverBindingProducerSample;

#[component]
impl DriverBindingProducerSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, protocols: Service<dyn ProtocolServices>, drivers: Service<dyn DriverServices>) -> Result<()> {
        let driver = install_controller_driver(protocols, SampleDriver)?;

        for controller in protocols.locate_handles_for::<SampleVendorProtocol>()? {
            if let Err(err) = drivers.connect_controller(controller, false) {
                log::debug!("SampleDriver: not connected to {controller:?}: {err:?}");
            }
        }

        // State can be read and modified outside start() and stop(), for example from an event callback.
        for controller in driver.controllers() {
            driver.with_controller(controller, |state, protocol| {
                state.last_status = (protocol.get_status)();
                state.polls += 1;
            })?;
        }

        Ok(())
    }
}
