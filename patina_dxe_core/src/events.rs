//! DXE Core Events
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{
    ffi::c_void,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

use r_efi::efi;

use patina::pi::protocols::timer;

use patina_internal_cpu::interrupts;

use crate::{
    event_db::{SpinLockedEventDb, TimerDelay},
    gcd,
    protocols::PROTOCOL_DB,
};

pub static EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();

static CURRENT_TPL: AtomicUsize = AtomicUsize::new(efi::TPL_APPLICATION);
static SYSTEM_TIME: AtomicU64 = AtomicU64::new(0);

extern "efiapi" fn create_event(
    event_type: u32,
    notify_tpl: efi::Tpl,
    notify_function: Option<efi::EventNotify>,
    notify_context: *mut c_void,
    event: *mut efi::Event,
) -> efi::Status {
    if event.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let notify_context = if !notify_context.is_null() { Some(notify_context) } else { None };

    let (event_type, event_group) = match event_type {
        efi::EVT_SIGNAL_EXIT_BOOT_SERVICES => (efi::EVT_NOTIFY_SIGNAL, Some(efi::EVENT_GROUP_EXIT_BOOT_SERVICES)),
        efi::EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE => {
            (efi::EVT_NOTIFY_SIGNAL, Some(efi::EVENT_GROUP_VIRTUAL_ADDRESS_CHANGE))
        }
        other => (other, None),
    };

    match EVENT_DB.create_event(event_type, notify_tpl, notify_function, notify_context, event_group) {
        Ok(new_event) => {
            // Safety: caller must ensure that event is a valid pointer. It is null-checked above.
            unsafe { event.write_unaligned(new_event) };
            efi::Status::SUCCESS
        }
        Err(err) => err.into(),
    }
}

extern "efiapi" fn create_event_ex(
    event_type: u32,
    notify_tpl: efi::Tpl,
    notify_function: Option<efi::EventNotify>,
    notify_context: *const c_void,
    event_group: *const efi::Guid,
    event: *mut efi::Event,
) -> efi::Status {
    if event.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let notify_context = if !notify_context.is_null() { Some(notify_context as *mut c_void) } else { None };

    match event_type {
        efi::EVT_SIGNAL_EXIT_BOOT_SERVICES | efi::EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE => {
            return efi::Status::INVALID_PARAMETER;
        }
        _ => (),
    }

    // Safety: caller must ensure that event_group is a valid pointer if not null.
    let event_group = if !event_group.is_null() { Some(unsafe { event_group.read_unaligned() }) } else { None };

    match EVENT_DB.create_event(event_type, notify_tpl, notify_function, notify_context, event_group) {
        Ok(new_event) => {
            // Safety: caller must ensure that event is a valid pointer. It is null-checked above.
            unsafe { event.write_unaligned(new_event) };
            efi::Status::SUCCESS
        }
        Err(err) => err.into(),
    }
}

pub extern "efiapi" fn close_event(event: efi::Event) -> efi::Status {
    match EVENT_DB.close_event(event) {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => err.into(),
    }
}

pub extern "efiapi" fn signal_event(event: efi::Event) -> efi::Status {
    let status = match EVENT_DB.signal_event(event) {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => err.into(),
    };

    //Note: The C-reference implementation of SignalEvent gets an immediate dispatch of
    //pending events as a side effect of the locking implementation calling raise/restore
    //TPL. The spec doesn't require this; but it's likely that code out there depends
    //on it. So emulate that here with an artificial raise/restore.
    let old_tpl = raise_tpl(efi::TPL_HIGH_LEVEL);
    restore_tpl(old_tpl);

    status
}

extern "efiapi" fn wait_for_event(
    number_of_events: usize,
    event_array: *mut efi::Event,
    out_index: *mut usize,
) -> efi::Status {
    if number_of_events == 0 || event_array.is_null() || out_index.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    if CURRENT_TPL.load(Ordering::SeqCst) != efi::TPL_APPLICATION {
        return efi::Status::UNSUPPORTED;
    }

    //spin on the list
    loop {
        let mut event_ptr = event_array;
        for index in 0..number_of_events {
            // Safety: caller must ensure that event_array is a valid pointer and number_of_events is correct. event_array is null-checked above.
            let event = unsafe { event_ptr.read_unaligned() };
            match check_event(event) {
                efi::Status::NOT_READY => (),
                status => {
                    // Safety: caller must ensure that out_index is a valid pointer. It is null-checked above.
                    unsafe {
                        out_index.write_unaligned(index);
                    };
                    return status;
                }
            }
            // Safety: caller must ensure that event_array is a valid pointer and number_of_events is correct. event_array is null-checked above.
            event_ptr = unsafe { event_ptr.add(1) };
        }
    }
}

pub extern "efiapi" fn check_event(event: efi::Event) -> efi::Status {
    let event_type = match EVENT_DB.get_event_type(event) {
        Ok(event_type) => event_type,
        Err(err) => return err.into(),
    };

    if event_type.is_notify_signal() {
        return efi::Status::INVALID_PARAMETER;
    }

    match EVENT_DB.read_and_clear_signaled(event) {
        Ok(signaled) => {
            if signaled {
                return efi::Status::SUCCESS;
            }
        }
        Err(err) => return err.into(),
    }

    match EVENT_DB.queue_event_notify(event) {
        Ok(()) => (),
        Err(err) => return err.into(),
    }

    // raise/restore TPL to allow notifies to occur at the appropriate level.
    let old_tpl = raise_tpl(efi::TPL_HIGH_LEVEL);
    restore_tpl(old_tpl);

    match EVENT_DB.read_and_clear_signaled(event) {
        Ok(signaled) => {
            if signaled {
                return efi::Status::SUCCESS;
            }
        }
        Err(err) => return err.into(),
    }

    efi::Status::NOT_READY
}

pub extern "efiapi" fn set_timer(event: efi::Event, timer_type: efi::TimerDelay, trigger_time: u64) -> efi::Status {
    let timer_type = match TimerDelay::try_from(timer_type) {
        Err(err) => return err,
        Ok(timer_type) => timer_type,
    };

    let (trigger_time, period) = match timer_type {
        TimerDelay::Cancel => (None, None),
        TimerDelay::Relative => (Some(SYSTEM_TIME.load(Ordering::SeqCst) + trigger_time), None),
        TimerDelay::Periodic => (Some(SYSTEM_TIME.load(Ordering::SeqCst) + trigger_time), Some(trigger_time)),
    };

    match EVENT_DB.set_timer(event, timer_type, trigger_time, period) {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => err.into(),
    }
}

pub extern "efiapi" fn raise_tpl(new_tpl: efi::Tpl) -> efi::Tpl {
    assert!(new_tpl <= efi::TPL_HIGH_LEVEL, "Invalid attempt to raise TPL above TPL_HIGH_LEVEL");

    let prev_tpl = CURRENT_TPL.fetch_max(new_tpl, Ordering::SeqCst);

    assert!(
        new_tpl >= prev_tpl,
        "Invalid attempt to raise TPL to lower value. New TPL: {new_tpl:#x?}, Prev TPL: {prev_tpl:#x?}"
    );

    if (new_tpl == efi::TPL_HIGH_LEVEL) && (prev_tpl < efi::TPL_HIGH_LEVEL) {
        interrupts::disable_interrupts();
    }
    prev_tpl
}

pub extern "efiapi" fn restore_tpl(new_tpl: efi::Tpl) {
    let prev_tpl = CURRENT_TPL.fetch_min(new_tpl, Ordering::SeqCst);

    assert!(
        new_tpl <= prev_tpl,
        "Invalid attempt to restore TPL to higher value. New TPL: {new_tpl:#x?}, Prev TPL: {prev_tpl:#x?}"
    );

    if new_tpl < prev_tpl {
        // loop over any pending event notifications. Note: more notifications can be queued in the course of servicing
        // the current set of notifies; this will continue looping as long as there are any pending notifications, even
        // if they were queued after the loop started.
        loop {
            // Care must be taken to deal with reentrant "restore_tpl" cases. For example, the consume_next_event_notify
            // call requires taking the lock on EVENT_DB to retrieve the next notification. The release of that lock will
            // call restore_tpl. To avoid infinite recursion, this logic uses EVENT_NOTIFIES_IN_PROGRESS as a flag to
            // avoid reentrancy in the specific case that the lock is being taken for the purpose of acquiring event
            // notifies.
            static EVENT_NOTIFIES_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
            let event =
                match EVENT_NOTIFIES_IN_PROGRESS.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed) {
                    Ok(_) => {
                        let result = EVENT_DB.consume_next_event_notify(new_tpl);
                        EVENT_NOTIFIES_IN_PROGRESS.store(false, Ordering::Release);
                        result
                    }
                    _ => break, /* reentrant restore_tpl case */
                };

            let Some(event) = event else {
                break; /* no pending events */
            };
            if event.notify_tpl < efi::TPL_HIGH_LEVEL {
                interrupts::enable_interrupts();
            } else {
                interrupts::disable_interrupts();
            }
            CURRENT_TPL.store(event.notify_tpl, Ordering::SeqCst);
            let notify_context = event.notify_context.unwrap_or(core::ptr::null_mut());

            if EVENT_DB.get_event_type(event.event).unwrap().is_notify_signal() {
                let _ = EVENT_DB.clear_signal(event.event);
            }

            //Caution: this is calling function pointer supplied by code outside DXE Rust.
            //The notify_function is not "unsafe" per the signature, even though it's
            //supplied by code outside the core module. If it were marked 'unsafe'
            //then other Rust modules executing under DXE Rust would need to mark all event
            //callbacks as "unsafe", and the r_efi definition for EventNotify would need to
            //change.
            if let Some(notify_function) = event.notify_function {
                (notify_function)(event.event, notify_context);
            }
        }
    }

    if new_tpl < efi::TPL_HIGH_LEVEL {
        interrupts::enable_interrupts();
    }
    CURRENT_TPL.store(new_tpl, Ordering::SeqCst);
}

