//! UEFI Event Database support
//!
//! This module provides an UEFI event database implementation.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![warn(missing_docs)]

extern crate alloc;

use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use core::{cmp::Ordering, ffi::c_void, fmt};
use patina::error::EfiError;
use r_efi::efi;

use crate::{runtime, tpl_mutex};

/// Defines the supported UEFI event types
#[repr(u32)]
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum EventType {
    ///
    /// 0x80000200       Timer event with a notification function that is
    /// queue when the event is signaled with SignalEvent()
    ///
    TimerNotify = efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
    ///
    /// 0x80000000       Timer event without a notification function. It can be
    /// signaled with SignalEvent() and checked with CheckEvent() or WaitForEvent().
    ///
    Timer = efi::EVT_TIMER,
    ///
    /// 0x00000100       Generic event with a notification function that
    /// can be waited on with CheckEvent() or WaitForEvent()
    ///
    NotifyWait = efi::EVT_NOTIFY_WAIT,
    ///
    /// 0x00000200       Generic event with a notification function that
    /// is queue when the event is signaled with SignalEvent()
    ///
    NotifySignal = efi::EVT_NOTIFY_SIGNAL,
    ///
    /// 0x00000201       ExitBootServicesEvent.
    ///
    ExitBootServices = efi::EVT_SIGNAL_EXIT_BOOT_SERVICES,
    ///
    /// 0x60000202       SetVirtualAddressMapEvent.
    ///
    SetVirtualAddress = efi::EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE,
    ///
    /// 0x00000000       Generic event without a notification function.
    /// It can be signaled with SignalEvent() and checked with CheckEvent()
    /// or WaitForEvent().
    ///
    Generic = 0x00000000,
    ///
    /// 0x80000100       Timer event with a notification function that can be
    /// waited on with CheckEvent() or WaitForEvent()
    ///
    TimerNotifyWait = efi::EVT_TIMER | efi::EVT_NOTIFY_WAIT,
}

impl TryFrom<u32> for EventType {
    type Error = EfiError;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            x if x == EventType::TimerNotify as u32 => Ok(EventType::TimerNotify),
            x if x == EventType::Timer as u32 => Ok(EventType::Timer),
            x if x == EventType::NotifyWait as u32 => Ok(EventType::NotifyWait),
            x if x == EventType::NotifySignal as u32 => Ok(EventType::NotifySignal),
            //NOTE: the following are placeholders for corresponding event groups; we don't allow them here
            //as the code using the library should do the appropriate translation to event groups before calling create_event
            x if x == EventType::ExitBootServices as u32 => Err(EfiError::InvalidParameter),
            x if x == EventType::SetVirtualAddress as u32 => Err(EfiError::InvalidParameter),
            x if x == EventType::Generic as u32 => Ok(EventType::Generic),
            x if x == EventType::TimerNotifyWait as u32 => Ok(EventType::TimerNotifyWait),
            _ => Err(EfiError::InvalidParameter),
        }
    }
}

impl EventType {
    /// indicates whether this EventType is NOTIFY_SIGNAL
    pub fn is_notify_signal(&self) -> bool {
        (*self as u32) & efi::EVT_NOTIFY_SIGNAL != 0
    }

    /// indicates whether this EventType is NOTIFY_WAIT
    pub fn is_notify_wait(&self) -> bool {
        (*self as u32) & efi::EVT_NOTIFY_WAIT != 0
    }

    /// indicates whether this EventType is TIMER
    pub fn is_timer(&self) -> bool {
        (*self as u32) & efi::EVT_TIMER != 0
    }
}

/// Defines supported timer delay types.
#[repr(u32)]
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum TimerDelay {
    /// Cancels a pending timer
    Cancel,
    /// Creates a periodic timer
    Periodic,
    /// Creates a one-shot relative timer
    Relative,
}

impl TryFrom<u32> for TimerDelay {
    type Error = efi::Status;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            x if x == TimerDelay::Cancel as u32 => Ok(TimerDelay::Cancel),
            x if x == TimerDelay::Periodic as u32 => Ok(TimerDelay::Periodic),
            x if x == TimerDelay::Relative as u32 => Ok(TimerDelay::Relative),
            _ => Err(efi::Status::INVALID_PARAMETER),
        }
    }
}

/// Event Notification
#[derive(Clone)]
pub struct EventNotification {
    /// event handle
    pub event: efi::Event,
    /// efi::TPL that notification should run at
    pub notify_tpl: efi::Tpl,
    /// notification function
    pub notify_function: Option<efi::EventNotify>,
    /// context passed to the notification function
    pub notify_context: Option<*mut c_void>,
}

impl fmt::Debug for EventNotification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventNotification")
            .field("event", &self.event)
            .field("notify_tpl", &self.notify_tpl)
            .field("notify_function", &self.notify_function.map(|f| f as usize))
            .field("notify_context", &self.notify_context)
            .finish()
    }
}

//This type is necessary because the HeapSort used to order BTreeSet is not stable with respect
//to insertion order. So we have to tag each event notification as it is added so that we can
//use insertion order as part of the element comparison.
#[derive(Debug, Clone)]
struct TaggedEventNotification(EventNotification, u64);

impl PartialOrd for TaggedEventNotification {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TaggedEventNotification {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.0.event.cmp(&other.0.event) {
            // events are unique - so if event ids match, they are the same event.
            Ordering::Equal => Ordering::Equal,
            _ =>
            // Higher TPL has precedence; if TPLs are equal, then earlier insertion has precedence.
            {
                match other.0.notify_tpl.cmp(&self.0.notify_tpl) {
                    Ordering::Equal => self.1.cmp(&other.1),
                    non_eq => non_eq,
                }
            }
        }
    }
}

impl PartialEq for TaggedEventNotification {
    fn eq(&self, other: &Self) -> bool {
        self.0.event == other.0.event
    }
}

impl Eq for TaggedEventNotification {}

// Note: this Event type is a distinct data structure from efi::Event.
// Event defined here is a private data structure that tracks the data related to the event,
// whereas efi::Event is used as the public index or handle into the event database.
// In the code below efi::Event is used to qualify the index/handle type, where as `Event` with
// scope qualification refers to this private type.
struct Event {
    event_id: usize,
    event_type: EventType,
    event_group: Option<efi::Guid>,

    signaled: bool,

    //Only used for NOTIFY events.
    notify_tpl: efi::Tpl,
    notify_function: Option<efi::EventNotify>,
    notify_context: Option<*mut c_void>,

