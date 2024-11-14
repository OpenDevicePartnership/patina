extern crate alloc;

use r_efi::efi;
use core::ffi::c_void;
use core::sync::atomic::Ordering;
use crate::{event_db::{TimerDelay, EventDb}, protocol_db::ProtocolDb};
use mu_pi::protocols::{cpu_arch, timer};
use crate::uefi_gcd::gcd;
use super::SystemState;

#[inline(always)]
pub fn create_event(
    event_db: &mut EventDb,
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

    match event_db.create_event(event_type, notify_tpl, notify_function, notify_context, event_group) {
        Ok(new_event) => {
            unsafe { *event = new_event };
            efi::Status::SUCCESS
        }
        Err(err) => err,
    }
}

#[inline(always)]
pub fn create_event_ex(
    event_db: &mut EventDb,
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
            return efi::Status::INVALID_PARAMETER
        }
        _ => (),
    }

    let event_group = if !event_group.is_null() { Some(unsafe { *event_group }) } else { None };

    match event_db.create_event(event_type, notify_tpl, notify_function, notify_context, event_group) {
        Ok(new_event) => {
            unsafe { *event = new_event };
            efi::Status::SUCCESS
        }
        Err(err) => err,
    }
}

#[inline(always)]
pub fn close_event(event_db: &mut EventDb, event: efi::Event) -> efi::Status {
    match event_db.close_event(event) {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => err,
    }
}

#[inline(always)]
pub fn signal_event(event_db: &mut EventDb, st: &SystemState, event: efi::Event) -> efi::Status {
    let status = match event_db.signal_event(event) {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => err,
    };

    //Note: The C-reference implementation of SignalEvent gets an immediate dispatch of
    //pending events as a side effect of the locking implementation calling raise/restore
    //TPL. The spec doesn't require this; but it's likely that code out there depends
    //on it. So emulate that here with an artificial raise/restore.
    let old_tpl = raise_tpl(st, efi::TPL_HIGH_LEVEL);
    restore_tpl(event_db, st, old_tpl);

    status
}

#[inline(always)]
pub fn wait_for_event(
    event_db: &mut EventDb,
    st: &SystemState,
    number_of_events: usize,
    event_array: *mut efi::Event,
    out_index: *mut usize,
) -> efi::Status {
    if number_of_events == 0 || event_array.is_null() || out_index.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    if st.tpl() != efi::TPL_APPLICATION {
        return efi::Status::UNSUPPORTED;
    }

    //get the events list as a slice
    let event_list = unsafe { core::slice::from_raw_parts(event_array, number_of_events) };

    //spin on the list
    loop {
        for (index, event) in event_list.iter().enumerate() {
            match check_event(event_db, st, *event) {
                efi::Status::NOT_READY => (),
                status => {
                    unsafe { *out_index = index };
                    return status;
                }
            }
        }
    }
}

#[inline(always)]
pub fn check_event(event_db: &mut EventDb, st: &SystemState, event: efi::Event) -> efi::Status {
    let event_type = match event_db.get_event_type(event) {
        Ok(event_type) => event_type,
        Err(err) => return err,
    };

    if event_type.is_notify_signal() {
        return efi::Status::INVALID_PARAMETER;
    }

    match event_db.read_and_clear_signaled(event) {
        Ok(signaled) => {
            if signaled {
                return efi::Status::SUCCESS;
            }
        }
        Err(err) => return err,
    }

    match event_db.queue_event_notify(event) {
        Ok(()) => (),
        Err(err) => return err,
    }

    // raise/restore TPL to allow notifies to occur at the appropriate level.
    let old_tpl = raise_tpl(st, efi::TPL_HIGH_LEVEL);
    restore_tpl(event_db, st, old_tpl);

    match event_db.read_and_clear_signaled(event) {
        Ok(signaled) => {
            if signaled {
                return efi::Status::SUCCESS;
            }
        }
        Err(err) => return err,
    }

    efi::Status::NOT_READY
}

#[inline(always)]
pub fn set_timer(event_db: &mut EventDb, st: &SystemState, event: efi::Event, timer_type: efi::TimerDelay, trigger_time: u64) -> efi::Status {
    let timer_type = match TimerDelay::try_from(timer_type) {
        Err(err) => return err,
        Ok(timer_type) => timer_type,
    };

    let (trigger_time, period) = match timer_type {
        TimerDelay::TimerCancel => (None, None),
        TimerDelay::TimerRelative => (Some(st.time() + trigger_time), None),
        TimerDelay::TimerPeriodic => (Some(st.time() + trigger_time), Some(trigger_time)),
    };

    match event_db.set_timer(event, timer_type, trigger_time, period) {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => err,
    }
}