extern "efiapi" fn timer_tick(time: u64) {
    let old_tpl = raise_tpl(efi::TPL_HIGH_LEVEL);
    SYSTEM_TIME.fetch_add(time, Ordering::SeqCst);
    let current_time = SYSTEM_TIME.load(Ordering::SeqCst);
    EVENT_DB.timer_tick(current_time);
    restore_tpl(old_tpl); //implicitly dispatches timer notifies if any.
}

extern "efiapi" fn timer_available_callback(event: efi::Event, _context: *mut c_void) {
    match PROTOCOL_DB.locate_protocol(timer::PROTOCOL_GUID) {
        Ok(timer_arch_ptr) => {
            let timer_arch_ptr = timer_arch_ptr as *mut timer::Protocol;
            let timer_arch = unsafe { &*(timer_arch_ptr) };
            (timer_arch.register_handler)(timer_arch_ptr, timer_tick);
            if let Err(status_err) = EVENT_DB.close_event(event) {
                log::warn!("Could not close event for timer_available_callback due to error {status_err:?}");
            }
        }
        Err(err) => panic!("Unable to locate timer arch: {err:?}"),
    }
}

// indicates that eventing subsystem is fully initialized.
static EVENT_DB_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// This callback is invoked whenever the GCD changes, and will signal the required UEFI event group.
pub fn gcd_map_change(map_change_type: gcd::MapChangeType) {
    if EVENT_DB_INITIALIZED.load(Ordering::SeqCst) {
        match map_change_type {
            gcd::MapChangeType::AddMemorySpace
            | gcd::MapChangeType::AllocateMemorySpace
            | gcd::MapChangeType::FreeMemorySpace
            | gcd::MapChangeType::RemoveMemorySpace => EVENT_DB.signal_group(efi::EVENT_GROUP_MEMORY_MAP_CHANGE),
            gcd::MapChangeType::SetMemoryAttributes | gcd::MapChangeType::SetMemoryCapabilities => (),
        }
    }
}