    //Only used for TIMER events.
    trigger_time: Option<u64>,
    period: Option<u64>,
}

// SAFETY: This structure is used within a lock on a single core and is not mutated
// after creation. It is safe to share references to it.
unsafe impl Sync for crate::event_db::Event {}
unsafe impl Send for crate::event_db::Event {}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut notify_func = 0;
        if self.notify_function.is_some() {
            notify_func = self.notify_function.unwrap() as usize;
        }

        f.debug_struct("Event")
            .field("event_id", &self.event_id)
            .field("event_type", &self.event_type)
            .field("event_group", &self.event_group)
            .field("signaled", &self.signaled)
            .field("notify_tpl", &self.notify_tpl)
            .field("notify_function", &notify_func)
            .field("notify_context", &self.notify_context)
            .field("trigger_time", &self.trigger_time)
            .field("period", &self.period)
            .finish()
    }
}

impl Event {
    fn new(
        event_id: usize,
        event_type: u32,
        notify_tpl: efi::Tpl,
        notify_function: Option<efi::EventNotify>,
        notify_context: Option<*mut c_void>,
        event_group: Option<efi::Guid>,
    ) -> Result<Self, EfiError> {
        let notifiable = (event_type & (efi::EVT_NOTIFY_SIGNAL | efi::EVT_NOTIFY_WAIT)) != 0;
        let event_type: EventType = event_type.try_into()?;

        if notifiable {
            if notify_function.is_none() {
                return Err(EfiError::InvalidParameter);
            }

            // Pedantic check; this will probably not work with "real firmware", so
            // loosen up a bit.
            // match notify_tpl {
            //     efi::TPL_APPLICATION | efi::TPL_CALLBACK | efi::TPL_NOTIFY | efi::TPL_HIGH_LEVEL => (),
            //     _ => return Err(EfiError::InvalidParameter),
            // }
            if !((efi::TPL_APPLICATION + 1)..=efi::TPL_HIGH_LEVEL).contains(&notify_tpl) {
                return Err(EfiError::InvalidParameter);
            }
        }

        Ok(Event {
            event_id,
            event_type,
            notify_tpl,
            notify_function,
            notify_context,
            event_group,
            signaled: false,
            trigger_time: None,
            period: None,
        })
    }

    pub fn efi_event(&self) -> efi::Event {
        self.event_id as efi::Event
    }
}

struct EventDb {
    events: BTreeMap<usize, Event>,
    next_event_id: usize,
    //TODO: using a BTreeSet here as a priority queue is slower [O(log n)] vs. the
    //per-TPL lists used in the reference C implementation [O(1)] for (de)queueing of event notifies.
    //Benchmarking would need to be done to see whether that perf impact plays out to significantly
    //impact real-world usage.
    pending_notifies: BTreeSet<TaggedEventNotification>,
    notify_tags: u64, //used to ensure that each notify gets a unique tag in increasing order
}

impl EventDb {
    /// Tag used for runtime event IDs.
    const RT_EVENT: usize = 1 << (usize::BITS - 1);

    const fn new() -> Self {
        EventDb { events: BTreeMap::new(), next_event_id: 1, pending_notifies: BTreeSet::new(), notify_tags: 0 }
    }

    fn create_event(
        &mut self,
        event_type: u32,
        notify_tpl: r_efi::base::Tpl,
        notify_function: Option<efi::EventNotify>,
        notify_context: Option<*mut c_void>,
        event_group: Option<efi::Guid>,
    ) -> Result<efi::Event, EfiError> {
        if self.next_event_id == Self::RT_EVENT {
            debug_assert!(false, "Event ID space exhausted.");
            return Err(EfiError::OutOfResources);
        }

        let runtime =
            (event_type & efi::EVT_RUNTIME) != 0 || event_group == Some(efi::EVENT_GROUP_VIRTUAL_ADDRESS_CHANGE);

        // Tag runtime event specially as they are tracked seperately.
        let id = if runtime { self.next_event_id | Self::RT_EVENT } else { self.next_event_id };
        self.next_event_id += 1;

        if runtime {
            // Runtime events are managed by the runtime module.
            runtime::add_runtime_event(
                id as efi::Event,
                event_type,
                notify_tpl,
                notify_function,
                event_group,
                notify_context,
            )?;
        } else {
            let event = Event::new(id, event_type, notify_tpl, notify_function, notify_context, event_group)?;
            self.events.insert(id, event);
        }

        Ok(id as efi::Event)
    }

    fn close_event(&mut self, event: efi::Event) -> Result<(), EfiError> {
        let id = event as usize;
        if (id & Self::RT_EVENT) != 0 {
            runtime::remove_runtime_event(id as efi::Event)?;
        } else {
            self.events.remove(&id).ok_or(EfiError::InvalidParameter)?;
        }

        Ok(())
    }

    //private helper function for signal_event.
    fn queue_notify_event(pending_notifies: &mut BTreeSet<TaggedEventNotification>, event: &mut Event, tag: u64) {
        if event.event_type.is_notify_signal() || event.event_type.is_notify_wait() {
            pending_notifies.insert(TaggedEventNotification(
                EventNotification {
                    event: event.efi_event(),
                    notify_tpl: event.notify_tpl,
                    notify_function: event.notify_function,
                    notify_context: event.notify_context,
                },
                tag,
            ));
        }
    }

    fn signal_event(&mut self, event: efi::Event) -> Result<(), EfiError> {
        let id = event as usize;
        let current_event = self.events.get_mut(&id).ok_or(EfiError::InvalidParameter)?;

        //explicitly match the EDK II C implementation by not queueing an additional notify.
        if current_event.signaled {
            return Ok(());
        }

        //signal all the members of the same event group (including the current one), if present.
        if let Some(target_group) = current_event.event_group {
            self.signal_group(target_group);
        } else {
            // if no group, signal the event by itself.
            current_event.signaled = true;
            if current_event.event_type.is_notify_signal() {
                Self::queue_notify_event(&mut self.pending_notifies, current_event, self.notify_tags);
                self.notify_tags += 1;
            }
        }
        Ok(())
    }

    fn signal_group(&mut self, group: efi::Guid) {
        for member_event in self.events.values_mut().rev().filter(|e| e.event_group == Some(group) && !e.signaled) {
            member_event.signaled = true;

            if member_event.event_type.is_notify_signal() {
                Self::queue_notify_event(&mut self.pending_notifies, member_event, self.notify_tags);
                self.notify_tags += 1;
            }
        }
    }

