//! DXE Core implementation of [`EventServices`].
//!
//! Notification callbacks supplied by components as Rust closures are boxed and stored with the
//! event's notification context. A single C-ABI "trampoline" recovers the closure and invokes it
//! when the event fires. The closure is reclaimed and dropped when the event is closed.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::boxed::Box;
use core::ffi::c_void;

use patina::BinaryGuid;
use patina::component::service::{
    IntoService,
    uefi_services::event::{Event, EventError, EventNotifyCallback, EventServices, Tpl},
};
use patina::error::EfiError;
use patina::standard::efi;

use crate::events::{EVENT_DB, check_event as core_check_event};

/// Owns a component-supplied notification closure for the lifetime of an event.
struct ClosureHolder {
    callback: EventNotifyCallback,
}

/// C-ABI trampoline registered with every event created through [`create_event_internal`].
///
/// It recovers the [`ClosureHolder`] from the notification context and invokes the closure.
extern "efiapi" fn notify_trampoline(_event: efi::Event, context: *mut c_void) {
    if context.is_null() {
        return;
    }
    // SAFETY: `context` was produced by `Box::into_raw` of a `ClosureHolder` in
    // `create_event_internal` and remains valid until `close_event` reclaims and drops it. UEFI
    // dispatches notifications serially at the event's TPL, so there is no concurrent access.
    let holder = unsafe { &mut *(context as *mut ClosureHolder) };
    (holder.callback)();
}

/// Creates an event backed by a boxed notification closure.
///
/// Shared by [`CoreEventServices`] and [`CoreTimerEventServices`](super::timer_event::CoreTimerEventServices),
/// since both create events through the same closure mechanism.
pub(crate) fn create_event_internal(
    event_type: u32,
    notify_tpl: Tpl,
    callback: EventNotifyCallback,
    event_group: Option<efi::Guid>,
) -> Result<Event, EventError> {
    let holder = Box::new(ClosureHolder { callback });
    let context = Box::into_raw(holder) as *mut c_void;

    match EVENT_DB.create_event(event_type, tpl_to_efi(notify_tpl), Some(notify_trampoline), Some(context), event_group)
    {
        Ok(efi_event) => Event::from_raw(efi_event).ok_or_else(|| {
            // The event database returned a null handle. Reclaim the closure to avoid a leak.
            // SAFETY: `context` came from `Box::into_raw` above and has not been freed.
            drop(unsafe { Box::from_raw(context as *mut ClosureHolder) });
            EventError::Internal
        }),
        Err(err) => {
            // SAFETY: `context` came from `Box::into_raw` above and has not been freed.
            drop(unsafe { Box::from_raw(context as *mut ClosureHolder) });
            Err(EventError::from(err))
        }
    }
}

/// Core implementation of [`EventServices`], delegating to the core event database.
#[derive(IntoService)]
#[service(dyn EventServices)]
pub(crate) struct CoreEventServices;

impl EventServices for CoreEventServices {
    fn create_event(&self, notify_tpl: Tpl, callback: EventNotifyCallback) -> Result<Event, EventError> {
        create_event_internal(efi::EVT_NOTIFY_SIGNAL, notify_tpl, callback, None)
    }

    fn create_event_for_group(
        &self,
        group: BinaryGuid,
        notify_tpl: Tpl,
        callback: EventNotifyCallback,
    ) -> Result<Event, EventError> {
        create_event_internal(efi::EVT_NOTIFY_SIGNAL, notify_tpl, callback, Some(group.into_inner()))
    }

    fn signal_event(&self, event: Event) -> Result<(), EventError> {
        EVENT_DB.signal_event(event.as_raw()).map_err(EventError::from)
    }

    fn check_event(&self, event: Event) -> Result<bool, EventError> {
        match core_check_event(event.as_raw()) {
            efi::Status::SUCCESS => Ok(true),
            efi::Status::NOT_READY => Ok(false),
            status => Err(EventError::from(EfiError::status_to_result(status).unwrap_err())),
        }
    }

    fn close_event(&self, event: Event) -> Result<(), EventError> {
        let efi_event = event.as_raw();

        // Retrieve the closure context before closing so it can be reclaimed afterward.
        let context = EVENT_DB.get_notification_data(efi_event).ok().and_then(|data| data.notify_context);

        EVENT_DB.close_event(efi_event).map_err(EventError::from)?;

        if let Some(context) = context
            && !context.is_null()
        {
            // SAFETY: `context` is the `ClosureHolder` pointer created in `create_event_internal`.
            // The event has just been closed, so no further notifications can reference it.
            drop(unsafe { Box::from_raw(context as *mut ClosureHolder) });
        }

        Ok(())
    }
}

