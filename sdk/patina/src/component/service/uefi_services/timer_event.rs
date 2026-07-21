//! Timer event services for Patina components.
//!
//! [`TimerEventServices`] exposes UEFI timer-event operations, creating and arming a timer event,
//! as a safe, idiomatic Rust service. It is split out from [`EventServices`](super::event::EventServices)
//! because arming a timer depends on the Timer Architectural Protocol, so a component depending on
//! this service is not dispatched until the protocol is available and timers can actually fire.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::boxed::Box;
use core::time::Duration;

use super::event::{Event, EventError, EventNotifyCallback};
pub use super::tpl::Tpl;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// Describes how a timer configured with [`TimerEventServices::set_timer`] should fire.
///
/// The duration can be expressed in any unit supported by [`core::time::Duration`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerType {
    /// Cancels a previously configured timer.
    Cancel,
    /// Fires the timer once, `Duration` from now.
    Relative(Duration),
    /// Fires the timer repeatedly, every `Duration`.
    Periodic(Duration),
}

/// Timer event services.
///
/// This trait is object-safe and [`Self::create_timer_event`] takes a pre-boxed
/// [`EventNotifyCallback`]. Component authors should generally use the type-safe method provided
/// by [`TimerEventServicesExt`] instead of calling it directly.
///
/// This service is implemented by the Patina DXE Core. Components consume it by adding a
/// [`Service<dyn TimerEventServices>`](crate::component::service::Service) parameter to their
/// entry point. The DXE Core only registers this service once the Timer Architectural Protocol is
/// installed, so a component depending on it is not dispatched until `set_timer` can actually
/// succeed.
///
/// # Examples
///
/// ```rust,no_run
/// use core::time::Duration;
/// use patina::component::service::{Service, uefi_services::timer_event::{TimerEventServices, TimerEventServicesExt, Tpl, TimerType}};
/// use patina::error::Result;
///
/// fn entry_point(events: Service<dyn TimerEventServices>) -> Result<()> {
///     let event = events.on_timer_event(Tpl::Callback, || {
///         log::info!("tick");
///     })?;
///     events.set_timer(event, TimerType::Periodic(Duration::from_secs(1)))?;
///     Ok(())
/// }
/// ```
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait TimerEventServices {
    /// Creates a timer event with a notification callback.
    ///
    /// The returned event can be armed with [`Self::set_timer`]. The `callback` runs at
    /// `notify_tpl` each time the timer fires, and is dropped when the event is closed with
    /// [`EventServices::close_event`](super::event::EventServices::close_event).
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if the event could not be created.
    fn create_timer_event(&self, notify_tpl: Tpl, callback: EventNotifyCallback) -> Result<Event, EventError>;

    /// Arms, re-arms, or cancels the timer on a timer event.
    ///
    /// The event must have been created with [`Self::create_timer_event`].
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if `event` is not a valid timer event.
    fn set_timer(&self, event: Event, timer_type: TimerType) -> Result<(), EventError>;
}

/// Type-safe extension methods for [`TimerEventServices`].
///
/// This method accepts a plain closure and boxes it internally, so callers don't need to write
/// `Box::new` themselves. The trait is implemented for every [`TimerEventServices`] implementor
/// (including [`Service<dyn TimerEventServices>`]).
///
/// [`Service<dyn TimerEventServices>`]: crate::component::service::Service
pub trait TimerEventServicesExt: TimerEventServices {
    /// Creates a timer event with a notification callback.
    ///
    /// Equivalent to [`TimerEventServices::create_timer_event`], but takes a plain closure instead
    /// of a pre-boxed [`EventNotifyCallback`].
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if the event could not be created.
    fn on_timer_event(&self, notify_tpl: Tpl, callback: impl FnMut() + 'static) -> Result<Event, EventError> {
        self.create_timer_event(notify_tpl, Box::new(callback))
    }
}

impl<T: TimerEventServices + ?Sized> TimerEventServicesExt for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_void;
    use core::ptr::NonNull;

    #[test]
    fn test_timer_event_services_mock_timer_flow() {
        let mut mock = MockTimerEventServices::new();
        mock.expect_create_timer_event()
            .times(1)
            .returning(|_, _| Ok(Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()));
        mock.expect_set_timer().times(1).returning(|_, timer_type| {
            assert_eq!(timer_type, TimerType::Periodic(Duration::from_millis(10)));
            Ok(())
        });

        let event = mock.create_timer_event(Tpl::Callback, Box::new(|| {})).unwrap();
        assert!(mock.set_timer(event, TimerType::Periodic(Duration::from_millis(10))).is_ok());
    }

    #[test]
    fn test_timer_event_services_ext_on_timer_event() {
        let mut mock = MockTimerEventServices::new();
        mock.expect_create_timer_event()
            .times(1)
            .returning(|_, _| Ok(Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()));

        assert!(mock.on_timer_event(Tpl::Callback, || {}).is_ok());
    }
}
