//! UEFI Services Overview Sample Component
//!
//! This component demonstrates using the Patina UEFI Services from
//! [`patina::component::service::uefi_services`] in some common patterns.
//!
//! For samples of how to use individual services, see the other sibling modules:
//!
//! - [`super::configuration_table`] - Installing and reading a vendor configuration table.
//! - [`super::driver_connect`] - Locating controllers and connecting drivers to them.
//! - [`super::end_of_dxe_protocol_consumer`] - Deferring protocol consumption to the End-of-DXE event group.
//! - [`super::protocol_consumer`] - Different approaches to consume a protocol.
//! - [`super::protocol_publisher`] - Publishing a protocol for other components to consume.
//! - [`super::timers`] - One-shot and periodic timers used with Rust closures.
//! - [`super::tpl_critical_section`] - Guarding shared state with a raised task priority level.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::boxed::Box;
use core::sync::atomic::{AtomicU32, Ordering};
use core::time::Duration;

use patina::standard::efi::protocols::graphics_output::Protocol as GraphicsOutput;
use patina::{
    component::{
        component,
        service::{
            Service,
            uefi_services::{
                protocol::{ProtocolServices, ProtocolServicesExt},
                timer_event::{TimerEventServices, TimerType, Tpl},
                timing::TimingServices,
            },
        },
    },
    error::Result,
};

/// Counts how many times the sample timer has fired. Shared with the timer's notification closure.
static TICK_COUNT: AtomicU32 = AtomicU32::new(0);

/// A sample component that consumes the timing, event, and protocol UEFI services.
#[derive(Default)]
pub struct UefiServicesSample;

#[component]
impl UefiServicesSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(
        self,
        timing: Service<dyn TimingServices>,
        timer_events: Service<dyn TimerEventServices>,
        protocols: Service<dyn ProtocolServices>,
    ) -> Result<()> {
        // Using the timing service to stall for one millisecond.
        timing.stall(Duration::from_millis(1))?;

        // Create a periodic timer whose Rust closure runs on each tick. The closure is
        // owned by the event and dropped when the event is closed.
        let timer = timer_events.create_timer_event(
            Tpl::Callback,
            Box::new(|| {
                TICK_COUNT.fetch_add(1, Ordering::Relaxed);
            }),
        )?;
        // Fire every 10 milliseconds.
        timer_events.set_timer(timer, TimerType::Periodic(Duration::from_millis(10)))?;

        // Locate a protocol in safe code. The returned value is a reference bound to the
        // protocol's interface type. The component never handles a raw pointer or GUID.
        match protocols.locate_protocol::<GraphicsOutput>() {
            Ok(_gop) => log::info!("Graphics Output Protocol is available"),
            Err(_) => log::debug!("Graphics Output Protocol not present at this time"),
        }

        Ok(())
    }
}
