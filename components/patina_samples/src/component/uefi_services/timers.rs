//! Timer and Event Sample Component
//!
//! This component demonstrates [`TimerEventServicesExt`] timers. [`TimerEventServices`] is only
//! registered by the DXE Core once the Timer Architectural Protocol, the protocol backing the
//! `SetTimer()` boot service, is installed, so this component is simply not dispatched until
//! timers can be used. It shows a **one-shot** timer that fires once after a delay and a
//! **periodic** timer that fires repeatedly. Notifications are ordinary Rust closures. The
//! closure is owned by the event and dropped when the event is closed.
//!
//! Because a timer closure runs asynchronously at a raised task priority level, it communicates
//! with the rest of the component through `'static` atomics rather than captured borrows.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use core::time::Duration;

use patina::{
    component::{
        component,
        service::{
            Service,
            uefi_services::{
                event::EventServices,
                timer_event::{TimerEventServices, TimerEventServicesExt, TimerType, Tpl},
                timing::TimingServices,
            },
        },
    },
    error::Result,
};

/// Set to `true` by the one-shot timer's closure when it fires.
static ONE_SHOT_FIRED: AtomicBool = AtomicBool::new(false);
/// Incremented by the periodic timer's closure on every tick.
static PERIODIC_TICKS: AtomicU32 = AtomicU32::new(0);

/// Arms a one-shot timer and a periodic timer. Dispatched only once the Timer Architectural
/// Protocol is installed.
#[derive(Default)]
pub struct TimerSample;

#[component]
impl TimerSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(
        self,
        timer_events: Service<dyn TimerEventServices>,
        timing: Service<dyn TimingServices>,
        events: Service<dyn EventServices>,
    ) -> Result<()> {
        // Fire once, 50 ms from now. `TimerType::Relative` schedules a single fire.
        let one_shot = timer_events.on_timer_event(Tpl::Callback, || {
            ONE_SHOT_FIRED.store(true, Ordering::Relaxed);
            log::info!("Logged from the one-shot timer event");
        })?;
        timer_events.set_timer(one_shot, TimerType::Relative(Duration::from_millis(5)))?;

        // Fire every 10 ms until cancelled. `TimerType::Periodic` re-arms automatically.
        let periodic = timer_events.on_timer_event(Tpl::Callback, || {
            PERIODIC_TICKS.fetch_add(1, Ordering::Relaxed);
            log::info!("Logged from the periodic timer event");
        })?;
        timer_events.set_timer(periodic, TimerType::Periodic(Duration::from_millis(10)))?;

        log::info!("Armed one-shot (50 ms) and periodic (10 ms) timers");

        // Give the one shot timer enough time to fire before cancelling it.
        // There should be at least 5 ticks of the periodic timer during this time,
        // as well but the exact number is not guaranteed.
        timing.stall(Duration::from_millis(50))?;

        // A component that only needed the one-shot would cancel and close it once done. Here we
        // close the one-shot event to show the cleanup path; closing drops its closure. The
        // periodic timer is left running to demonstrate a long-lived event.
        timer_events.set_timer(one_shot, TimerType::Cancel)?;
        events.close_event(one_shot)?;

        Ok(())
    }
}
