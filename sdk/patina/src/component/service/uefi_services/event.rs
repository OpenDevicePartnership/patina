//! Event services for Patina components.
//!
//! [`EventServices`] exposes UEFI event operations as a Rust service. Notification callbacks are
//! supplied as Rust closures rather than C function pointers with an opaque context argument, and
//! events are represented by the opaque [`Event`] handle rather than a raw pointer.
//!
//! Timer events are handled separately by [`TimerEventServices`](super::timer_event::TimerEventServices),
//! since arming a timer depends on the Timer Architectural Protocol.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::boxed::Box;
use core::ffi::c_void;
use core::ptr::NonNull;

use crate::base::error::EfiError;
use crate::base::guid::BinaryGuid;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// A notification callback invoked when an event is signaled.
///
/// The callback is supplied to [`EventServices::create_event`],
/// [`EventServices::create_event_for_group`], and
/// [`TimerEventServices::create_timer_event`](super::timer_event::TimerEventServices::create_timer_event),
/// and is owned by the event until the event is closed.
pub type EventNotifyCallback = Box<dyn FnMut() + 'static>;

/// An opaque handle to a created event.
///
/// A handle is returned by [`EventServices::create_event`] and
/// [`TimerEventServices::create_timer_event`](super::timer_event::TimerEventServices::create_timer_event)
/// and is passed back to the other service methods to refer to the event. The handle is a
/// copyable token; it does not own the event, and the event must be released with
/// [`EventServices::close_event`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event(NonNull<c_void>);

impl Event {
    /// Wraps a raw event handle produced by the service implementation.
    ///
    /// This is intended for use by service implementations, not component authors.
    #[doc(hidden)]
    pub fn from_raw(handle: *mut c_void) -> Option<Self> {
        NonNull::new(handle).map(Self)
    }

    /// Returns the raw event handle for use by the service implementation.
    ///
    /// This is intended for use by service implementations, not component authors.
    #[doc(hidden)]
    pub fn as_raw(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}

/// The task priority level (TPL) at which an event's notification runs.
pub use super::tpl::Tpl;

/// Errors that can occur when using [`EventServices`].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum EventError {
    /// A provided parameter (such as an unknown event handle) was invalid.
    InvalidParameter,
    /// The requested resource was not found.
    NotFound,
    /// An unexpected internal error occurred.
    Internal,
}

impl From<EventError> for EfiError {
    fn from(value: EventError) -> Self {
        match value {
            EventError::InvalidParameter => EfiError::InvalidParameter,
            EventError::NotFound => EfiError::NotFound,
            EventError::Internal => EfiError::Unsupported,
        }
    }
}

impl From<EfiError> for EventError {
    fn from(value: EfiError) -> Self {
        match value {
            EfiError::InvalidParameter => EventError::InvalidParameter,
            EfiError::NotFound => EventError::NotFound,
            _ => EventError::Internal,
        }
    }
}

/// Event services.
///
/// This trait is object-safe and its creation methods take a pre-boxed [`EventNotifyCallback`].
/// Component authors should generally use the type-safe methods provided by [`EventServicesExt`]
/// instead of calling these methods directly.
///
/// This service is implemented by the Patina DXE Core. Components consume it by adding a
/// [`Service<dyn EventServices>`](crate::component::service::Service) parameter to their entry
/// point.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait EventServices {
    /// Creates an event with a notification callback that runs when the event is signaled.
    ///
    /// The `callback` is invoked at `notify_tpl` each time the event is signaled. The callback is
    /// owned by the event and is dropped when the event is closed with [`Self::close_event`].
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if the event could not be created.
    fn create_event(&self, notify_tpl: Tpl, callback: EventNotifyCallback) -> Result<Event, EventError>;

    /// Creates an event with a notification callback that runs whenever `group` is signaled.
    ///
    /// The `callback` runs at `notify_tpl` when any event that is a member of `group` (including
    /// this one) is signaled. For example, [`crate::pi::event::END_OF_DXE_EVENT_GROUP_GUID`]. This
    /// lets a component defer work until an event group fires, without taking a dispatch
    /// dependency on whatever signals it. The callback is owned by the event and is dropped when
    /// the event is closed with [`Self::close_event`].
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if the event could not be created.
    fn create_event_for_group(
        &self,
        group: BinaryGuid,
        notify_tpl: Tpl,
        callback: EventNotifyCallback,
    ) -> Result<Event, EventError>;

    /// Signals an event, queuing its notification callback for dispatch.
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if `event` is not a valid event.
    fn signal_event(&self, event: Event) -> Result<(), EventError>;

    /// Checks whether an event is in the signaled state, clearing it if so.
    ///
    /// Returns `true` if the event was signaled. The event must not be a notify-signal event.
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if `event` is not a valid non-signal event.
    fn check_event(&self, event: Event) -> Result<bool, EventError>;

    /// Closes an event, releasing it and dropping its notification callback.
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if `event` is not a valid event.
    fn close_event(&self, event: Event) -> Result<(), EventError>;
}