pub(crate) fn tpl_to_efi(tpl: Tpl) -> efi::Tpl {
    match tpl {
        Tpl::Application => efi::TPL_APPLICATION,
        Tpl::Callback => efi::TPL_CALLBACK,
        Tpl::Notify => efi::TPL_NOTIFY,
        Tpl::HighLevel => efi::TPL_HIGH_LEVEL,
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    extern "efiapi" fn noop_wait_notify(_event: efi::Event, _context: *mut c_void) {}

    #[test]
    fn test_tpl_to_efi_maps_all_variants() {
        assert_eq!(tpl_to_efi(Tpl::Application), efi::TPL_APPLICATION);
        assert_eq!(tpl_to_efi(Tpl::Callback), efi::TPL_CALLBACK);
        assert_eq!(tpl_to_efi(Tpl::Notify), efi::TPL_NOTIFY);
        assert_eq!(tpl_to_efi(Tpl::HighLevel), efi::TPL_HIGH_LEVEL);
    }

    #[test]
    fn test_core_event_services_create_event_rejects_invalid_notify_tpl() {
        crate::test_support::with_global_lock(|| {
            let service = CoreEventServices;

            // TPL_APPLICATION is one level below the minimum notify TPL that `Event::new` accepts.
            let result = service.create_event(Tpl::Application, Box::new(|| {}));

            assert_eq!(result, Err(EventError::InvalidParameter));
        })
        .unwrap();
    }

    #[test]
    fn test_core_event_services_signal_event_invokes_closure_using_trampoline() {
        crate::test_support::with_global_lock(|| {
            let service = CoreEventServices;
            let counter = Arc::new(AtomicUsize::new(0));
            let callback_counter = counter.clone();
            let callback: EventNotifyCallback = Box::new(move || {
                callback_counter.fetch_add(1, Ordering::SeqCst);
            });

            let event = service.create_event(Tpl::Callback, callback).unwrap();
            service.signal_event(event).unwrap();

            assert_eq!(counter.load(Ordering::SeqCst), 1);

            service.close_event(event).unwrap();
        })
        .unwrap();
    }

    #[test]
    fn test_core_event_services_create_event_for_group_signal_group_invokes_closure() {
        crate::test_support::with_global_lock(|| {
            let service = CoreEventServices;
            let counter = Arc::new(AtomicUsize::new(0));
            let callback_counter = counter.clone();
            let callback: EventNotifyCallback = Box::new(move || {
                callback_counter.fetch_add(1, Ordering::SeqCst);
            });

            let event = service.create_event_for_group(BinaryGuid::ZERO, Tpl::Callback, callback).unwrap();
            EVENT_DB.signal_group(BinaryGuid::ZERO.into_inner());

            assert_eq!(counter.load(Ordering::SeqCst), 1);

            service.close_event(event).unwrap();
        })
        .unwrap();
    }

    #[test]
    fn test_core_event_services_check_event_reflects_signaled_state_for_wait_event() {
        crate::test_support::with_global_lock(|| {
            let service = CoreEventServices;

            // The service can only create NOTIFY_SIGNAL events, so create a NOTIFY_WAIT event
            // directly through the event database to exercise check_event's other branches.
            let raw_event = EVENT_DB
                .create_event(efi::EVT_NOTIFY_WAIT, efi::TPL_NOTIFY, Some(noop_wait_notify), None, None)
                .unwrap();
            let event = Event::from_raw(raw_event).unwrap();

            assert_eq!(service.check_event(event), Ok(false));

            service.signal_event(event).unwrap();

            assert_eq!(service.check_event(event), Ok(true));

            service.close_event(event).unwrap();
        })
        .unwrap();
    }

    #[test]
    fn test_core_event_services_check_event_rejects_notify_signal_event() {
        crate::test_support::with_global_lock(|| {
            let service = CoreEventServices;
            let event = service.create_event(Tpl::Callback, Box::new(|| {})).unwrap();

            // check_event's UEFI semantics never accept a NOTIFY_SIGNAL event, which is the only
            // kind this service creates.
            assert_eq!(service.check_event(event), Err(EventError::InvalidParameter));

            service.close_event(event).unwrap();
        })
        .unwrap();
    }

    #[test]
    fn test_core_event_services_close_event_then_double_close_fails() {
        crate::test_support::with_global_lock(|| {
            let service = CoreEventServices;
            let event = service.create_event(Tpl::Callback, Box::new(|| {})).unwrap();

            assert_eq!(service.close_event(event), Ok(()));
            assert_eq!(service.close_event(event), Err(EventError::InvalidParameter));
        })
        .unwrap();
    }
}