    fn clear_signal(&mut self, event: efi::Event) -> Result<(), EfiError> {
        let id = event as usize;
        let event = self.events.get_mut(&id).ok_or(EfiError::InvalidParameter)?;
        event.signaled = false;
        Ok(())
    }

    fn is_signaled(&mut self, event: efi::Event) -> bool {
        let id = event as usize;
        if let Some(event) = self.events.get(&id) { event.signaled } else { false }
    }

    fn queue_event_notify(&mut self, event: efi::Event) -> Result<(), EfiError> {
        let id = event as usize;
        let current_event = self.events.get_mut(&id).ok_or(EfiError::InvalidParameter)?;

        Self::queue_notify_event(&mut self.pending_notifies, current_event, self.notify_tags);
        self.notify_tags += 1;

        Ok(())
    }

    fn get_event_type(&mut self, event: efi::Event) -> Result<EventType, EfiError> {
        let id = event as usize;
        Ok(self.events.get(&id).ok_or(EfiError::InvalidParameter)?.event_type)
    }

    #[allow(dead_code)]
    fn get_notification_data(&mut self, event: efi::Event) -> Result<EventNotification, EfiError> {
        let id = event as usize;
        if let Some(found_event) = self.events.get(&id) {
            if (found_event.event_type as u32) & (efi::EVT_NOTIFY_SIGNAL | efi::EVT_NOTIFY_WAIT) == 0 {
                return Err(EfiError::NotFound);
            }
            Ok(EventNotification {
                event,
                notify_tpl: found_event.notify_tpl,
                notify_function: found_event.notify_function,
                notify_context: found_event.notify_context,
            })
        } else {
            Err(EfiError::NotFound)
        }
    }

    fn set_timer(
        &mut self,
        event: efi::Event,
        timer_type: TimerDelay,
        trigger_time: Option<u64>,
        period: Option<u64>,
    ) -> Result<(), EfiError> {
        let id = event as usize;
        if let Some(event) = self.events.get_mut(&id) {
            if !event.event_type.is_timer() {
                return Err(EfiError::InvalidParameter);
            }
            match timer_type {
                TimerDelay::Cancel => {
                    if trigger_time.is_some() || period.is_some() {
                        return Err(EfiError::InvalidParameter);
                    }
                }
                TimerDelay::Periodic => {
                    if trigger_time.is_none() || period.is_none() {
                        return Err(EfiError::InvalidParameter);
                    }
                }
                TimerDelay::Relative => {
                    if trigger_time.is_none() || period.is_some() {
                        return Err(EfiError::InvalidParameter);
                    }
                }
            }
            event.trigger_time = trigger_time;
            event.period = period;
            Ok(())
        } else {
            Err(EfiError::InvalidParameter)
        }
    }

    fn timer_tick(&mut self, current_time: u64) {
        // Poll the debugger before processing any events. This has no effect if
        // the debugger is not enabled.
        patina_debugger::poll_debugger();

        let events: Vec<usize> = self.events.keys().rev().cloned().collect();
        for event in events {
            let current_event = if let Some(current) = self.events.get_mut(&event) {
                current
            } else {
                debug_assert!(false, "Event {event:?} not found.");
                log::error!("Event {event:?} not found.");
                continue;
            };
            if current_event.event_type.is_timer()
                && let Some(trigger_time) = current_event.trigger_time
                && trigger_time <= current_time
            {
                if let Some(period) = current_event.period {
                    current_event.trigger_time = Some(current_time + period);
                } else {
                    //no period means it's a one-shot event; another call to set_timer is required to "re-arm"
                    current_event.trigger_time = None;
                }
                if let Err(e) = self.signal_event(event as *mut c_void) {
                    log::error!("Error {e:?} signaling event {event:?}.");
                }
            }
        }
    }

    fn consume_next_event_notify(&mut self, tpl_level: efi::Tpl) -> Option<EventNotification> {
        //if items at front of queue don't exist (e.g. due to close_event), silently pop them off.
        while let Some(item) = self.pending_notifies.first() {
            if !self.events.contains_key(&(item.0.event as usize)) {
                self.pending_notifies.pop_first();
            } else {
                break;
            }
        }
        //if item at front of queue is not higher than desired efi::TPL, then return none
        //otherwise, pop it off, mark it un-signaled, and return it.
        if let Some(item) = self.pending_notifies.first() {
            if item.0.notify_tpl <= tpl_level {
                return None;
            } else if let Some(item) = self.pending_notifies.pop_first() {
                return Some(item.0);
            } else {
                log::error!("Pending_notifies was empty, but it should have at least one item.");
            }
        }
        None
    }

    fn is_valid(&mut self, event: efi::Event) -> bool {
        self.events.contains_key(&(event as usize))
    }
}

/// Spin-Locked event database instance.
///
/// This is the main access point for interaction with the event database.
/// The event database is intended to be used as a global singleton, so access
/// is only allowed through this structure which ensures that the event database
/// is properly guarded against race conditions.
pub struct SpinLockedEventDb {
    inner: tpl_mutex::TplMutex<EventDb>,
}

impl Default for SpinLockedEventDb {
    fn default() -> Self {
        Self::new()
    }
}

impl SpinLockedEventDb {
    /// Creates a new instance of EventDb.
    pub const fn new() -> Self {
        SpinLockedEventDb { inner: tpl_mutex::TplMutex::new(efi::TPL_HIGH_LEVEL, EventDb::new(), "EventLock") }
    }