/// Type-safe extension methods for [`EventServices`].
///
/// These methods accept a plain closure and box it internally, so callers never write
/// `Box::new` themselves. The trait is implemented for every [`EventServices`] implementor
/// (including [`Service<dyn EventServices>`]).
///
/// [`Service<dyn EventServices>`]: crate::component::service::Service
///
/// # Examples
///
/// ```rust,no_run
/// use patina::component::service::{Service, uefi_services::event::{EventServices, EventServicesExt, Tpl}};
/// use patina::error::Result;
///
/// fn entry_point(events: Service<dyn EventServices>) -> Result<()> {
///     let _event = events.on_event(Tpl::Callback, || {
///         log::info!("signaled");
///     })?;
///     Ok(())
/// }
/// ```
pub trait EventServicesExt: EventServices {
    /// Creates an event with a notification callback that runs when the event is signaled.
    ///
    /// Equivalent to [`EventServices::create_event`], but takes a plain closure instead of a
    /// pre-boxed [`EventNotifyCallback`].
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if the event could not be created.
    fn on_event(&self, notify_tpl: Tpl, callback: impl FnMut() + 'static) -> Result<Event, EventError> {
        self.create_event(notify_tpl, Box::new(callback))
    }

    /// Creates an event with a notification callback that runs whenever `group` is signaled.
    ///
    /// Equivalent to [`EventServices::create_event_for_group`], but takes a plain closure instead
    /// of a pre-boxed [`EventNotifyCallback`].
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidParameter`] if the event could not be created.
    fn on_event_group(
        &self,
        group: BinaryGuid,
        notify_tpl: Tpl,
        callback: impl FnMut() + 'static,
    ) -> Result<Event, EventError> {
        self.create_event_for_group(group, notify_tpl, Box::new(callback))
    }
}

impl<T: EventServices + ?Sized> EventServicesExt for T {}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_event() -> Event {
        Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()
    }

    #[test]
    fn test_event_services_error_to_efi() {
        assert_eq!(EfiError::from(EventError::InvalidParameter), EfiError::InvalidParameter);
        assert_eq!(EfiError::from(EventError::NotFound), EfiError::NotFound);
        assert_eq!(EfiError::from(EventError::Internal), EfiError::Unsupported);
    }

    #[test]
    fn test_event_services_error_from_efi() {
        assert_eq!(EventError::from(EfiError::InvalidParameter), EventError::InvalidParameter);
        assert_eq!(EventError::from(EfiError::NotFound), EventError::NotFound);
        assert_eq!(EventError::from(EfiError::DeviceError), EventError::Internal);
    }

    #[test]
    fn test_event_services_handle_dummy() {
        assert!(Event::from_raw(core::ptr::null_mut()).is_none());
        let event = dummy_event();
        assert_eq!(event.as_raw(), NonNull::<c_void>::dangling().as_ptr());
    }

    #[test]
    fn test_event_services_mock_event_group_flow() {
        let mut mock = MockEventServices::new();
        mock.expect_create_event_for_group()
            .times(1)
            .returning(|_, _, _| Ok(Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()));
        mock.expect_close_event().times(1).returning(|_| Ok(()));

        let event = mock.create_event_for_group(BinaryGuid::ZERO, Tpl::Callback, Box::new(|| {})).unwrap();
        assert!(mock.close_event(event).is_ok());
    }

    #[test]
    fn test_event_services_ext_on_event() {
        let mut mock = MockEventServices::new();
        mock.expect_create_event()
            .times(1)
            .returning(|_, _| Ok(Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()));

        assert!(mock.on_event(Tpl::Callback, || {}).is_ok());
    }

    #[test]
    fn test_event_services_ext_on_event_group() {
        let mut mock = MockEventServices::new();
        mock.expect_create_event_for_group()
            .times(1)
            .returning(|_, _, _| Ok(Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()));

        assert!(mock.on_event_group(BinaryGuid::ZERO, Tpl::Callback, || {}).is_ok());
    }
}
