//! Timer event services for Patina components.
//!
//! [`TimerEventServices`] exposes UEFI timer-event operations, creating and arming a timer event,
//! as a safe, idiomatic Rust service. It is split out from [`EventServices`](super::event::EventServices)
//! because arming a timer depends on the Timer Architectural Protocol, so a component depending on
//! this service is not dispatched until the protocol is available and timers can actually fire.
//!
//! A timer event either runs a closure via [`TimerEventServices::create_timer_event`], or is polled
//! with [`EventServices::check_event`](super::event::EventServices::check_event) after being created
//! with [`TimerEventServices::create_timer_event_no_notify`], which has no notification callback.
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

    /// Creates a timer event with no notification callback.
    ///
    /// The returned event can be armed with [`Self::set_timer`].
    ///
    /// While [`Self::create_timer_event`] requires a notification callback, this does not and it
    /// can be polled with [`EventServices::check_event`](super::event::EventServices::check_event) instead.
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if the event could not be created.
    fn create_timer_event_no_notify(&self) -> Result<Event, EventError>;

    /// Arms, re-arms, or cancels the timer on a timer event.
    ///
    /// The event must have been created with [`Self::create_timer_event`] or
    /// [`Self::create_timer_event_no_notify`].
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if `event` is not a valid timer event.
    fn set_timer(&self, event: Event, timer_type: TimerType) -> Result<(), EventError>;
}

/// Type-safe extension methods for [`TimerEventServices`].
///
/// These methods accept a plain closure and box it internally, so callers don't need to write
/// `Box::new` themselves. The trait is implemented for every [`TimerEventServices`] implementor
/// (including [`Service<dyn TimerEventServices>`]).
///
/// [`Service<dyn TimerEventServices>`]: crate::component::service::Service
///
/// # Examples
///
/// A one-shot timer that closes its own event once it fires. Nothing is returned to the caller,
/// since a relative timer does not fire again and the callback is the event's only owner:
///
/// ```rust,no_run
/// use core::time::Duration;
/// use patina::component::service::{
///     Service,
///     uefi_services::{event::EventServices, timer_event::{TimerEventServices, TimerEventServicesExt, Tpl, TimerType}},
/// };
/// use patina::error::Result;
///
/// fn entry_point(timer_events: Service<dyn TimerEventServices>, events: Service<dyn EventServices>) -> Result<()> {
///     timer_events.on_timer_event_self_managed(Tpl::Callback, TimerType::Relative(Duration::from_millis(50)), move |event| {
///         log::info!("fired once");
///         events.close_event(event).expect("failed to close event");
///     })?;
///     Ok(())
/// }
/// ```
pub trait TimerEventServicesExt: TimerEventServices {
    /// Creates a timer event with a notification callback.
    ///
    /// Equivalent to [`TimerEventServices::create_timer_event`], but takes a plain closure instead
    /// of a pre-boxed [`EventNotifyCallback`]. The callback does not receive the event. The caller
    /// owns the returned [`Event`] and is responsible for arming it with
    /// [`TimerEventServices::set_timer`] and for signaling, checking, and closing it. Use
    /// [`Self::on_timer_event_self_managed`] instead if the callback itself needs to act on its
    /// own event.
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if the event could not be created.
    fn on_timer_event(&self, notify_tpl: Tpl, mut callback: impl FnMut() + 'static) -> Result<Event, EventError> {
        self.create_timer_event(notify_tpl, Box::new(move |_event| callback()))
    }

    /// Creates and arms a timer event in one call, giving the callback its own event instead of
    /// returning it to the caller.
    ///
    /// Nothing is returned on success. The callback is the event's only owner, so there is
    /// nothing left for the caller to arm again, signal, check, or close. This is useful for a timer
    /// whose callback fully manages its own lifecycle. For example, a one-shot [`TimerType::Relative`]
    /// timer that closes itself once it fires, since it is only ever going to fire the once.
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if the event could not be created. If the event
    /// is created but [`TimerEventServices::set_timer`] then fails, that error is returned and the
    /// event is left registered but unarmed, with no handle left to close it; this mirrors arming a
    /// [`Self::on_timer_event`]-created event failing after creation succeeds.
    fn on_timer_event_self_managed(
        &self,
        notify_tpl: Tpl,
        timer_type: TimerType,
        callback: impl FnMut(Event) + 'static,
    ) -> Result<(), EventError> {
        let event = self.create_timer_event(notify_tpl, Box::new(callback))?;
        self.set_timer(event, timer_type)
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

        let event = mock.create_timer_event(Tpl::Callback, Box::new(|_event| {})).unwrap();
        assert!(mock.set_timer(event, TimerType::Periodic(Duration::from_millis(10))).is_ok());
    }

    #[test]
    fn test_timer_event_services_mock_create_timer_event_no_notify() {
        let mut mock = MockTimerEventServices::new();
        mock.expect_create_timer_event_no_notify()
            .times(1)
            .returning(|| Ok(Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()));

        assert!(mock.create_timer_event_no_notify().is_ok());
    }

    #[test]
    fn test_timer_event_services_ext_on_timer_event() {
        let mut mock = MockTimerEventServices::new();
        mock.expect_create_timer_event()
            .times(1)
            .returning(|_, _| Ok(Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()));

        assert!(mock.on_timer_event(Tpl::Callback, || {}).is_ok());
    }

    #[test]
    fn test_timer_event_services_ext_on_timer_event_ignores_its_own_event() {
        use alloc::rc::Rc;
        use core::cell::Cell;

        let mut mock = MockTimerEventServices::new();
        mock.expect_create_timer_event().times(1).returning(|_, mut callback| {
            callback(Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap());
            Ok(Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap())
        });

        let ran: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let ran_in_callback = ran.clone();
        assert!(mock.on_timer_event(Tpl::Callback, move || ran_in_callback.set(true)).is_ok());
        assert!(ran.get());
    }

    #[test]
    fn test_timer_event_services_ext_on_timer_event_self_managed() {
        use alloc::rc::Rc;
        use core::cell::Cell;

        let mut mock = MockTimerEventServices::new();
        mock.expect_create_timer_event().times(1).returning(|_, mut callback| {
            callback(Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap());
            Ok(Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap())
        });
        mock.expect_set_timer().times(1).returning(|_, timer_type| {
            assert_eq!(timer_type, TimerType::Relative(Duration::from_millis(5)));
            Ok(())
        });

        let received: Rc<Cell<Option<Event>>> = Rc::new(Cell::new(None));
        let received_in_callback = received.clone();
        let result = mock.on_timer_event_self_managed(
            Tpl::Callback,
            TimerType::Relative(Duration::from_millis(5)),
            move |event| {
                received_in_callback.set(Some(event));
            },
        );

        assert_eq!(result, Ok(()));
        assert_eq!(received.get(), Some(Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()));
    }

    #[test]
    fn test_timer_event_services_ext_on_timer_event_self_managed_propagates_set_timer_error() {
        let mut mock = MockTimerEventServices::new();
        mock.expect_create_timer_event()
            .times(1)
            .returning(|_, _| Ok(Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()));
        mock.expect_set_timer().times(1).returning(|_, _| Err(EventError::InvalidParameter));

        let result =
            mock.on_timer_event_self_managed(Tpl::Callback, TimerType::Relative(Duration::from_millis(5)), |_event| {});

        assert_eq!(result, Err(EventError::InvalidParameter));
    }
}