    fn lock(&self) -> tpl_mutex::TplGuard<'_, EventDb> {
        self.inner.lock()
    }

    /// Creates a new event in the event database
    ///
    /// This function closely matches the semantics of the EFI_BOOT_SERVICES.CreateEventEx() API in
    /// UEFI spec 2.10 section 7.1.2. Please refer to the spec for details on the input parameters.
    ///
    /// On success, this function returns the newly created event.
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    pub fn create_event(
        &self,
        event_type: u32,
        notify_tpl: r_efi::base::Tpl,
        notify_function: Option<efi::EventNotify>,
        notify_context: Option<*mut c_void>,
        event_group: Option<efi::Guid>,
    ) -> Result<efi::Event, EfiError> {
        self.lock().create_event(event_type, notify_tpl, notify_function, notify_context, event_group)
    }

    /// Closes (deletes) an event from the event database
    ///
    /// This function closely matches the semantics of the EFI_BOOT_SERVICES.CloseEvent() API in
    /// UEFI spec 2.10 section 7.1.3. Please refer to the spec for details on the input parameters.
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    pub fn close_event(&self, event: efi::Event) -> Result<(), EfiError> {
        self.lock().close_event(event)
    }

    /// Marks an event as signaled, and queues it for dispatch if it is of type NotifySignalEvent
    ///
    /// This function closely matches the semantics of the EFI_BOOT_SERVICES.SignalEvent() API in
    /// UEFI spec 2.10 section 7.1.4. Please refer to the spec for details on the input parameters.
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    pub fn signal_event(&self, event: efi::Event) -> Result<(), EfiError> {
        self.lock().signal_event(event)
    }

    /// Signals an event group
    ///
    /// This routine signals all events in the given event group. There isn't an equivalent UEFI spec API for this; the
    /// equivalent would need to be accomplished by creating a dummy event that is a member of the group and signalling
    /// that event.
    pub fn signal_group(&self, group: efi::Guid) {
        self.lock().signal_group(group)
    }

    /// Returns the event type for the given event
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect event is given.
    pub fn get_event_type(&self, event: efi::Event) -> Result<EventType, EfiError> {
        self.lock().get_event_type(event)
    }

    /// Indicates whether the given event is in the signaled state
    #[allow(dead_code)]
    pub fn is_signaled(&self, event: efi::Event) -> bool {
        self.lock().is_signaled(event)
    }

    /// Clears the signaled state for the given event.
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    #[allow(dead_code)]
    pub fn clear_signal(&self, event: efi::Event) -> Result<(), EfiError> {
        self.lock().clear_signal(event)
    }

    /// Atomically reads and clears the signaled state.
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    pub fn read_and_clear_signaled(&self, event: efi::Event) -> Result<bool, EfiError> {
        let mut event_db = self.lock();
        let signaled = event_db.is_signaled(event);
        if signaled {
            event_db.clear_signal(event)?;
        }
        Ok(signaled)
    }

    /// Queues the notify for the given event.
    ///
    /// Queued events can be retrieved via [`consume_next_event_notify`](SpinLockedEventDb::consume_next_event_notify).
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    pub fn queue_event_notify(&self, event: efi::Event) -> Result<(), EfiError> {
        self.lock().queue_event_notify(event)
    }

    /// Returns the notification data associated with the event.
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    #[allow(dead_code)]
    pub fn get_notification_data(&self, event: efi::Event) -> Result<EventNotification, EfiError> {
        self.lock().get_notification_data(event)
    }

    /// Sets a timer on the specified event
    ///
    /// [`timer_tick`](SpinLockedEventDb::timer_tick) is used to advanced time; when a timer expires, the corresponding
    /// event is queued and can be retrieved via [`consume_next_event_notify`](SpinLockedEventDb::consume_next_event_notify).
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    pub fn set_timer(
        &self,
        event: efi::Event,
        timer_type: TimerDelay,
        trigger_time: Option<u64>,
        period: Option<u64>,
    ) -> Result<(), EfiError> {
        self.lock().set_timer(event, timer_type, trigger_time, period)
    }

    /// called to advance the system time and process any timer events that fire
    ///
    /// [`set_timer`](SpinLockedEventDb::set_timer) is used to configure timers with either a one-shot or periodic
    /// timer.
    ///
    /// This routine is called to inform the event database that that a certain amount of time has passed. The event
    /// database will iterate over all events and determine if any of the timers have expired based on the amount of
    /// time that has passed per this call. If any timers are expired, the corresponding events will be signaled.
    ///
    /// signaled events with notifications are queued and can be retrieved via
    /// [`consume_next_event_notify`](SpinLockedEventDb::consume_next_event_notify).
    pub fn timer_tick(&self, current_time: u64) {
        self.lock().timer_tick(current_time);
    }

    /// Returns the next pending event notification (if any) that should be dispatched at or above the given TPL level.
    ///
    /// Events can be added to the pending queue directly via
    /// [`queue_event_notify`](SpinLockedEventDb::queue_event_notify) or via timer expiration configured via
    /// [`set_timer`](SpinLockedEventDb::set_timer) followed by a [`timer_tick`](SpinLockedEventDb::timer_tick) that
    /// causes the timer to expire.
    ///
    pub fn consume_next_event_notify(&self, tpl_level: efi::Tpl) -> Option<EventNotification> {
        self.lock().consume_next_event_notify(tpl_level)
    }

    /// Indicates whether a given event is valid.
    pub fn is_valid(&self, event: efi::Event) -> bool {
        self.lock().is_valid(event)
    }
}

unsafe impl Send for SpinLockedEventDb {}
unsafe impl Sync for SpinLockedEventDb {}

#[cfg(test)]
#[coverage(off)]
mod tests {
    extern crate std;
    use core::{iter, str::FromStr};

    use alloc::{vec, vec::Vec};
    use patina::Guid;
    use r_efi::efi;
    use uuid::Uuid;

    use crate::test_support;

