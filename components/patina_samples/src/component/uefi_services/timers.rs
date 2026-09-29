//! Timer and Event Sample Component
//!
//! This component demonstrates [`TimerEventServicesExt`] timers. [`TimerEventServices`] is only
//! registered by the DXE Core once the Timer Architectural Protocol, the protocol backing the
//! `SetTimer()` boot service, is installed, so this component is simply not dispatched until
//! timers can be used. It shows a **one-shot** timer, created with
//! [`TimerEventServicesExt::on_timer_event_self_managed`], whose closure owns its event
//! exclusively and closes it once it fires, since a relative timer never fires again. It also
//! shows a **periodic** timer, created with [`TimerEventServicesExt::on_timer_event`], whose
//! event is instead owned by the caller and left running to demonstrate a long-lived event.
//!
//! Because a timer closure runs asynchronously at a raised task priority level, it communicates
//! with the rest of the component through `'static` atomics rather than captured borrows.
//!
//! It also shows a **polling** timer, created with [`TimerEventServices::create_timer_event_no_notify`]
//! instead of a closure, and checked with [`EventServices::check_event`]. This avoids the cost of a
//! notification callback for a component that is already polling in a loop.
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
        // Fire once, 50 ms from now. `TimerType::Relative` schedules a single fire, so the
        // closure closes its own event (the one it is passed) once it runs. This is the only way
        // to arm a self-managed timer, since creation and arming happen in one call, nothing is
        // returned to the caller, as the closure is the event's sole owner from here on.
        timer_events.on_timer_event_self_managed(
            Tpl::Callback,
            TimerType::Relative(Duration::from_millis(5)),
            move |event| {
                ONE_SHOT_FIRED.store(true, Ordering::Relaxed);
                log::info!("Logged from the one-shot timer event");
                if let Err(e) = events.close_event(event) {
                    log::error!("Failed to close one-shot timer event: {e:?}");
                }
            },
        )?;

        // Fire every 10 ms until cancelled. `TimerType::Periodic` re-arms automatically. The
        // caller owns the returned event and is responsible for arming and eventually closing it.
        let periodic = timer_events.on_timer_event(Tpl::Callback, || {
            PERIODIC_TICKS.fetch_add(1, Ordering::Relaxed);
            log::info!("Logged from the periodic timer event");
        })?;
        timer_events.set_timer(periodic, TimerType::Periodic(Duration::from_millis(10)))?;

        log::info!("Armed one-shot (50 ms) and periodic (10 ms) timers");

        // Give the one-shot timer enough time to fire and close itself. There should be at
        // least 5 ticks of the periodic timer during this time as well, but the exact number is
        // not guaranteed.
        timing.stall(Duration::from_millis(50))?;

        // A polling timer can be setup without a notification callback, the caller checks it using
        // `EventServices::check_event` instead of a closure running asynchronously.
        let poll_timer = timer_events.create_timer_event_no_notify()?;
        timer_events.set_timer(poll_timer, TimerType::Relative(Duration::from_millis(5)))?;
        while !events.check_event(poll_timer)? {
            timing.stall(Duration::from_millis(1))?;
        }
        log::info!("Polling timer fired");
        events.close_event(poll_timer)?;

        // The periodic timer is left running to demonstrate a long-lived event. A real component
        // would cancel and close it once its work is done.

        Ok(())
    }
}
