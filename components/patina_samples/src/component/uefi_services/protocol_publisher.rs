//! Protocol Publisher Sample Component
//!
//! These demonstrate the producer side of  [`ProtocolServices`]. This component publishes a
//! protocol interface, and another consumes it.
//!
//! This is the pattern to use when a component must expose functionality to code that is not part
//! of the Patina component model (for example, a UEFI driver written in C), or when interoperating
//! with the wider UEFI protocol database. Otherwise, producing a Patina component service should be
//! preferred.
//!
//! The interface is a plain `#[repr(C)]` struct bound to a GUID via [`ProtocolInterface`]. Neither
//! component ever handles a raw pointer or GUID directly. [`ProtocolServicesExt`] methods derive
//! the GUID from the interface type.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{
    BinaryGuid,
    component::{
        component,
        service::{
            Service,
            uefi_services::protocol::{ProtocolServices, ProtocolServicesExt},
        },
    },
    error::Result,
    protocol::ProtocolInterface,
};

/// A sample vendor protocol exposing a single `revision` field and a function pointer.
///
/// UEFI protocols must be a `#[repr(C)]` struct of data with `extern "efiapi"` function pointers.
/// The layout must match what consumers expect, which is encoded by [`ProtocolInterface`].
#[repr(C)]
pub struct SampleVendorProtocol {
    /// Interface revision, so consumers can detect the layout they are talking to.
    pub revision: u64,
    /// Returns a vendor-defined status value.
    pub get_status: extern "efiapi" fn() -> u64,
}

// SAFETY: `SampleVendorProtocol` is `#[repr(C)]` and this GUID is used consistently for both the
// the install (publisher) and locate (consumer) paths in this sample, so the GUID correctly indicates
// the layout of the protocol binary interface.
unsafe impl ProtocolInterface for SampleVendorProtocol {
    const PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("a1b2c3d4-e5f6-4789-abcd-ef0123456789");
}

/// The `get_status` implementation backing the published interface.
extern "efiapi" fn sample_get_status() -> u64 {
    0x1234_5678
}

/// The interface instance. It must live for `'static` because the protocol database stores a
/// pointer to it for the lifetime of boot services.
static SAMPLE_PROTOCOL: SampleVendorProtocol =
    SampleVendorProtocol { revision: 0x0001_0000, get_status: sample_get_status };

/// Publishes [`SampleVendorProtocol`] on a new handle so other components (or drivers) can find it.
#[derive(Default)]
pub struct ProtocolPublisherSample;

#[component]
impl ProtocolPublisherSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, protocols: Service<dyn ProtocolServices>) -> Result<()> {
        // Passing `None` for the handle asks the core to create a fresh handle for the interface.
        let handle = protocols.install_protocol::<SampleVendorProtocol>(None, &SAMPLE_PROTOCOL)?;
        log::info!("published SampleVendorProtocol on handle {handle:?}");
        Ok(())
    }
}

/// Consumes [`SampleVendorProtocol`] by locating it and calling through its interface.
#[derive(Default)]
pub struct ProtocolConsumerSample;

#[component]
impl ProtocolConsumerSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, protocols: Service<dyn ProtocolServices>) -> Result<()> {
        match protocols.locate_protocol::<SampleVendorProtocol>() {
            Ok(protocol) => {
                let status = (protocol.get_status)();
                log::info!("SampleVendorProtocol rev {:#x} returned status {status:#x}", protocol.revision);
            }
            Err(_) => log::debug!("SampleVendorProtocol is not published yet"),
        }
        Ok(())
    }
}
