//! Protocol Dependency Sample Component
//!
//! `Protocol<P>` lets a component express a dispatch dependency on a specific UEFI protocol
//! directly in its `entry_point` signature, instead of locating the protocol manually through
//! `Service<dyn ProtocolServices>` (see [`super::protocol_consumer`]). The component is not
//! dispatched until the protocol is installed, and dereferencing the parameter then gives direct
//! access to the interface, with no lookup call, handle, or GUID needed at the call site.
//!
//! If you are creating new components that do not have a pre-existing requirement to use protocols,
//! Patina services should be used for sharing functionality instead of protocols.
//!
//! [`ProtocolDependencySample`] shows a hard protocol dependency as it never runs until the protocol
//! is installed. [`OptionalProtocolDependencySample`] wraps the parameter in `Option` so the component
//! runs immediately either way, reading the protocol only when it happens to already be installed.
//!
//! Both samples consume [`SampleVendorProtocol`], published by [`super::protocol_publisher`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina::{
    component::{component, protocol::Protocol},
    error::Result,
};

use super::protocol_publisher::SampleVendorProtocol;

/// Only dispatched once [`SampleVendorProtocol`] is installed.
#[derive(Default)]
pub struct ProtocolDependencySample;

#[component]
impl ProtocolDependencySample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, protocol: Protocol<SampleVendorProtocol>) -> Result<()> {
        // `protocol` already dereferences to `SampleVendorProtocol`. The dispatcher only
        // calls this entry point once the protocol is confirmed installed, so no lookup
        // call, handle, or GUID is needed.
        let status = (protocol.get_status)();
        log::info!(
            "Dispatched now that SampleVendorProtocol (rev {:#x}) is installed, status is {status:#x}",
            protocol.revision
        );
        Ok(())
    }
}

/// This component is always dispatched immediately. It only reads [`SampleVendorProtocol`]
/// if it is already installed.
#[derive(Default)]
pub struct OptionalProtocolDependencySample;

#[component]
impl OptionalProtocolDependencySample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, protocol: Option<Protocol<SampleVendorProtocol>>) -> Result<()> {
        // Unlike `Protocol<P>` alone, `Option<Protocol<P>>` never blocks dispatch: this component
        // runs immediately, and `protocol` is `None` until SampleVendorProtocol is installed.
        match protocol {
            Some(protocol) => {
                let status = (protocol.get_status)();
                log::info!("SampleVendorProtocol is already installed, status is {status:#x}");
            }
            None => log::debug!("SampleVendorProtocol is not installed yet, continuing without it"),
        }
        Ok(())
    }
}