#[inline(always)]
pub fn raise_tpl(st: &SystemState, new_tpl: efi::Tpl) -> efi::Tpl {
    assert!(new_tpl <= efi::TPL_HIGH_LEVEL, "Invalid attempt to raise TPL above TPL_HIGH_LEVEL");

    let prev_tpl = st.current_tpl.fetch_max(new_tpl, Ordering::SeqCst);

    assert!(
        new_tpl >= prev_tpl,
        "Invalid attempt to raise TPL to lower value. New TPL: {:#x?}, Prev TPL: {:#x?}",
        new_tpl,
        prev_tpl
    );

    if (new_tpl == efi::TPL_HIGH_LEVEL) && (prev_tpl < efi::TPL_HIGH_LEVEL) {
        st.set_interrupt_state(false);
    }
    prev_tpl
}

#[inline(always)]
pub fn restore_tpl(event_db: &mut EventDb, st: &SystemState, new_tpl: efi::Tpl) {
    let prev_tpl = st.current_tpl.fetch_min(new_tpl, Ordering::SeqCst);

    assert!(
        new_tpl <= prev_tpl,
        "Invalid attempt to restore TPL to higher value. New TPL: {:#x?}, Prev TPL: {:#x?}",
        new_tpl,
        prev_tpl
    );

    if new_tpl < prev_tpl {
        // Care must be taken to deal with re-entrant "restore_tpl" cases. For example, the event_notification_iter created
        // here requires taking the lock on EVENT_DB to iterate. The release of that lock will call restore_tpl.
        // To avoid infinite recursion, this logic uses EVENT_NOTIFIES_IN_PROGRESS to ensure that only one instance of
        // restore_tpl is accessing the locked EVENT_DB. restore_tpl calls that occur while the event notification iter is
        // in use will get back an empty vector of event notifications and will simply restore the TPL and exit.
        let events =
            match st.event_notifies_in_progress.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => {
                    let events = event_db.event_notification_iter(new_tpl).collect();
                    st.event_notifies_in_progress.store(false, Ordering::Release);
                    events
                }
                Err(_) => alloc::vec![],
            };

        for event in events {
            if event.notify_tpl < efi::TPL_HIGH_LEVEL {
                st.set_interrupt_state(true);
            } else {
                st.set_interrupt_state(false);
            }
            st.set_tpl(event.notify_tpl);
            let notify_context = match event.notify_context {
                Some(context) => context,
                None => core::ptr::null_mut(),
            };

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
        st.set_interrupt_state(true);
    }
    st.set_tpl(new_tpl);
}

pub fn gcd_map_change(event_db: &mut EventDb, st: &SystemState, map_change_type: gcd::MapChangeType) {
    if st.event_db_initialized.load(Ordering::SeqCst) {
        match map_change_type {
            gcd::MapChangeType::AddMemorySpace
            | gcd::MapChangeType::AllocateMemorySpace
            | gcd::MapChangeType::FreeMemorySpace
            | gcd::MapChangeType::RemoveMemorySpace => event_db.signal_group(efi::EVENT_GROUP_MEMORY_MAP_CHANGE),
            gcd::MapChangeType::SetMemoryAttributes | gcd::MapChangeType::SetMemoryCapabilities => (),
        }
    }
}

#[inline(always)]
pub fn timer_tick(event_db: &mut EventDb, st: &SystemState, time: u64) {
    let old_tpl = raise_tpl(st, efi::TPL_HIGH_LEVEL);
    st.time.fetch_add(time, Ordering::SeqCst);
    let current_time = st.time.load(Ordering::SeqCst);
    event_db.timer_tick(current_time);
    restore_tpl(event_db, st, old_tpl); //implicitly dispatches timer notifies if any.
}

#[inline(always)]
pub fn timer_available_callback(protocol_db: &mut ProtocolDb, event_db: &mut EventDb, event: efi::Event, _context: *mut c_void) {
    match protocol_db.locate_protocol(timer::PROTOCOL_GUID) {
        Ok(timer_arch_ptr) => {
            let timer_arch_ptr = timer_arch_ptr as *mut timer::Protocol;
            let timer_arch = unsafe { &*(timer_arch_ptr) };
            (timer_arch.register_handler)(timer_arch_ptr, super::BootServices::timer_tick);
            if let Err(status_err) = event_db.close_event(event) {
                log::warn!("Could not close event for timer_available_callback due to error {:?}", status_err);
            }
        }
        Err(err) => panic!("Unable to locate timer arch: {:?}", err),
    }
}

#[inline(always)]
pub fn cpu_arch_available(protocol_db: &mut ProtocolDb, event_db: &mut EventDb, st: &SystemState, event: efi::Event, _context: *mut c_void) {
    match protocol_db.locate_protocol(cpu_arch::PROTOCOL_GUID) {
        Ok(cpu_arch_ptr) => {
            st.cpu_arch_ptr.store(cpu_arch_ptr as *mut cpu_arch::Protocol, Ordering::SeqCst);
            if let Err(status_err) = event_db.close_event(event) {
                log::warn!("Could not close event for cpu_arch_available due to error {:?}", status_err);
            }
        }
        Err(err) => panic!("Unable to cpu arch: {:?}", err),
    }
}