    use super::*;

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            f();
        })
        .unwrap();
    }

    #[test]
    fn new_should_create_event_db_local() {
        with_locked_state(|| {
            //Note: for coverage, here we create the SpinLockedEventDb on the stack. But all the other tests create it as
            //'static' to mimic expected usage.
            let spin_locked_event_db: SpinLockedEventDb = SpinLockedEventDb::new();
            let events = &spin_locked_event_db.lock().events;
            assert_eq!(events.len(), 0);
        });
    }

    #[test]
    fn new_should_create_event_db() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            assert_eq!(SPIN_LOCKED_EVENT_DB.lock().events.len(), 0)
        });
    }

    extern "efiapi" fn test_notify_function(_: efi::Event, _: *mut core::ffi::c_void) {}

    #[test]
    fn create_event_should_create_event() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            let result = SPIN_LOCKED_EVENT_DB.create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                None,
                None,
            );
            assert!(result.is_ok());
            let event = result.unwrap();
            let index = event as usize;
            assert!(index < SPIN_LOCKED_EVENT_DB.lock().next_event_id);
            let events = &SPIN_LOCKED_EVENT_DB.lock().events;
            assert_eq!(events.get(&index).unwrap().event_type, EventType::TimerNotify);
            assert_eq!(events.get(&index).unwrap().event_type as u32, efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL);
            assert_eq!(events.get(&index).unwrap().notify_tpl, efi::TPL_NOTIFY);
            assert_eq!(events.get(&index).unwrap().notify_function.unwrap() as usize, test_notify_function as usize);
            assert_eq!(events.get(&index).unwrap().notify_context, None);
            assert_eq!(events.get(&index).unwrap().event_group, None);
        });
    }

    #[test]
    fn create_event_with_bad_input_should_not_create_event() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();

            //Try with an invalid event type.
            let result = SPIN_LOCKED_EVENT_DB.create_event(
                efi::EVT_SIGNAL_EXIT_BOOT_SERVICES,
                efi::TPL_NOTIFY,
                None,
                None,
                None,
            );
            assert_eq!(result, Err(EfiError::InvalidParameter));

            //if type has efi::EVT_NOTIFY_SIGNAL or efi::EVT_NOTIFY_WAIT, then NotifyFunction must be non-NULL and NotifyTpl must be a valid efi::TPL.
            //Try to create a notified event with None notify_function - should fail.
            let result = SPIN_LOCKED_EVENT_DB.create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                None,
                None,
                None,
            );
            assert_eq!(result, Err(EfiError::InvalidParameter));

            //Try to create a notified event with Some notify_function but invalid efi::TPL - should fail.
            let result = SPIN_LOCKED_EVENT_DB.create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_HIGH_LEVEL + 1,
                Some(test_notify_function),
                None,
                None,
            );
            assert_eq!(result, Err(EfiError::InvalidParameter));
        });
    }

    #[test]
    fn close_event_should_delete_event() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            let mut events: Vec<efi::Event> = Vec::new();
            for _ in 0..10 {
                events.push(
                    SPIN_LOCKED_EVENT_DB
                        .create_event(
                            efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                            efi::TPL_NOTIFY,
                            Some(test_notify_function),
                            None,
                            None,
                        )
                        .unwrap(),
                );
            }
            for consumed in 1..11 {
                let event = events.pop().unwrap();
                assert!(SPIN_LOCKED_EVENT_DB.is_valid(event));
                let result = SPIN_LOCKED_EVENT_DB.close_event(event);
                assert!(result.is_ok());
                assert_eq!(SPIN_LOCKED_EVENT_DB.lock().events.len(), 10 - consumed);
                assert!(!SPIN_LOCKED_EVENT_DB.is_valid(event));
            }
        });
    }

    #[test]
    fn signal_event_should_put_events_in_signaled_state() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            let mut events: Vec<efi::Event> = Vec::new();
            for _ in 0..10 {
                events.push(
                    SPIN_LOCKED_EVENT_DB
                        .create_event(
                            efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                            efi::TPL_NOTIFY,
                            Some(test_notify_function),
                            None,
                            None,
                        )
                        .unwrap(),
                );
            }

            for event in events {
                let result: Result<(), EfiError> = SPIN_LOCKED_EVENT_DB.signal_event(event);
                assert!(result.is_ok());
                assert!(SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }
        });
    }

    #[test]
    fn signal_event_should_not_double_queue() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();

            let event = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();

            for _ in 0..2 {
                assert!(SPIN_LOCKED_EVENT_DB.signal_event(event).is_ok());
            }

            //ensure only one notify was queued
            assert!(SPIN_LOCKED_EVENT_DB.lock().pending_notifies.len() == 1);

            //ensure the mere act of collecting the events doesn't allow another notification to be queued
            let _ = SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION);
            assert!(SPIN_LOCKED_EVENT_DB.signal_event(event).is_ok());
            assert!(SPIN_LOCKED_EVENT_DB.lock().pending_notifies.is_empty());

            //ensure the event can be re-queued after it's signal state has been cleared
            assert!(SPIN_LOCKED_EVENT_DB.clear_signal(event).is_ok());
            assert!(SPIN_LOCKED_EVENT_DB.signal_event(event).is_ok());
            assert!(SPIN_LOCKED_EVENT_DB.lock().pending_notifies.len() == 1);
        });
    }

    #[test]
    fn signal_event_on_an_event_group_should_put_all_members_in_signaled_state() {
        with_locked_state(|| {
            let uuid = Uuid::from_str("aefcf33c-ce02-47b4-89f6-4bacdeda3377").unwrap();
            let group1 = efi::Guid::from_bytes(uuid.as_bytes());
            let uuid = Uuid::from_str("3a08a8c7-054b-4268-8aed-bc6a3aef999f").unwrap();
            let group2 = efi::Guid::from_bytes(uuid.as_bytes());
            let uuid = Uuid::from_str("745e8316-4889-4f58-be3c-6b718b7170ec").unwrap();
            let group3 = efi::Guid::from_bytes(uuid.as_bytes());

            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            let mut group1_events: Vec<efi::Event> = Vec::new();
            let mut group2_events: Vec<efi::Event> = Vec::new();
            let mut group3_events: Vec<efi::Event> = Vec::new();
            let mut ungrouped_events: Vec<efi::Event> = Vec::new();

            for _ in 0..10 {
                group1_events.push(
                    SPIN_LOCKED_EVENT_DB
                        .create_event(
                            efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                            efi::TPL_NOTIFY,
                            Some(test_notify_function),
                            None,
                            Some(group1),
                        )
                        .unwrap(),
                );
            }

            for _ in 0..10 {
                group2_events.push(
                    SPIN_LOCKED_EVENT_DB
                        .create_event(
                            efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                            efi::TPL_NOTIFY,
                            Some(test_notify_function),
                            None,
                            Some(group2),
                        )
                        .unwrap(),
                );
            }

            for _ in 0..10 {
                group3_events.push(
                    SPIN_LOCKED_EVENT_DB
                        .create_event(
                            efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                            efi::TPL_NOTIFY,
                            Some(test_notify_function),
                            None,
                            Some(group3),
                        )
                        .unwrap(),
                );
            }

            for _ in 0..10 {
                ungrouped_events.push(
                    SPIN_LOCKED_EVENT_DB
                        .create_event(
                            efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                            efi::TPL_NOTIFY,
                            Some(test_notify_function),
                            None,
                            None,
                        )
                        .unwrap(),
                );
            }

            //signal an ungrouped event
            SPIN_LOCKED_EVENT_DB.signal_event(ungrouped_events.pop().unwrap()).unwrap();

            //all other events should remain un-signaled
            for event in group1_events.clone() {
                assert!(!SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }

            for event in group2_events.clone() {
                assert!(!SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }

            for event in ungrouped_events.clone() {
                assert!(!SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }

            //signal an event in a group
            SPIN_LOCKED_EVENT_DB.signal_event(group1_events[0]).unwrap();

            //events in the same group should be signaled.
            for event in group1_events.clone() {
                assert!(SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }

            //events in another group should not be signaled.
            for event in group2_events.clone() {
                assert!(!SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }

            //ungrouped events should not be signaled.
            for event in ungrouped_events.clone() {
                assert!(!SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }

            //signal an event in a different group
            SPIN_LOCKED_EVENT_DB.signal_event(group2_events[0]).unwrap();

            //first event group should remain signaled.
            for event in group1_events.clone() {
                assert!(SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }

            //second event group should now be signaled.
            for event in group2_events.clone() {
                assert!(SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }

            //third event group should not be signaled.
            for event in group3_events.clone() {
                assert!(!SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }

            //signal events in third group using signal_group
            SPIN_LOCKED_EVENT_DB.signal_group(group3);
            //first event group should remain signaled.
            for event in group1_events.clone() {
                assert!(SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }

            //second event group should remain signaled.
            for event in group2_events.clone() {
                assert!(SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }

            //third event group should now be signaled.
            for event in group3_events.clone() {
                assert!(SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }

            //ungrouped events should not be signaled.
            for event in ungrouped_events.clone() {
                assert!(!SPIN_LOCKED_EVENT_DB.is_signaled(event));
            }
        });
    }

    #[test]
    fn events_signaled_in_a_group_should_queue_in_reverse_creation_order() {
        with_locked_state(|| {
            let group_guid = Guid::try_from_string("4a7eafb6-2f5e-48d7-b8c4-44a48f341c6f").unwrap();
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            let mut group_events: Vec<efi::Event> = Vec::new();
            for _ in 0..10 {
                group_events.push(
                    SPIN_LOCKED_EVENT_DB
                        .create_event(
                            efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                            efi::TPL_NOTIFY,
                            Some(test_notify_function),
                            None,
                            Some(group_guid.to_efi_guid()),
                        )
                        .unwrap(),
                );
            }
            SPIN_LOCKED_EVENT_DB.signal_event(group_events[9]).unwrap();
            // verify that events are queued in reverse order of creation (assuming they are signaled at same time with
            // same notify_tpl).
            let mut event_db = SPIN_LOCKED_EVENT_DB.lock();
            let queue = &mut event_db.pending_notifies;
            assert_eq!(queue.len(), 10);
            for (group_item, queue_item) in iter::zip(group_events.iter().rev(), queue.iter()) {
                assert_eq!(group_item, &queue_item.0.event);
            }
        })
    }

    #[test]
    fn clear_signal_should_clear_signaled_state() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            let event = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(event).unwrap();
            assert!(SPIN_LOCKED_EVENT_DB.is_signaled(event));
            let result = SPIN_LOCKED_EVENT_DB.clear_signal(event);
            assert!(result.is_ok());
            assert!(!SPIN_LOCKED_EVENT_DB.is_signaled(event));
        });
    }

    #[test]
    fn is_signaled_should_return_false_for_closed_or_non_existent_event() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            let event = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(event).unwrap();
            assert!(SPIN_LOCKED_EVENT_DB.is_signaled(event));
            SPIN_LOCKED_EVENT_DB.close_event(event).unwrap();
            assert!(!SPIN_LOCKED_EVENT_DB.is_signaled(event));
            assert!(!SPIN_LOCKED_EVENT_DB.is_signaled(0x1234 as *mut c_void));
        });
    }

    #[test]
    fn signaled_events_with_notifies_should_be_put_in_pending_queue_in_tpl_order() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            let callback_evt1 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_CALLBACK,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            let callback_evt2 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_CALLBACK,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            let notify_evt1 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            let notify_evt2 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            let high_evt1 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_HIGH_LEVEL,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            let high_evt2 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_HIGH_LEVEL,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(callback_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(notify_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(high_evt1).unwrap();

            SPIN_LOCKED_EVENT_DB.signal_event(callback_evt2).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(notify_evt2).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(high_evt2).unwrap();

            {
                let mut event_db = SPIN_LOCKED_EVENT_DB.lock();
                let queue = &mut event_db.pending_notifies;
                assert_eq!(queue.pop_first().unwrap().0.event, high_evt1);
                assert_eq!(queue.pop_first().unwrap().0.event, high_evt2);
                assert_eq!(queue.pop_first().unwrap().0.event, notify_evt1);
                assert_eq!(queue.pop_first().unwrap().0.event, notify_evt2);
                assert_eq!(queue.pop_first().unwrap().0.event, callback_evt1);
                assert_eq!(queue.pop_first().unwrap().0.event, callback_evt2);
            }
        });
    }

    #[test]
    fn signaled_event_iterator_should_return_next_events_in_tpl_order() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();

            assert!(SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION).is_none());

            let callback_evt1 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_CALLBACK,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            let callback_evt2 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_CALLBACK,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            let notify_evt1 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            let notify_evt2 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            let high_evt1 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_HIGH_LEVEL,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            let high_evt2 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_HIGH_LEVEL,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(callback_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(notify_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(high_evt1).unwrap();

            SPIN_LOCKED_EVENT_DB.signal_event(callback_evt2).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(notify_evt2).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(high_evt2).unwrap();

            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_NOTIFY));
            for (event_notification, expected_event) in event_iter.zip(vec![high_evt1, high_evt2]) {
                assert_eq!(event_notification.event, expected_event);
                assert!(SPIN_LOCKED_EVENT_DB.is_signaled(expected_event));
                let _ = SPIN_LOCKED_EVENT_DB.clear_signal(expected_event);
            }

            //re-signal the consumed events
            SPIN_LOCKED_EVENT_DB.signal_event(high_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(high_evt2).unwrap();

            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_CALLBACK));
            for (event_notification, expected_event) in
                event_iter.zip(vec![high_evt1, high_evt2, notify_evt1, notify_evt2])
            {
                assert_eq!(event_notification.event, expected_event);
                assert!(SPIN_LOCKED_EVENT_DB.is_signaled(expected_event));
                let _ = SPIN_LOCKED_EVENT_DB.clear_signal(expected_event);
            }

            //re-signal the consumed events
            SPIN_LOCKED_EVENT_DB.signal_event(high_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(high_evt2).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(notify_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(notify_evt2).unwrap();

            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            for (event_notification, expected_event) in
                event_iter.zip(vec![high_evt1, high_evt2, notify_evt1, notify_evt2, callback_evt1, callback_evt2])
            {
                assert_eq!(event_notification.event, expected_event);
                assert!(SPIN_LOCKED_EVENT_DB.is_signaled(expected_event));
                let _ = SPIN_LOCKED_EVENT_DB.clear_signal(expected_event);
            }

            //re-signal the consumed events
            SPIN_LOCKED_EVENT_DB.signal_event(high_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(high_evt2).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(notify_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(notify_evt2).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(callback_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(callback_evt2).unwrap();

            //close or clear some of the events before consuming
            SPIN_LOCKED_EVENT_DB.close_event(high_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.close_event(notify_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.close_event(callback_evt1).unwrap();

            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            for (event_notification, expected_event) in event_iter.zip(vec![high_evt2, notify_evt2, callback_evt2]) {
                assert_eq!(event_notification.event, expected_event);
                assert!(SPIN_LOCKED_EVENT_DB.is_signaled(expected_event));
                let _ = SPIN_LOCKED_EVENT_DB.clear_signal(expected_event);
            }
        });
    }

    #[test]
    fn signalling_an_event_more_than_once_should_not_queue_it_more_than_once() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();

            let callback_evt1 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_CALLBACK,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();

            SPIN_LOCKED_EVENT_DB.signal_event(callback_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(callback_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(callback_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(callback_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.signal_event(callback_evt1).unwrap();

            {
                let db = SPIN_LOCKED_EVENT_DB.lock();
                assert_eq!(db.pending_notifies.len(), 1);
            }
            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            assert_eq!(event_iter.count(), 1);
        });
    }

    #[test]
    fn read_and_clear_signaled_should_clear_signal() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();

            let callback_evt1 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_CALLBACK,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();

            SPIN_LOCKED_EVENT_DB.signal_event(callback_evt1).unwrap();

            {
                let db = SPIN_LOCKED_EVENT_DB.lock();
                assert_eq!(db.pending_notifies.len(), 1);
            }

            let result = SPIN_LOCKED_EVENT_DB.read_and_clear_signaled(callback_evt1);
            assert!(result.is_ok());
            let result = result.unwrap();
            assert!(result);
            let result = SPIN_LOCKED_EVENT_DB.read_and_clear_signaled(callback_evt1);
            assert!(result.is_ok());
            let result = result.unwrap();
            assert!(!result);
        });
    }

    #[test]
    fn signalling_a_notify_wait_event_should_not_queue_it() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();

            let callback_evt1 = SPIN_LOCKED_EVENT_DB
                .create_event(efi::EVT_NOTIFY_WAIT, efi::TPL_CALLBACK, Some(test_notify_function), None, None)
                .unwrap();

            SPIN_LOCKED_EVENT_DB.signal_event(callback_evt1).unwrap();

            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            assert_eq!(event_iter.count(), 0);
        });
    }

    #[test]
    fn queue_event_notify_should_queue_event_notify() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();

            let callback_evt1 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_CALLBACK,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();

            SPIN_LOCKED_EVENT_DB.queue_event_notify(callback_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.queue_event_notify(callback_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.queue_event_notify(callback_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.queue_event_notify(callback_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.queue_event_notify(callback_evt1).unwrap();

            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            assert_eq!(event_iter.count(), 1);
        });
    }

    #[test]
    fn queue_event_notify_should_work_for_both_notify_wait_and_notify_signal() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();

            let callback_evt1 = SPIN_LOCKED_EVENT_DB
                .create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(test_notify_function), None, None)
                .unwrap();

            let callback_evt2 = SPIN_LOCKED_EVENT_DB
                .create_event(efi::EVT_NOTIFY_WAIT, efi::TPL_CALLBACK, Some(test_notify_function), None, None)
                .unwrap();

            SPIN_LOCKED_EVENT_DB.queue_event_notify(callback_evt1).unwrap();
            SPIN_LOCKED_EVENT_DB.queue_event_notify(callback_evt2).unwrap();

            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            assert_eq!(event_iter.count(), 2);
        });
    }

    #[test]
    fn get_event_type_should_return_event_type() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            let event = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();

            let result = SPIN_LOCKED_EVENT_DB.get_event_type(event);
            assert_eq!(result.unwrap(), EventType::TimerNotify);

            let event = (event as usize + 1) as *mut c_void;
            let result = SPIN_LOCKED_EVENT_DB.get_event_type(event);
            assert_eq!(result, Err(EfiError::InvalidParameter));
        });
    }

    #[test]
    fn get_notification_data_should_return_notification_data() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            let test_context: *mut c_void = 0x1234 as *mut c_void;
            let event = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    Some(test_context),
                    None,
                )
                .unwrap();

            let notification_data = SPIN_LOCKED_EVENT_DB.get_notification_data(event);
            assert!(notification_data.is_ok());
            let event_notification = notification_data.unwrap();
            assert_eq!(event_notification.notify_tpl, efi::TPL_NOTIFY);
            assert_eq!(event_notification.notify_function.unwrap() as usize, test_notify_function as usize);
            assert_eq!(event_notification.notify_context.unwrap(), test_context);

            let event = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();

            let notification_data = SPIN_LOCKED_EVENT_DB.get_notification_data(event);
            assert!(notification_data.is_ok());
            let event_notification = notification_data.unwrap();
            assert_eq!(event_notification.notify_tpl, efi::TPL_NOTIFY);
            assert_eq!(event_notification.notify_function.unwrap() as usize, test_notify_function as usize);
            assert!(event_notification.notify_context.is_none());

            let event = SPIN_LOCKED_EVENT_DB.create_event(efi::EVT_TIMER, efi::TPL_NOTIFY, None, None, None).unwrap();
            let notification_data = SPIN_LOCKED_EVENT_DB.get_notification_data(event);
            assert_eq!(notification_data.err(), Some(EfiError::NotFound));

            let notification_data = SPIN_LOCKED_EVENT_DB.get_notification_data(0x1234 as *mut c_void);
            assert_eq!(notification_data.err(), Some(EfiError::NotFound));
        });
    }

    #[test]
    fn set_timer_on_event_should_set_timer_on_event() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            let event = SPIN_LOCKED_EVENT_DB
                .create_event(efi::EVT_TIMER, efi::TPL_NOTIFY, Some(test_notify_function), None, None)
                .unwrap();

            let index = event as usize;

            let result = SPIN_LOCKED_EVENT_DB.set_timer(event, TimerDelay::Relative, Some(0x100), None);
            assert!(result.is_ok());
            {
                let events = &SPIN_LOCKED_EVENT_DB.lock().events;
                assert_eq!(events.get(&index).unwrap().trigger_time, Some(0x100));
                assert_eq!(events.get(&index).unwrap().period, None);
            }

            let result = SPIN_LOCKED_EVENT_DB.set_timer(event, TimerDelay::Periodic, Some(0x100), Some(0x200));
            assert!(result.is_ok());
            {
                let events = &SPIN_LOCKED_EVENT_DB.lock().events;
                assert_eq!(events.get(&index).unwrap().trigger_time, Some(0x100));
                assert_eq!(events.get(&index).unwrap().period, Some(0x200));
            }

            let result = SPIN_LOCKED_EVENT_DB.set_timer(event, TimerDelay::Cancel, None, None);
            assert!(result.is_ok());
            {
                let events = &SPIN_LOCKED_EVENT_DB.lock().events;
                assert_eq!(events.get(&index).unwrap().trigger_time, None);
                assert_eq!(events.get(&index).unwrap().period, None);
            }

            let event = SPIN_LOCKED_EVENT_DB
                .create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_NOTIFY, Some(test_notify_function), None, None)
                .unwrap();

            let result = SPIN_LOCKED_EVENT_DB.set_timer(event, TimerDelay::Periodic, Some(0x100), Some(0x200));
            assert_eq!(result.err(), Some(EfiError::InvalidParameter));

            let event = SPIN_LOCKED_EVENT_DB
                .create_event(efi::EVT_TIMER, efi::TPL_NOTIFY, Some(test_notify_function), None, None)
                .unwrap();
            let result = SPIN_LOCKED_EVENT_DB.set_timer(event, TimerDelay::Cancel, Some(0x100), None);
            assert_eq!(result.err(), Some(EfiError::InvalidParameter));

            let event = SPIN_LOCKED_EVENT_DB
                .create_event(efi::EVT_TIMER, efi::TPL_NOTIFY, Some(test_notify_function), None, None)
                .unwrap();
            let result = SPIN_LOCKED_EVENT_DB.set_timer(event, TimerDelay::Periodic, None, None);
            assert_eq!(result.err(), Some(EfiError::InvalidParameter));

            let event = SPIN_LOCKED_EVENT_DB
                .create_event(efi::EVT_TIMER, efi::TPL_NOTIFY, Some(test_notify_function), None, None)
                .unwrap();
            let result = SPIN_LOCKED_EVENT_DB.set_timer(event, TimerDelay::Relative, None, Some(0x100));
            assert_eq!(result.err(), Some(EfiError::InvalidParameter));

            let result = SPIN_LOCKED_EVENT_DB.set_timer(event, TimerDelay::Relative, None, Some(0x100));
            assert_eq!(result.err(), Some(EfiError::InvalidParameter));

            let result = SPIN_LOCKED_EVENT_DB.set_timer(0x1234 as *mut c_void, TimerDelay::Relative, Some(0x100), None);
            assert_eq!(result.err(), Some(EfiError::InvalidParameter));
        });
    }

    #[test]
    fn timer_tick_should_signal_expired_timers() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            let event = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();

            let event2 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();

            SPIN_LOCKED_EVENT_DB.set_timer(event, TimerDelay::Relative, Some(0x100), None).unwrap();
            SPIN_LOCKED_EVENT_DB.set_timer(event2, TimerDelay::Relative, Some(0x400), None).unwrap();

            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            assert_eq!(event_iter.count(), 0);

            //tick past the first timer
            SPIN_LOCKED_EVENT_DB.timer_tick(0x200);

            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            let events = event_iter.collect::<Vec<EventNotification>>();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event, event);

            //tick again, but not enough to trigger second timer.
            SPIN_LOCKED_EVENT_DB.timer_tick(0x300);

            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            assert_eq!(event_iter.count(), 0);

            //tick past the second timer.
            SPIN_LOCKED_EVENT_DB.timer_tick(0x400);

            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            let events = event_iter.collect::<Vec<EventNotification>>();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event, event2);
        });
    }

    #[test]
    fn periodic_timers_should_rearm_after_tick() {
        with_locked_state(|| {
            static SPIN_LOCKED_EVENT_DB: SpinLockedEventDb = SpinLockedEventDb::new();
            let event = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();

            let event2 = SPIN_LOCKED_EVENT_DB
                .create_event(
                    efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                    efi::TPL_NOTIFY,
                    Some(test_notify_function),
                    None,
                    None,
                )
                .unwrap();

            SPIN_LOCKED_EVENT_DB.set_timer(event, TimerDelay::Periodic, Some(0x100), Some(0x100)).unwrap();
            SPIN_LOCKED_EVENT_DB.set_timer(event2, TimerDelay::Periodic, Some(0x500), Some(0x500)).unwrap();

            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            assert_eq!(event_iter.count(), 0);

            //tick past the first timer
            SPIN_LOCKED_EVENT_DB.timer_tick(0x100);
            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            let events = event_iter.collect::<Vec<EventNotification>>();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event, event);
            let _ = SPIN_LOCKED_EVENT_DB.clear_signal(events[0].event);

            //tick just prior to re-armed first timer
            SPIN_LOCKED_EVENT_DB.timer_tick(0x1FF);
            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            assert_eq!(event_iter.count(), 0);

            //tick past the re-armed first timer
            SPIN_LOCKED_EVENT_DB.timer_tick(0x210);
            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            let events = event_iter.collect::<Vec<EventNotification>>();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event, event);
            let _ = SPIN_LOCKED_EVENT_DB.clear_signal(events[0].event);

            //tick past the second timer.
            SPIN_LOCKED_EVENT_DB.timer_tick(0x500);
            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            let events = event_iter.collect::<Vec<EventNotification>>();
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].event, event2);
            assert_eq!(events[1].event, event);
            let _ = SPIN_LOCKED_EVENT_DB.clear_signal(events[0].event);
            let _ = SPIN_LOCKED_EVENT_DB.clear_signal(events[1].event);

            //tick past the rearmed first timer
            SPIN_LOCKED_EVENT_DB.timer_tick(0x600);
            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            let events = event_iter.collect::<Vec<EventNotification>>();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event, event);
            let _ = SPIN_LOCKED_EVENT_DB.clear_signal(events[0].event);

            //cancel the first timer
            SPIN_LOCKED_EVENT_DB.set_timer(event, TimerDelay::Cancel, None, None).unwrap();

            //tick past where it would have been.
            SPIN_LOCKED_EVENT_DB.timer_tick(0x700);
            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            assert_eq!(event_iter.count(), 0);

            //close the event for the second timer
            SPIN_LOCKED_EVENT_DB.close_event(event2).unwrap();

            //tick past where it would have been.
            SPIN_LOCKED_EVENT_DB.timer_tick(0x1000);
            let event_iter = iter::from_fn(|| SPIN_LOCKED_EVENT_DB.consume_next_event_notify(efi::TPL_APPLICATION));
            assert_eq!(event_iter.count(), 0);
        });
    }
}
