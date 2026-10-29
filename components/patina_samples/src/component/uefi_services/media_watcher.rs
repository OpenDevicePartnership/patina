//! Media Watcher Sample Component
//!
//! The sample component demonstrates keeping per-controller state that a timer callback updates, using a
//! [`ControllerDriver`]. It helps show how to use state within a closure in a driver binding function.
//!
//! This driver implementation manages every Block I/O controller and polls its media once a second to notice if a
//! removable disk is being swapped when the device cannot signal it. The timer is created in `start()` and needs the
//! controller's state in its callback. [`StartContext::state_ref`] provides a [`ControllerStateRef`] that can be
//! moved into the closure. The callback calls [`ControllerStateRef::with_protocol`] on each tick, and the driver
//! closes the timer in `stop()`.
//!
//! [`ControllerDriver`]: patina::component::service::uefi_services::controller_driver::ControllerDriver
//! [`StartContext::state_ref`]: patina::component::service::uefi_services::controller_driver::StartContext::state_ref
//! [`ControllerStateRef`]: patina::component::service::uefi_services::controller_driver::ControllerStateRef
//! [`ControllerStateRef::with_protocol`]: patina::component::service::uefi_services::controller_driver::ControllerStateRef::with_protocol
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::time::Duration;

use patina::{
    component::{
        component,
        service::{
            Service,
            uefi_services::{
                controller_driver::{ControllerDriver, StartContext, SupportedContext, install_controller_driver},
                driver::DriverServices,
                event::{Event, EventError, EventServices},
                protocol::{Handle, ProtocolError, ProtocolServices, ProtocolServicesExt},
                timer_event::{TimerEventServices, TimerEventServicesExt, TimerType, Tpl},
            },
        },
    },
    error::{EfiError, Result},
    standard::efi::protocols::block_io::Protocol as BlockIo,
};

/// How often each controller's media is polled.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// The parts of a Block I/O controller's media that identify the disk currently in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MediaStatus {
    media_id: u32,
    present: bool,
}

impl MediaStatus {
    /// Reads the current media status, or `None` if the controller has no media description.
    fn read(protocol: &BlockIo) -> Option<Self> {
        let media = protocol.media;
        if media.is_null() {
            return None;
        }
        // SAFETY: A Block I/O driver keeps `media` valid for as long as the protocol is installed. The fields are
        // read through the raw pointer because the driver updates them in place.
        Some(unsafe { Self { media_id: (*media).media_id, present: (*media).media_present } })
    }
}

/// State kept for each controller.
struct WatchState {
    timer: Event,
    last: MediaStatus,
    changes: u32,
}

impl WatchState {
    /// Logs `controller`'s media and records it if it changed since the last poll.
    fn poll(&mut self, controller: Handle, protocol: &BlockIo) {
        let Some(now) = MediaStatus::read(protocol) else {
            return;
        };
        // This is a noisy log message but it helps track the media status of each controller over time.
        log::info!("MediaWatcher: {controller:?} media {now:?}. Changes: {}", self.changes);
        if now != self.last {
            self.changes += 1;
            log::info!("MediaWatcher: media changed on {controller:?}: {:?} -> {now:?}", self.last);
            self.last = now;
        }
    }
}

fn event_error(err: EventError) -> ProtocolError {
    ProtocolError::from(EfiError::from(err))
}

/// Polls the media of every Block I/O controller it manages.
struct MediaWatcher {
    timers: Service<dyn TimerEventServices>,
    events: Service<dyn EventServices>,
}

impl ControllerDriver for MediaWatcher {
    type Protocol = BlockIo;
    type State = WatchState;

    fn supported(&self, context: &SupportedContext<'_, Self>) -> core::result::Result<(), ProtocolError> {
        MediaStatus::read(context.protocol()).map(|_| ()).ok_or(ProtocolError::Unsupported)
    }

    fn start(&self, context: &StartContext<'_, Self>) -> core::result::Result<WatchState, ProtocolError> {
        let last = MediaStatus::read(context.protocol()).ok_or(ProtocolError::Unsupported)?;

        // Note: The reference is only valid for use once start() returns. The first tick is a full interval away.
        let controller = context.controller();
        let state = context.state_ref();
        let timer = self
            .timers
            .on_timer_event(Tpl::Callback, move || {
                // `Busy` means this tick fired while the driver is already using the state, for example in stop().
                let result = state.with_protocol(|state, protocol| state.poll(controller, protocol));
                if let Err(err) = result {
                    log::debug!("MediaWatcher: skipped poll of {controller:?}: {err}");
                }
            })
            .map_err(event_error)?;

        if let Err(err) = self.timers.set_timer(timer, TimerType::Periodic(POLL_INTERVAL)) {
            let _ = self.events.close_event(timer);
            return Err(event_error(err));
        }

        log::info!("MediaWatcher: watching {controller:?}, media {last:?}");
        Ok(WatchState { timer, last, changes: 0 })
    }

    fn stop(&self, controller: Handle, state: &mut WatchState) -> core::result::Result<(), ProtocolError> {
        // Closing the event cancels the timer and drops the closure.
        self.events.close_event(state.timer).map_err(event_error)?;
        log::info!("MediaWatcher: released {controller:?} after {} media changes", state.changes);
        Ok(())
    }
}

/// Installs `MediaWatcher` and connects it to Block I/O controllers.
#[derive(Default)]
pub struct MediaWatcherSample;

#[component]
impl MediaWatcherSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(
        self,
        protocols: Service<dyn ProtocolServices>,
        drivers: Service<dyn DriverServices>,
        timers: Service<dyn TimerEventServices>,
        events: Service<dyn EventServices>,
    ) -> Result<()> {
        install_controller_driver(protocols, MediaWatcher { timers, events })?;

        for controller in protocols.locate_handles_for::<BlockIo>()? {
            if let Err(err) = drivers.connect_controller(controller, false) {
                log::debug!("MediaWatcher: not connected to {controller:?}: {err:?}");
            }
        }

        Ok(())
    }
}
