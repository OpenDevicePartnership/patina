//! Protocol Consumer Sample Component
//!
//! Depending on how a protocol needs to be used, there are different approaches to consuming it.
//! This sample shows the four access styles that [`ProtocolServicesExt`] provides and explains
//! when each is useful. It consumes the [`SampleVendorProtocol`] published by the protocol
//! publisher sample.
//!
//! The approaches below are sorted from shortest-lived to longest-lived. Try to use the shortest
//! lived approach
//!
//! 1. `with_protocol` runs a closure with the interface. Use it for a single, immediate use.
//! 2. `open_protocol` returns a guard that dereferences to the interface for a block. Use it when
//!    several statements in one scope need the interface.
//! 3. `locate_token` returns a token that stores only a handle. Use it to keep a reference for a
//!    longer period of time such as throughout boot. Call `resolve` to re-validate on each use.
//! 4. `on_protocol_installed` runs a callback for every present and future install. Use it when
//!    code needs to run when the protocol is published.
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
            uefi_services::protocol::{ProtocolServices, ProtocolServicesExt, Tpl},
        },
    },
    error::Result,
};

use super::protocol_publisher::SampleVendorProtocol;

/// Demonstrates the four ways to consume a protocol over its lifetime.
#[derive(Default)]
pub struct ProtocolConsumerSample;

#[component]
impl ProtocolConsumerSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, protocols: Service<dyn ProtocolServices>) -> Result<()> {
        // Option 1: with_protocol. Best for a single immediate use like calling a function on
        // the protocol. The closure borrows the interface and returns a plain value, so nothing
        // outlives the call.
        match protocols.with_protocol::<SampleVendorProtocol, _>(|protocol| (protocol.get_status)()) {
            Ok(status) => log::info!("with_protocol() read status is {status:#x}"),
            Err(_) => log::debug!("SampleVendorProtocol is not present"),
        }

        // Option 2 and 3 act on a specific handle, so first find one that has the protocol.
        if let Ok(handle) = protocols.locate_first_handle::<SampleVendorProtocol>() {
            // Option 2: open_protocol. Best when a block needs the interface across several
            // statements. The guard dereferences to the interface and releases access at the end
            // of the block.
            {
                let protocol = protocols.open_protocol::<SampleVendorProtocol>(handle)?;
                log::info!("open_protocol() revision is {:#x}", protocol.revision);
                log::info!("open_protocol() status is {:#x}", (protocol.get_status)());
            }

            // Option 3: locate_token then resolve. Best for using a protocol instance over time. The
            // token holds only a handle and doesn't dangle. resolve re-validates and returns None if the
            // interface has been uninstalled since the token was created. A real component could
            // store the token and resolve it as needed.
            let token = protocols.locate_token::<SampleVendorProtocol>()?;
            match protocols.resolve(&token) {
                Some(protocol) => log::info!("Protocol token resolved, status is {:#x}", (protocol.get_status)()),
                None => log::debug!("Token no longer valid"),
            }
        }

        // Option 4: on_protocol_installed. Best when another component publishes the protocol later.
        // The callback runs for handles already present and for every future install, until the
        // registration is cancelled.
        let registration = protocols.on_protocol_installed::<SampleVendorProtocol>(Tpl::Callback, |handle| {
            log::info!("SampleVendorProtocol notification: Protocol is installed on {handle:?}");
        })?;

        // The callback stays active only while the registration is alive. Dropping it does not cancel
        // it. This sample cancels immediately to demonstrate that call. To cancel it later, the
        // NotifyRegistration value needs to be stored somewhere to pass to cancel in the future.
        protocols.cancel(registration)?;

        Ok(())
    }
}