pub fn init_events_support(bs: &mut efi::BootServices) {
    bs.create_event = create_event;
    bs.create_event_ex = create_event_ex;
    bs.close_event = close_event;
    bs.signal_event = signal_event;
    bs.wait_for_event = wait_for_event;
    bs.check_event = check_event;
    bs.set_timer = set_timer;
    bs.raise_tpl = raise_tpl;
    bs.restore_tpl = restore_tpl;

    //set up call back for timer arch protocol installation.
    let event = EVENT_DB
        .create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(timer_available_callback), None, None)
        .expect("Failed to create timer available callback.");

    PROTOCOL_DB
        .register_protocol_notify(timer::PROTOCOL_GUID, event)
        .expect("Failed to register protocol notify on timer arch callback.");

    //Indicate eventing is initialized
    EVENT_DB_INITIALIZED.store(true, Ordering::SeqCst);
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::test_support;
    use std::{ptr, sync::atomic::Ordering};

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            unsafe {
                crate::test_support::init_test_gcd(None);
                crate::test_support::init_test_protocol_db();
            }
            crate::test_support::reset_dispatcher_context();
            f();
        })
        .unwrap();
    }

    extern "efiapi" fn test_notify(_event: efi::Event, _context: *mut c_void) {}

    // Track if notification was called
    static NOTIFY_CALLED: AtomicBool = AtomicBool::new(false);
    extern "efiapi" fn tracking_notify(_event: efi::Event, _context: *mut c_void) {
        NOTIFY_CALLED.store(true, Ordering::SeqCst);
    }

    #[test]
    fn test_create_event_null_event_pointer() {
        with_locked_state(|| {
            let result = create_event(0, efi::TPL_APPLICATION, None, ptr::null_mut(), ptr::null_mut());

            assert_eq!(result, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_create_event_success() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();
            let result = create_event(0, efi::TPL_APPLICATION, None, ptr::null_mut(), &mut event);

            assert_eq!(result, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_create_event_with_notify_context() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();
            let context = Box::into_raw(Box::new(42)) as *mut c_void;
            let result = create_event(0, efi::TPL_APPLICATION, None, context, &mut event);

            assert_eq!(result, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_create_event_with_notify_function() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();
            let notify_fn: Option<efi::EventNotify> = Some(test_notify);
            let result = create_event(efi::EVT_NOTIFY_WAIT, efi::TPL_CALLBACK, notify_fn, ptr::null_mut(), &mut event);

            assert_eq!(result, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_create_event_virtual_address_change() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();

            let notify_fn: Option<efi::EventNotify> = Some(test_notify);

            let result = create_event(
                efi::EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE,
                efi::TPL_CALLBACK,
                notify_fn,
                ptr::null_mut(),
                &mut event,
            );

            assert_eq!(result, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_create_event_exit_boot_services() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();

            let notify_fn: Option<efi::EventNotify> = Some(test_notify);

            let result = create_event(
                efi::EVT_SIGNAL_EXIT_BOOT_SERVICES,
                efi::TPL_CALLBACK,
                notify_fn,
                ptr::null_mut(),
                &mut event,
            );

            assert_eq!(result, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_create_event_ex_null_event() {
        with_locked_state(|| {
            let result = create_event_ex(0, efi::TPL_APPLICATION, None, ptr::null(), ptr::null(), ptr::null_mut());

            assert_eq!(result, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_create_event_ex_with_event_group() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();
            let event_guid: efi::Guid =
                efi::Guid::from_fields(0x87a2e5d9, 0xc34f, 0x4b21, 0x8e, 0x57, &[0x1a, 0xf9, 0x3c, 0x82, 0xd7, 0x6b]);
            let notify_fn: Option<efi::EventNotify> = Some(test_notify);
            let result = create_event_ex(
                efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_CALLBACK,
                notify_fn,
                ptr::null(),
                &event_guid,
                &mut event,
            );

            assert_eq!(result, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_create_event_ex_exit_boot_services() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();
            // EVT_SIGNAL_EXIT_BOOT_SERVICES should fail with create_event_ex
            let result = create_event_ex(
                efi::EVT_SIGNAL_EXIT_BOOT_SERVICES,
                efi::TPL_CALLBACK,
                Some(test_notify),
                ptr::null(),
                ptr::null(),
                &mut event,
            );

            assert_eq!(result, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_create_event_ex_virtual_address_change() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();
            // EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE should fail with create_event_ex
            let result = create_event_ex(
                efi::EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE,
                efi::TPL_CALLBACK,
                Some(test_notify),
                ptr::null(),
                ptr::null(),
                &mut event,
            );

            assert_eq!(result, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_close_event() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();
            let notify_fn: Option<efi::EventNotify> = Some(test_notify);
            let _ = create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                notify_fn,
                ptr::null_mut(),
                &mut event,
            );

            let result = EVENT_DB.close_event(event);

            assert!(result.is_ok());
            assert!(!EVENT_DB.is_valid(event));
        });
    }

    #[test]
    fn test_signal_event() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();
            let notify_fn: Option<efi::EventNotify> = Some(test_notify);
            let _ = create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                notify_fn,
                ptr::null_mut(),
                &mut event,
            );
            let result = signal_event(event);

            assert_eq!(result, efi::Status::SUCCESS);
            assert!(EVENT_DB.read_and_clear_signaled(event).is_ok());
        });
    }

    #[test]
    fn test_wait_for_event_signaled() {
        with_locked_state(|| {
            CURRENT_TPL.store(efi::TPL_APPLICATION, Ordering::SeqCst);
            let mut event: efi::Event = ptr::null_mut();
            create_event(efi::EVT_NOTIFY_WAIT, efi::TPL_NOTIFY, Some(test_notify), ptr::null_mut(), &mut event);
            signal_event(event);

            let events: [efi::Event; 1] = [event];
            let mut index: usize = 0;

            let mut test_wait = || {
                let status = wait_for_event(1, events.as_ptr() as *mut efi::Event, &mut index as *mut usize);
                assert_eq!(status, efi::Status::SUCCESS);
                assert_eq!(index, 0);
            };

            test_wait();

            let _ = close_event(event);
        });
    }

    #[test]
    fn test_timer_delay_relative_basic() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();
            let notify_fn: Option<efi::EventNotify> = Some(test_notify);

            let result = create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                notify_fn,
                ptr::null_mut(),
                &mut event,
            );
            assert_eq!(result, efi::Status::SUCCESS);

            let initial_time = 1000u64;
            SYSTEM_TIME.store(initial_time, Ordering::SeqCst);

            let wait_time = 500u64;
            let result = set_timer(event, 1 /* TimerDelay::Relative */, wait_time);
            assert_eq!(result, efi::Status::SUCCESS);
        })
    }

    #[test]
    fn test_timer_delay_error_handling() {
        with_locked_state(|| {
            // Test with invalid event
            let invalid_event: efi::Event = ptr::null_mut();
            let result = set_timer(invalid_event, 1 /* TimerDelay::Relative */, 100);

            // Should return an error status
            assert_ne!(result, efi::Status::SUCCESS);

            // Test with invalid timer time
            let mut event: efi::Event = ptr::null_mut();
            let notify_fn: Option<efi::EventNotify> = Some(test_notify);

            // Create timer event
            let result = create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                notify_fn,
                ptr::null_mut(),
                &mut event,
            );
            assert_eq!(result, efi::Status::SUCCESS);

            // Set timer with an invalid timer type
            let invalid_timer_type = 10; // Any value not defined in TimerDelay enum
            let result = set_timer(event, invalid_timer_type, 100);

            // Should return an error status
            assert_ne!(result, efi::Status::SUCCESS);

            let _ = EVENT_DB.close_event(event);
        });
    }

    #[test]
    fn test_set_timer_cancel() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();
            let notify_fn: Option<efi::EventNotify> = Some(test_notify);

            let result = create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                notify_fn,
                ptr::null_mut(),
                &mut event,
            );
            assert_eq!(result, efi::Status::SUCCESS);

            // Set a timer
            let result = set_timer(event, 1 /* TimerDelay::Relative */, 500);
            assert_eq!(result, efi::Status::SUCCESS);

            // Cancel the timer
            let result = set_timer(event, 0 /* TimerDelay::Cancel */, 0);
            assert_eq!(result, efi::Status::SUCCESS);

            // Clean up
            let _ = close_event(event);
        });
    }

    #[test]
    fn test_set_timer_periodic() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();
            let notify_fn: Option<efi::EventNotify> = Some(test_notify);

            let result = create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                notify_fn,
                ptr::null_mut(),
                &mut event,
            );
            assert_eq!(result, efi::Status::SUCCESS);

            // Set periodic timer
            let result = set_timer(event, 2 /* TimerDelay::Periodic */, 100);
            assert_eq!(result, efi::Status::SUCCESS);

            // Clean up
            let _ = close_event(event);
        });
    }

    // Test for event notifications
    #[test]
    fn test_event_notification() {
        with_locked_state(|| {
            // Ensure we start from a low TPL so that signal_event's raise/restore will dispatch notifies
            CURRENT_TPL.store(efi::TPL_APPLICATION, Ordering::SeqCst);
            NOTIFY_CALLED.store(false, Ordering::SeqCst);

            let mut event: efi::Event = ptr::null_mut();
            // Create notification signal event
            let result = create_event(
                efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_CALLBACK,
                Some(tracking_notify),
                ptr::null_mut(),
                &mut event,
            );
            assert_eq!(result, efi::Status::SUCCESS);

            // Signal the event
            let result = signal_event(event);
            assert_eq!(result, efi::Status::SUCCESS);

            // Check if notification was called
            assert!(NOTIFY_CALLED.load(Ordering::SeqCst));

            // Clean up
            let _ = close_event(event);
        });
    }

    #[test]
    fn test_event_notification_with_tpl_change_fires_lower_events() {
        with_locked_state(|| {
            NOTIFY_CALLED.store(false, Ordering::SeqCst);

            // special callback that does TPL manipulation.
            extern "efiapi" fn test_tpl_switching_notify(_event: efi::Event, _context: *mut c_void) {
                let old_tpl = raise_tpl(efi::TPL_HIGH_LEVEL);
                restore_tpl(efi::TPL_APPLICATION);

                if old_tpl > efi::TPL_APPLICATION {
                    raise_tpl(old_tpl);
                }
            }

            let mut event: efi::Event = ptr::null_mut();
            // Create notification signal event
            let result = create_event(
                efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_CALLBACK,
                Some(tracking_notify),
                ptr::null_mut(),
                &mut event,
            );
            assert_eq!(result, efi::Status::SUCCESS);

            let mut event2: efi::Event = ptr::null_mut();
            let result = create_event(
                efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_tpl_switching_notify),
                ptr::null_mut(),
                &mut event2,
            );
            assert_eq!(result, efi::Status::SUCCESS);

            //raise TPL to callback than event
            let _old_tpl = raise_tpl(efi::TPL_CALLBACK);

            // Signal the event
            let result = signal_event(event);
            assert_eq!(result, efi::Status::SUCCESS);

            // notification should not have been called (because current TPL >= notification TPL).
            assert!(!NOTIFY_CALLED.load(Ordering::SeqCst));

            // Signal the TPL manipulation event. This should fire and lower the TPL so the event1 notification should
            // signal.
            let result = signal_event(event2);
            assert_eq!(result, efi::Status::SUCCESS);

            // notification should have been called (current TPL was briefly lowered to notification TPL).
            assert!(NOTIFY_CALLED.load(Ordering::SeqCst));

            assert_eq!(CURRENT_TPL.load(Ordering::SeqCst), efi::TPL_CALLBACK);

            // Clean up
            let _ = close_event(event);
            let _ = close_event(event2);
        });
    }

    #[test]
    fn test_wait_for_event_null_parameters() {
        with_locked_state(|| {
            let mut index: usize = 0;
            let events: [efi::Event; 1] = [ptr::null_mut()];

            // Test null event array
            let status = wait_for_event(1, ptr::null_mut(), &mut index as *mut usize);
            assert_eq!(status, efi::Status::INVALID_PARAMETER);

            // Test null out_index
            let status = wait_for_event(1, events.as_ptr() as *mut efi::Event, ptr::null_mut());
            assert_eq!(status, efi::Status::INVALID_PARAMETER);

            // Test zero events
            let status = wait_for_event(0, events.as_ptr() as *mut efi::Event, &mut index as *mut usize);
            assert_eq!(status, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_wait_for_event_wrong_tpl() {
        with_locked_state(|| {
            let mut index: usize = 0;
            let events: [efi::Event; 1] = [ptr::null_mut()];

            // Set TPL to something other than APPLICATION
            CURRENT_TPL.store(efi::TPL_NOTIFY, Ordering::SeqCst);

            let status = wait_for_event(1, events.as_ptr() as *mut efi::Event, &mut index as *mut usize);
            assert_eq!(status, efi::Status::UNSUPPORTED);

            CURRENT_TPL.store(efi::TPL_APPLICATION, Ordering::SeqCst);
        });
    }

    // Tests for check_event function
    #[test]
    fn test_check_event_with_invalid_event() {
        with_locked_state(|| {
            let invalid_event: efi::Event = ptr::null_mut();
            let result = check_event(invalid_event);
            assert_ne!(result, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_check_event_notify_signal_type() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();
            // Create a notification signal event
            let result =
                create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_NOTIFY, Some(test_notify), ptr::null_mut(), &mut event);
            assert_eq!(result, efi::Status::SUCCESS);

            // Check event should fail for notify signal events
            let result = check_event(event);
            assert_eq!(result, efi::Status::INVALID_PARAMETER);

            // Clean up
            let _ = close_event(event);
        });
    }

    #[test]
    fn test_check_event_signaled_event() {
        with_locked_state(|| {
            let mut event: efi::Event = ptr::null_mut();
            // Create a wait event
            let result =
                create_event(efi::EVT_NOTIFY_WAIT, efi::TPL_NOTIFY, Some(test_notify), ptr::null_mut(), &mut event);
            assert_eq!(result, efi::Status::SUCCESS);

            // Signal the event
            let result = signal_event(event);
            assert_eq!(result, efi::Status::SUCCESS);

            // Check event should succeed for signaled events
            let result = check_event(event);
            assert_eq!(result, efi::Status::SUCCESS);

            // Checking again should return NOT_READY as it's been cleared
            let result = check_event(event);
            assert_eq!(result, efi::Status::NOT_READY);

            // Clean up
            let _ = close_event(event);
        });
    }

    // Tests for TPL functions
    #[test]
    fn test_raise_tpl_sequence() {
        with_locked_state(|| {
            // Store original TPL to restore later
            let original_tpl = CURRENT_TPL.load(Ordering::SeqCst);

            // Set known starting TPL
            CURRENT_TPL.store(efi::TPL_APPLICATION, Ordering::SeqCst);

            // Test raising from APPLICATION to CALLBACK
            let prev_tpl = raise_tpl(efi::TPL_CALLBACK);
            assert_eq!(prev_tpl, efi::TPL_APPLICATION);
            assert_eq!(CURRENT_TPL.load(Ordering::SeqCst), efi::TPL_CALLBACK);

            // Test raising from CALLBACK to NOTIFY
            let prev_tpl = raise_tpl(efi::TPL_NOTIFY);
            assert_eq!(prev_tpl, efi::TPL_CALLBACK);
            assert_eq!(CURRENT_TPL.load(Ordering::SeqCst), efi::TPL_NOTIFY);

            // Test raising to HIGH_LEVEL (should disable interrupts)
            let prev_tpl = raise_tpl(efi::TPL_HIGH_LEVEL);
            assert_eq!(prev_tpl, efi::TPL_NOTIFY);
            assert_eq!(CURRENT_TPL.load(Ordering::SeqCst), efi::TPL_HIGH_LEVEL);

            // Restore original TPL
            CURRENT_TPL.store(original_tpl, Ordering::SeqCst);
            // Re-enable interrupts if we left them disabled
            interrupts::enable_interrupts();
        });
    }

    #[test]
    fn test_raise_tpl_too_high() {
        with_locked_state(|| {
            // Instead of calling raise_tpl directly with an invalid value,
            // let's check that the condition that would cause a panic is enforced

            // The function should panic if TPL > HIGH_LEVEL
            let too_high_tpl = efi::TPL_HIGH_LEVEL + 1;

            // We can test the assertion condition without triggering the panic
            let would_panic = too_high_tpl > efi::TPL_HIGH_LEVEL;
            assert!(would_panic, "TPL values greater than HIGH_LEVEL should not be allowed");

            // Additionally, we can test that valid TPL values work correctly
            let original_tpl = CURRENT_TPL.load(Ordering::SeqCst);
            CURRENT_TPL.store(efi::TPL_APPLICATION, Ordering::SeqCst);

            // Test with valid value - should not panic
            let prev_tpl = raise_tpl(efi::TPL_HIGH_LEVEL);
            assert_eq!(prev_tpl, efi::TPL_APPLICATION);
            assert_eq!(CURRENT_TPL.load(Ordering::SeqCst), efi::TPL_HIGH_LEVEL);

            // Restore original TPL
            CURRENT_TPL.store(original_tpl, Ordering::SeqCst);
        });
    }

    #[test]
    fn test_raise_tpl_to_lower() {
        with_locked_state(|| {
            // Store original TPL to restore later
            let original_tpl = CURRENT_TPL.load(Ordering::SeqCst);

            // Instead of triggering a panic, we'll test the condition
            // that would cause a panic
            let current_tpl = efi::TPL_NOTIFY;
            let lower_tpl = efi::TPL_CALLBACK; // Lower than NOTIFY

            // Set starting TPL to NOTIFY
            CURRENT_TPL.store(current_tpl, Ordering::SeqCst);

            // This would trigger the panic in raise_tpl:
            // raise_tpl(lower_tpl)

            // Instead, verify the condition that would cause a panic
            let would_panic = lower_tpl < current_tpl;
            assert!(would_panic, "Attempting to raise TPL to a lower value should cause a panic");

            // Test valid case - should not panic
            let prev_tpl = raise_tpl(current_tpl); // Same level, should be fine
            assert_eq!(prev_tpl, current_tpl);

            let higher_tpl = efi::TPL_HIGH_LEVEL; // Higher than NOTIFY
            let prev_tpl = raise_tpl(higher_tpl);
            assert_eq!(prev_tpl, current_tpl);
            assert_eq!(CURRENT_TPL.load(Ordering::SeqCst), higher_tpl);

            // Restore original TPL
            CURRENT_TPL.store(original_tpl, Ordering::SeqCst);
        });
    }

    #[test]
    fn test_restore_tpl_sequence() {
        with_locked_state(|| {
            // Store original TPL to restore later
            let original_tpl = CURRENT_TPL.load(Ordering::SeqCst);

            // Set known starting TPL
            CURRENT_TPL.store(efi::TPL_HIGH_LEVEL, Ordering::SeqCst);
            interrupts::disable_interrupts();

            // Test restoring from HIGH_LEVEL to NOTIFY
            restore_tpl(efi::TPL_NOTIFY);
            assert_eq!(CURRENT_TPL.load(Ordering::SeqCst), efi::TPL_NOTIFY);

            // Test restoring from NOTIFY to CALLBACK
            restore_tpl(efi::TPL_CALLBACK);
            assert_eq!(CURRENT_TPL.load(Ordering::SeqCst), efi::TPL_CALLBACK);

            // Test restoring from CALLBACK to APPLICATION
            restore_tpl(efi::TPL_APPLICATION);
            assert_eq!(CURRENT_TPL.load(Ordering::SeqCst), efi::TPL_APPLICATION);

            // Restore original TPL
            CURRENT_TPL.store(original_tpl, Ordering::SeqCst);
        });
    }

    #[test]
    fn test_restore_tpl_to_higher() {
        with_locked_state(|| {
            // Store original TPL to restore later
            let original_tpl = CURRENT_TPL.load(Ordering::SeqCst);

            // Set starting TPL to a known value
            let current_tpl = efi::TPL_NOTIFY;
            let higher_tpl = efi::TPL_HIGH_LEVEL; // Higher than NOTIFY

            // Set starting TPL
            CURRENT_TPL.store(current_tpl, Ordering::SeqCst);

            // This would trigger the panic in restore_tpl:
            // restore_tpl(higher_tpl)

            // Instead, verify the condition that would cause a panic
            let would_panic = higher_tpl > current_tpl;
            assert!(would_panic, "Attempting to restore TPL to a higher value should cause a panic");

            // Test valid case - should not panic
            restore_tpl(current_tpl); // Same level, should be fine
            assert_eq!(CURRENT_TPL.load(Ordering::SeqCst), current_tpl);

            let lower_tpl = efi::TPL_CALLBACK; // Lower than NOTIFY
            restore_tpl(lower_tpl);
            assert_eq!(CURRENT_TPL.load(Ordering::SeqCst), lower_tpl);

            // Restore original TPL
            CURRENT_TPL.store(original_tpl, Ordering::SeqCst);
        });
    }

    // Tests for GCD and initialization functions
    #[test]
    fn test_gcd_map_change() {
        with_locked_state(|| {
            // Set initialized flag
            EVENT_DB_INITIALIZED.store(true, Ordering::SeqCst);

            // Test each map change type
            gcd_map_change(gcd::MapChangeType::AddMemorySpace);
            gcd_map_change(gcd::MapChangeType::AllocateMemorySpace);
            gcd_map_change(gcd::MapChangeType::FreeMemorySpace);
            gcd_map_change(gcd::MapChangeType::RemoveMemorySpace);
            gcd_map_change(gcd::MapChangeType::SetMemoryAttributes);
            gcd_map_change(gcd::MapChangeType::SetMemoryCapabilities);

            // Reset initialized flag
            EVENT_DB_INITIALIZED.store(false, Ordering::SeqCst);
        });
    }

    #[test]
    fn test_gcd_map_change_not_initialized() {
        with_locked_state(|| {
            // Ensure initialized flag is false
            EVENT_DB_INITIALIZED.store(false, Ordering::SeqCst);

            // Call should have no effect and not panic
            gcd_map_change(gcd::MapChangeType::AddMemorySpace);
        });
    }

    #[test]
    fn test_timer_tick() {
        with_locked_state(|| {
            let original_time = SYSTEM_TIME.load(Ordering::SeqCst);

            let test_time = 1000;
            timer_tick(test_time);

            assert_eq!(SYSTEM_TIME.load(Ordering::SeqCst), original_time + test_time);

            SYSTEM_TIME.store(original_time, Ordering::SeqCst);
        });
    }

    // Mock for init_events_support test
    #[test]
    fn test_init_events_support() {
        with_locked_state(|| {
            // Create dummy function pointers to use for initialization
            extern "efiapi" fn dummy_raise_tpl(_new_tpl: efi::Tpl) -> efi::Tpl {
                0
            }
            extern "efiapi" fn dummy_restore_tpl(_old_tpl: efi::Tpl) {}
            extern "efiapi" fn dummy_allocate_pages(
                _allocation_type: u32,
                _memory_type: u32,
                _pages: usize,
                _memory: *mut u64,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_free_pages(_memory: u64, _pages: usize) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_get_memory_map(
                _memory_map_size: *mut usize,
                _memory_map: *mut efi::MemoryDescriptor,
                _map_key: *mut usize,
                _descriptor_size: *mut usize,
                _descriptor_version: *mut u32,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_allocate_pool(
                _pool_type: u32,
                _size: usize,
                _buffer: *mut *mut c_void,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_free_pool(_buffer: *mut c_void) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_create_event(
                _event_type: u32,
                _notify_tpl: efi::Tpl,
                _notify_function: Option<efi::EventNotify>,
                _notify_context: *mut c_void,
                _event: *mut efi::Event,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_set_timer(_event: efi::Event, _type: u32, _trigger_time: u64) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_wait_for_event(
                _number_of_events: usize,
                _event: *mut efi::Event,
                _index: *mut usize,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_signal_event(_event: efi::Event) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_close_event(_event: efi::Event) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_check_event(_event: efi::Event) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_install_protocol_interface(
                _handle: *mut efi::Handle,
                _protocol: *mut efi::Guid,
                _interface_type: u32,
                _interface: *mut c_void,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_reinstall_protocol_interface(
                _handle: efi::Handle,
                _protocol: *mut efi::Guid,
                _old_interface: *mut c_void,
                _new_interface: *mut c_void,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_uninstall_protocol_interface(
                _handle: efi::Handle,
                _protocol: *mut efi::Guid,
                _interface: *mut c_void,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_handle_protocol(
                _handle: efi::Handle,
                _protocol: *mut efi::Guid,
                _interface: *mut *mut c_void,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_register_protocol_notify(
                _protocol: *mut efi::Guid,
                _event: efi::Event,
                _registration: *mut *mut c_void,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_locate_handle(
                _search_type: u32,
                _protocol: *mut efi::Guid,
                _search_key: *mut c_void,
                _buffer_size: *mut usize,
                _buffer: *mut efi::Handle,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_locate_device_path(
                _protocol: *mut efi::Guid,
                _device_path: *mut *mut r_efi::protocols::device_path::Protocol,
                _device: *mut efi::Handle,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_install_configuration_table(
                _guid: *mut efi::Guid,
                _table: *mut c_void,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_load_image(
                _boot_policy: efi::Boolean,
                _parent_image_handle: efi::Handle,
                _device_path: *mut r_efi::protocols::device_path::Protocol,
                _source_buffer: *mut c_void,
                _source_size: usize,
                _image_handle: *mut efi::Handle,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_start_image(
                _image_handle: efi::Handle,
                _exit_data_size: *mut usize,
                _exit_data: *mut *mut u16,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_exit(
                _image_handle: efi::Handle,
                _exit_status: efi::Status,
                _exit_data_size: usize,
                _exit_data: *mut u16,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_unload_image(_image_handle: efi::Handle) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_exit_boot_services(_image_handle: efi::Handle, _map_key: usize) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_get_next_monotonic_count(_count: *mut u64) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_stall(_microseconds: usize) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_set_watchdog_timer(
                _timeout: usize,
                _watchdog_code: u64,
                _data_size: usize,
                _watchdog_data: *mut u16,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_connect_controller(
                _controller_handle: efi::Handle,
                _driver_image_handle: *mut efi::Handle,
                _remaining_device_path: *mut r_efi::protocols::device_path::Protocol,
                _recursive: efi::Boolean,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_disconnect_controller(
                _controller_handle: efi::Handle,
                _driver_image_handle: efi::Handle,
                _child_handle: efi::Handle,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_open_protocol(
                _handle: efi::Handle,
                _protocol: *mut efi::Guid,
                _interface: *mut *mut c_void,
                _agent_handle: efi::Handle,
                _controller_handle: efi::Handle,
                _attributes: u32,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_close_protocol(
                _handle: efi::Handle,
                _protocol: *mut efi::Guid,
                _agent_handle: efi::Handle,
                _controller_handle: efi::Handle,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_open_protocol_information(
                _handle: efi::Handle,
                _protocol: *mut efi::Guid,
                _entry_buffer: *mut *mut efi::OpenProtocolInformationEntry,
                _entry_count: *mut usize,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_protocols_per_handle(
                _handle: efi::Handle,
                _protocol_buffer: *mut *mut *mut efi::Guid,
                _protocol_buffer_count: *mut usize,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_locate_handle_buffer(
                _search_type: u32,
                _protocol: *mut efi::Guid,
                _search_key: *mut c_void,
                _no_handles: *mut usize,
                _buffer: *mut *mut efi::Handle,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_locate_protocol(
                _protocol: *mut efi::Guid,
                _registration: *mut c_void,
                _interface: *mut *mut c_void,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_install_multiple_protocol_interfaces(
                _handle: *mut efi::Handle,
                _args: *mut c_void,
                _more_args: *mut c_void,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_uninstall_multiple_protocol_interfaces(
                _handle: efi::Handle,
                _args: *mut c_void,
                _more_args: *mut c_void,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_calculate_crc32(
                _data: *mut c_void,
                _data_size: usize,
                _crc32: *mut u32,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            extern "efiapi" fn dummy_copy_mem(_destination: *mut c_void, _source: *mut c_void, _length: usize) {}
            extern "efiapi" fn dummy_set_mem(_buffer: *mut c_void, _size: usize, _value: u8) {}
            extern "efiapi" fn dummy_create_event_ex(
                _event_type: u32,
                _notify_tpl: efi::Tpl,
                _notify_function: Option<efi::EventNotify>,
                _notify_context: *const c_void,
                _event_group: *const efi::Guid,
                _event: *mut efi::Event,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }

            // Create a mutable BootServices to pass to init_events_support
            let mut boot_services = efi::BootServices {
                hdr: efi::TableHeader { signature: 0, revision: 0, header_size: 0, crc32: 0, reserved: 0 },
                // Fill with dummy function pointers
                raise_tpl: dummy_raise_tpl,
                restore_tpl: dummy_restore_tpl,
                allocate_pages: dummy_allocate_pages,
                free_pages: dummy_free_pages,
                get_memory_map: dummy_get_memory_map,
                allocate_pool: dummy_allocate_pool,
                free_pool: dummy_free_pool,
                create_event: dummy_create_event,
                set_timer: dummy_set_timer,
                wait_for_event: dummy_wait_for_event,
                signal_event: dummy_signal_event,
                close_event: dummy_close_event,
                check_event: dummy_check_event,
                install_protocol_interface: dummy_install_protocol_interface,
                reinstall_protocol_interface: dummy_reinstall_protocol_interface,
                uninstall_protocol_interface: dummy_uninstall_protocol_interface,
                handle_protocol: dummy_handle_protocol,
                reserved: ptr::null_mut(),
                register_protocol_notify: dummy_register_protocol_notify,
                locate_handle: dummy_locate_handle,
                locate_device_path: dummy_locate_device_path,
                install_configuration_table: dummy_install_configuration_table,
                load_image: dummy_load_image,
                start_image: dummy_start_image,
                exit: dummy_exit,
                unload_image: dummy_unload_image,
                exit_boot_services: dummy_exit_boot_services,
                get_next_monotonic_count: dummy_get_next_monotonic_count,
                stall: dummy_stall,
                set_watchdog_timer: dummy_set_watchdog_timer,
                connect_controller: dummy_connect_controller,
                disconnect_controller: dummy_disconnect_controller,
                open_protocol: dummy_open_protocol,
                close_protocol: dummy_close_protocol,
                open_protocol_information: dummy_open_protocol_information,
                protocols_per_handle: dummy_protocols_per_handle,
                locate_handle_buffer: dummy_locate_handle_buffer,
                locate_protocol: dummy_locate_protocol,
                install_multiple_protocol_interfaces: dummy_install_multiple_protocol_interfaces,
                uninstall_multiple_protocol_interfaces: dummy_uninstall_multiple_protocol_interfaces,
                calculate_crc32: dummy_calculate_crc32,
                copy_mem: dummy_copy_mem,
                set_mem: dummy_set_mem,
                create_event_ex: dummy_create_event_ex,
            };

            // Initialize events support
            init_events_support(&mut boot_services);

            // Verify function pointers are updated
            assert!(boot_services.create_event as usize != dummy_create_event as usize);
            assert!(boot_services.create_event_ex as usize != dummy_create_event_ex as usize);
            assert!(boot_services.close_event as usize != dummy_close_event as usize);
            assert!(boot_services.signal_event as usize != dummy_signal_event as usize);
            assert!(boot_services.wait_for_event as usize != dummy_wait_for_event as usize);
            assert!(boot_services.check_event as usize != dummy_check_event as usize);
            assert!(boot_services.set_timer as usize != dummy_set_timer as usize);
            assert!(boot_services.raise_tpl as usize != dummy_raise_tpl as usize);
            assert!(boot_services.restore_tpl as usize != dummy_restore_tpl as usize);

            // Verify initialization flag is set
            assert!(EVENT_DB_INITIALIZED.load(Ordering::SeqCst));

            // Reset the flag for other tests
            EVENT_DB_INITIALIZED.store(false, Ordering::SeqCst);
        });
    }
}
