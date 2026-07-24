//! End-of-DXE Protocol Consumer Sample Component
//!
//! This component demonstrates deferring protocol consumption to an event group with
//! [`EventServicesExt`], rather than locating the protocol during normal component dispatch. It
//! registers a callback for [`END_OF_DXE_EVENT_GROUP_GUID`], the event group signaled once at the
//! end of the DXE phase, before BDS. When the callback runs, it locates the [`SampleVendorProtocol`]
//! published by the protocol publisher sample and logs the status returned through its interface.
//!
//! This pattern suits a component that must consume a protocol that may not be published yet when
//! it dispatches, without taking an explicit dispatch dependency on the publisher.
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
                event::{EventServices, EventServicesExt, Tpl},
                protocol::{ProtocolServices, ProtocolServicesExt},
            },
        },
    },
    error::Result,
    pi::event::END_OF_DXE_EVENT_GROUP_GUID,
};

use super::protocol_publisher::SampleVendorProtocol;

/// Registers an End-of-DXE callback that locates [`SampleVendorProtocol`] and logs its status.
#[derive(Default)]
pub struct EndOfDxeProtocolConsumerSample;

#[component]
impl EndOfDxeProtocolConsumerSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, events: Service<dyn EventServices>, protocols: Service<dyn ProtocolServices>) -> Result<()> {
        // This example checks for a protocol at End of DXE. The callback fires once every
        // event that shares the group (including this one) is signaled. move is used to
        // capture the protocols service by value for use in the closure. This allows the
        // closure to take ownership of the protocols service so it can be used when the
        // callback runs in the future (after this entry_point function returns).
        events.on_event_group(END_OF_DXE_EVENT_GROUP_GUID, Tpl::Callback, move || {
            match protocols.locate_protocol::<SampleVendorProtocol>() {
                Ok(protocol) => {
                    let status = (protocol.get_status)();
                    log::info!("End of DXE: sample_get_status returned {status:#x}");
                }
                Err(_) => log::debug!("End of DXE: SampleVendorProtocol not published"),
            }
        })?;

        log::info!("Registered End-of-DXE callback for SampleVendorProtocol");

        Ok(())
    }
}
