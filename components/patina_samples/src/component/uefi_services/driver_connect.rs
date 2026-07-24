//! Driver Connect Sample Component
//!
//! This component demonstrates combining [`ProtocolServices`] with [`DriverServices`]. It connects
//! a driver to each Block I/O controller as the protocol is installed, rather than waiting for a
//! fixed point in boot. Connecting drivers is commonly used to bring up a device stack (for example,
//! binding a disk driver onto every Block I/O controller).
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::standard::efi::protocols::block_io::Protocol as BlockIo;
use patina::{
    component::{
        component,
        service::{
            Service,
            uefi_services::{
                driver::DriverServices,
                protocol::{ProtocolServices, ProtocolServicesExt, Tpl},
            },
        },
    },
    error::Result,
};

/// Connects a driver to each Block I/O controller as it is installed.
#[derive(Default)]
pub struct DriverConnectSample;

#[component]
impl DriverConnectSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, protocols: Service<dyn ProtocolServices>, drivers: Service<dyn DriverServices>) -> Result<()> {
        // Runs for every handle that already exposes Block I/O and for every future install.
        protocols.on_protocol_installed::<BlockIo>(Tpl::Callback, move |controller| {
            log::info!("Block I/O controller installed: {controller:?}");

            if let Ok(controllers) = protocols.locate_handles_for::<BlockIo>() {
                log::info!("There are currently {} Block I/O controllers installed", controllers.len());
            }

            // `recursive = true` also connects any child controllers the driver produces, bringing
            // up the device tree. A failure to connect one controller should not stop future
            // notifications, so log and continue.
            log::info!("Calling connect_controller for {controller:?}");
            if let Err(err) = drivers.connect_controller(controller, true) {
                log::warn!("Failed to connect {controller:?}: {err:?}");
            }
        })?;

        Ok(())
    }
}
