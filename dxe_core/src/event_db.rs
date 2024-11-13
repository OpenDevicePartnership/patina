//! UEFI Event Database support
//!
//! This library provides an UEFI event database implementation.
//!
//! ## Examples and Usage
//!
//! ```
//! use uefi_event::SpinLockedEventDb;
//!
//! let mut event_db = EventDb::new();
//! let result = event_db.create_event(
//!   0,
//!   0,
//!   None,
//!   None,
//!   None,
//! );
//!
//! event_db.signal_event (result.unwrap());
//!
//! assert!(event_db.is_signaled (result.unwrap()));
//!
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![warn(missing_docs)]

extern crate alloc;

use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use core::{cmp::Ordering, ffi::c_void, fmt};
use r_efi::efi;

/// Defines the supported UEFI event types
#[repr(u32)]
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum EventType {
    ///
    /// 0x80000200       Timer event with a notification function that is
    /// queue when the event is signaled with SignalEvent()
    ///
    TimerNotifyEvent = efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
    ///
    /// 0x80000000       Timer event without a notification function. It can be
    /// signaled with SignalEvent() and checked with CheckEvent() or WaitForEvent().
    ///
    TimerEvent = efi::EVT_TIMER,
    ///
    /// 0x00000100       Generic event with a notification function that
    /// can be waited on with CheckEvent() or WaitForEvent()
    ///
    NotifyWaitEvent = efi::EVT_NOTIFY_WAIT,
    ///
    /// 0x00000200       Generic event with a notification function that
    /// is queue when the event is signaled with SignalEvent()
    ///
    NotifySignalEvent = efi::EVT_NOTIFY_SIGNAL,
    ///
    /// 0x00000201       ExitBootServicesEvent.
    ///
    ExitBootServicesEvent = efi::EVT_SIGNAL_EXIT_BOOT_SERVICES,
    ///
    /// 0x60000202       SetVirtualAddressMapEvent.
    ///
    SetVirtualAddressEvent = efi::EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE,
    ///
    /// 0x00000000       Generic event without a notification function.
    /// It can be signaled with SignalEvent() and checked with CheckEvent()
    /// or WaitForEvent().
    ///
    GenericEvent = 0x00000000,
    ///
    /// 0x80000100       Timer event with a notification function that can be
    /// waited on with CheckEvent() or WaitForEvent()
    ///
    TimerNotifyWaitEvent = efi::EVT_TIMER | efi::EVT_NOTIFY_WAIT,
}

impl TryFrom<u32> for EventType {
    type Error = efi::Status;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            x if x == EventType::TimerNotifyEvent as u32 => Ok(EventType::TimerNotifyEvent),
            x if x == EventType::TimerEvent as u32 => Ok(EventType::TimerEvent),
            x if x == EventType::NotifyWaitEvent as u32 => Ok(EventType::NotifyWaitEvent),
            x if x == EventType::NotifySignalEvent as u32 => Ok(EventType::NotifySignalEvent),
            //NOTE: the following are placeholders for corresponding event groups; we don't allow them here
            //as the code using the library should do the appropriate translation to event groups before calling create_event
            x if x == EventType::ExitBootServicesEvent as u32 => Err(efi::Status::INVALID_PARAMETER),
            x if x == EventType::SetVirtualAddressEvent as u32 => Err(efi::Status::INVALID_PARAMETER),
            x if x == EventType::GenericEvent as u32 => Ok(EventType::GenericEvent),
            x if x == EventType::TimerNotifyWaitEvent as u32 => Ok(EventType::TimerNotifyWaitEvent),
            _ => Err(efi::Status::INVALID_PARAMETER),
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
    TimerCancel,
    /// Creates a periodic timer
    TimerPeriodic,
    /// Creates a one-shot relative timer
    TimerRelative,
}

impl TryFrom<u32> for TimerDelay {
    type Error = efi::Status;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            x if x == TimerDelay::TimerCancel as u32 => Ok(TimerDelay::TimerCancel),
            x if x == TimerDelay::TimerPeriodic as u32 => Ok(TimerDelay::TimerPeriodic),
            x if x == TimerDelay::TimerRelative as u32 => Ok(TimerDelay::TimerRelative),
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
        if self.0.event == other.0.event {
            Ordering::Equal
        } else if self.0.notify_tpl == other.0.notify_tpl {
            self.1.cmp(&other.1)
        } else {
            other.0.notify_tpl.cmp(&self.0.notify_tpl)
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
    ) -> Result<Self, efi::Status> {
        let notifiable = (event_type & (efi::EVT_NOTIFY_SIGNAL | efi::EVT_NOTIFY_WAIT)) != 0;
        let event_type: EventType = event_type.try_into()?;

        if notifiable {
            if notify_function.is_none() {
                return Err(efi::Status::INVALID_PARAMETER);
            }

            // Pedantic check; this will probably not work with "real firmware", so
            // loosen up a bit.
            // match notify_tpl {
            //     efi::TPL_APPLICATION | efi::TPL_CALLBACK | efi::TPL_NOTIFY | efi::TPL_HIGH_LEVEL => (),
            //     _ => return Err(efi::Status::INVALID_PARAMETER),
            // }
            if !((efi::TPL_APPLICATION + 1)..=efi::TPL_HIGH_LEVEL).contains(&notify_tpl) {
                return Err(efi::Status::INVALID_PARAMETER);
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
}

pub struct EventDb {
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
    pub const fn new() -> Self {
        EventDb { events: BTreeMap::new(), next_event_id: 1, pending_notifies: BTreeSet::new(), notify_tags: 0 }
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
    ///
    /// ## Examples
    ///
    /// ```
    /// use uefi_event::EventDb;
    ///
    /// let mut event = EventDb::new();
    /// let result = event.create_event(
    ///   0,
    ///   0,
    ///   None,
    ///   None,
    ///   None,
    /// );
    /// assert_ne!(result.unwrap(), core::ptr::null_mut());
    /// ```
    pub fn create_event(
        &mut self,
        event_type: u32,
        notify_tpl: r_efi::base::Tpl,
        notify_function: Option<efi::EventNotify>,
        notify_context: Option<*mut c_void>,
        event_group: Option<efi::Guid>,
    ) -> Result<efi::Event, efi::Status> {
        let id = self.next_event_id;
        self.next_event_id += 1;
        let event = Event::new(id, event_type, notify_tpl, notify_function, notify_context, event_group)?;
        self.events.insert(id, event);
        Ok(id as efi::Event)
    }

    /// Closes (deletes) an event from the event database
    ///
    /// This function closely matches the semantics of the EFI_BOOT_SERVICES.CloseEvent() API in
    /// UEFI spec 2.10 section 7.1.3. Please refer to the spec for details on the input parameters.
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    ///
    /// ## Examples
    ///
    /// ```
    /// use uefi_event::EventDb;
    ///
    /// let mut event_db = EventDb::new();
    /// let result = event_db.create_event(
    ///   0,
    ///   0,
    ///   None,
    ///   None,
    ///   None,
    /// );
    /// let result = event_db.close_event(result.unwrap());
    /// assert_eq!(result, Ok(()));
    /// ```
    pub fn close_event(&mut self, event: efi::Event) -> Result<(), efi::Status> {
        let id = event as usize;
        self.events.remove(&id).ok_or(efi::Status::INVALID_PARAMETER)?;
        Ok(())
    }

    //private helper function for signal_event.
    fn queue_notify_event(pending_notifies: &mut BTreeSet<TaggedEventNotification>, event: &mut Event, tag: u64) {
        if event.event_type.is_notify_signal() || event.event_type.is_notify_wait() {
            pending_notifies.insert(TaggedEventNotification(
                EventNotification {
                    event: event.event_id as efi::Event,
                    notify_tpl: event.notify_tpl,
                    notify_function: event.notify_function,
                    notify_context: event.notify_context,
                },
                tag,
            ));
        }
    }

    /// Marks an event as signaled, and queues it for dispatch if it is of type NotifySignalEvent
    ///
    /// This function closely matches the semantics of the EFI_BOOT_SERVICES.SignalEvent() API in
    /// UEFI spec 2.10 section 7.1.4. Please refer to the spec for details on the input parameters.
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    ///
    /// ## Examples
    ///
    /// ```
    /// use uefi_event::EventDb;
    ///
    /// let mut event_db = EventDb::new();
    /// let result = event_db.create_event(
    ///   0,
    ///   0,
    ///   None,
    ///   None,
    ///   None,
    /// );
    /// let handle = result.unwrap();
    /// let result = event_db.signal_event(handle);
    /// assert_eq!(result, Ok(()));
    /// assert!(event_db.is_signaled(handle));
    /// ```
    pub fn signal_event(&mut self, event: efi::Event) -> Result<(), efi::Status> {
        let id = event as usize;
        let current_event = self.events.get_mut(&id).ok_or(efi::Status::INVALID_PARAMETER)?;

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

    /// Signals an event group
    ///
    /// This routine signals all events in the given event group. There isn't an equivalent UEFI spec API for this; the
    /// equivalent would need to be accomplished by creating a dummy event that is a member of the group and signalling
    /// that event.
    ///
    /// ##Examples
    ///
    /// ```
    /// use r_efi::efi;
    /// use std::str::FromStr;
    /// use uuid::Uuid;
    /// use uefi_event::EventDb;
    /// let uuid = Uuid::from_str("aefcf33c-ce02-47b4-89f6-4bacdeda3377").unwrap();
    /// let group1: efi::Guid = unsafe { core::mem::transmute(*uuid.as_bytes()) };
    /// let mut event_db = EventDb::new();
    /// let result = event_db.create_event(
    ///   0,
    ///   0,
    ///   None,
    ///   None,
    ///   Some(group1),
    /// );
    /// let handle = result.unwrap();
    /// let result = event_db.signal_group(group1);
    /// assert!(event_db.is_signaled(handle));
    /// ```
    pub fn signal_group(&mut self, group: efi::Guid) {
        for member_event in self.events.values_mut().filter(|e| e.event_group == Some(group)) {
            member_event.signaled = true;
            if member_event.event_type.is_notify_signal() {
                Self::queue_notify_event(&mut self.pending_notifies, member_event, self.notify_tags);
                self.notify_tags += 1;
            }
        }
    }

    /// Clears the signaled state for the given event.
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    ///
    /// ## Examples
    ///
    /// ```
    /// use uefi_event::EventDb;
    ///
    /// let mut event_db = EventDb::new();
    /// let result = event_db.create_event(
    ///   0,
    ///   0,
    ///   None,
    ///   None,
    ///   None,
    /// );
    /// let handle = result.unwrap();
    /// event_db.signal_event(handle);
    /// assert!(event_db.is_signaled(handle));
    /// SPIN_LOCKEDevent_db_EVENT_DB.clear_signal(handle);
    /// assert!(!event_db.is_signaled(handle));
    /// ```
    pub fn clear_signal(&mut self, event: efi::Event) -> Result<(), efi::Status> {
        let id = event as usize;
        let event = self.events.get_mut(&id).ok_or(efi::Status::INVALID_PARAMETER)?;
        event.signaled = false;
        Ok(())
    }

    /// Indicates whether the given event is in the signaled state
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    ///
    /// ## Examples
    ///
    /// ```
    /// use uefi_event::EventDb;
    ///
    /// let mut event_db = EventDb::new();
    /// let result = event_db.create_event(
    ///   0,
    ///   0,
    ///   None,
    ///   None,
    ///   None,
    /// );
    /// let handle = result.unwrap();
    /// let result = event_db.signal_event(handle);
    /// assert_eq!(result, Ok(()));
    /// assert!(event_db.is_signaled(handle));
    /// ```
    pub fn is_signaled(&self, event: efi::Event) -> bool {
        let id = event as usize;
        if let Some(event) = self.events.get(&id) {
            event.signaled
        } else {
            false
        }
    }

    /// Queues the notify for the given event.
    ///
    /// Queued events can be retrieved via [`event_notification_iter`](SpinLockedEventDb::event_notification_iter).
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    ///
    /// ## Examples
    ///
    /// ```
    /// use uefi_event::*;
    /// use r_efi::efi;
    ///
    /// extern "efiapi" fn notify_function(_:efi::Event, _:*mut core::ffi::c_void) {}
    ///
    /// let mut event_db = EventDb::new();
    /// let result = event_db.create_event(
    ///   efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
    ///   efi::TPL_CALLBACK,
    ///   Some(notify_function),
    ///   None,
    ///   None,
    /// );
    /// let handle = result.unwrap();
    ///
    /// event_db.queue_event_notify(handle).unwrap();
    /// assert_eq!(
    ///   event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>().len(),
    ///   1
    /// );
    /// ```
    pub fn queue_event_notify(&mut self, event: efi::Event) -> Result<(), efi::Status> {
        let id = event as usize;
        let current_event = self.events.get_mut(&id).ok_or(efi::Status::INVALID_PARAMETER)?;

        Self::queue_notify_event(&mut self.pending_notifies, current_event, self.notify_tags);
        self.notify_tags += 1;

        Ok(())
    }

    /// Returns the event type for the given event
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect event is given.
    ///
    /// ## Examples
    ///
    /// ```
    /// use uefi_event::EventDb;
    /// use uefi_event::EventType::GenericEvent;
    ///
    /// let mut event_db = EventDb::new();
    /// let result = event_db.create_event(
    ///   0,
    ///   0,
    ///   None,
    ///   None,
    ///   None,
    /// );
    /// let result = event_db.get_event_type(result.unwrap());
    /// assert_eq!(result, Ok(GenericEvent));
    /// ```
    pub fn get_event_type(&mut self, event: efi::Event) -> Result<EventType, efi::Status> {
        let id = event as usize;
        Ok(self.events.get(&id).ok_or(efi::Status::INVALID_PARAMETER)?.event_type)
    }

    /// Returns the notification data associated with the event.
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    ///
    /// ## Examples
    ///
    /// ```
    /// use uefi_event::*;
    /// use core::ffi::c_void;
    /// use r_efi::efi;
    ///
    /// extern "efiapi" fn notify_function(_:efi::Event, _:*mut core::ffi::c_void) {}
    ///
    /// let notify_context = 0x1234 as *mut c_void;
    /// let mut event_db = EventDb::new();
    /// let result = event_db.create_event(
    ///   efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
    ///   efi::TPL_CALLBACK,
    ///   Some(notify_function),
    ///   Some(notify_context),
    ///   None,
    /// );
    /// let handle = result.unwrap();
    ///
    /// let notification_data = event_db.get_notification_data(handle).unwrap();
    /// assert_eq!(notification_data.notify_tpl, efi::TPL_CALLBACK);
    /// assert_eq!(notification_data.notify_function.unwrap() as usize, notify_function as usize);
    /// assert_eq!(notification_data.notify_context, Some(notify_context));
    /// ```
    pub fn get_notification_data(&mut self, event: efi::Event) -> Result<EventNotification, efi::Status> {
        let id = event as usize;
        if let Some(found_event) = self.events.get(&id) {
            if (found_event.event_type as u32) & (efi::EVT_NOTIFY_SIGNAL | efi::EVT_NOTIFY_WAIT) == 0 {
                return Err(efi::Status::NOT_FOUND);
            }
            Ok(EventNotification {
                event,
                notify_tpl: found_event.notify_tpl,
                notify_function: found_event.notify_function,
                notify_context: found_event.notify_context,
            })
        } else {
            Err(efi::Status::NOT_FOUND)
        }
    }

    /// Sets a timer on the specified event
    ///
    /// [`timer_tick`](EventDb::timer_tick) is used to advanced time; when a timer expires, the corresponding
    /// event is queued and can be retrieved via [`event_notification_iter`](SpinLockedEventDb::event_notification_iter).
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    ///
    /// ## Examples
    ///
    /// ```
    /// use uefi_event::{EventDb, TimerDelay, EventNotification};
    /// use core::ffi::c_void;
    /// use r_efi::efi;
    ///
    /// extern "efiapi" fn notify_function(_:efi::Event, _:*mut core::ffi::c_void) {};
    ///
    /// let notify_context = 0x1234 as *mut c_void;
    /// let mut event_db = EventDb::new();
    /// let result = event_db.create_event(
    ///   efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
    ///   efi::TPL_CALLBACK,
    ///   Some(notify_function),
    ///   Some(notify_context),
    ///   None,
    /// );
    /// let handle = result.unwrap();
    ///
    /// event_db.set_timer(handle, TimerDelay::TimerRelative, Some(0x100), None).unwrap();
    /// event_db.timer_tick(0x200);
    /// assert_eq!(
    ///   event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>().len(),
    ///   1
    /// );
    /// ```
    pub fn set_timer(
        &mut self,
        event: efi::Event,
        timer_type: TimerDelay,
        trigger_time: Option<u64>,
        period: Option<u64>,
    ) -> Result<(), efi::Status> {
        let id = event as usize;
        if let Some(event) = self.events.get_mut(&id) {
            if !event.event_type.is_timer() {
                return Err(efi::Status::INVALID_PARAMETER);
            }
            match timer_type {
                TimerDelay::TimerCancel => {
                    if trigger_time.is_some() || period.is_some() {
                        return Err(efi::Status::INVALID_PARAMETER);
                    }
                }
                TimerDelay::TimerPeriodic => {
                    if trigger_time.is_none() || period.is_none() {
                        return Err(efi::Status::INVALID_PARAMETER);
                    }
                }
                TimerDelay::TimerRelative => {
                    if trigger_time.is_none() || period.is_some() {
                        return Err(efi::Status::INVALID_PARAMETER);
                    }
                }
            }
            event.trigger_time = trigger_time;
            event.period = period;
            Ok(())
        } else {
            Err(efi::Status::INVALID_PARAMETER)
        }
    }

    /// called to advance the system time and process any timer events that fire
    ///
    /// [`set_timer`](EventDb::set_timer) is used to configure timers with either a one-shot or periodic
    /// timer.
    ///
    /// This routine is called to inform the event database that that a certain amount of time has passed. The event
    /// database will iterate over all events and determine if any of the timers have expired based on the amount of
    /// time that has passed per this call. If any timers are expired, the corresponding events will be signaled.
    ///
    /// signaled events with notifications are queued and can be retrieved via
    /// [`event_notification_iter`](EventDb::event_notification_iter).
    ///
    /// ## Examples
    ///
    /// ```
    /// use uefi_event::{EventDb, TimerDelay, EventNotification};
    /// use core::ffi::c_void;
    /// use r_efi::efi;
    ///
    /// extern "efiapi" fn notify_function(_:efi::Event, _:*mut core::ffi::c_void) {}
    ///
    /// let notify_context = 0x1234 as *mut c_void;
    /// let mut event_db = EventDb::new();
    /// let result = event_db.create_event(
    ///   efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
    ///   efi::TPL_CALLBACK,
    ///   Some(notify_function),
    ///   Some(notify_context),
    ///   None,
    /// );
    /// let handle = result.unwrap();
    ///
    /// event_db.set_timer(handle, TimerDelay::TimerRelative, Some(0x100), None).unwrap();
    /// event_db.timer_tick(0x200);
    /// assert_eq!(
    ///   event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>().len(),
    ///   1
    /// );
    /// ```
    pub fn timer_tick(&mut self, current_time: u64) {
        let events: Vec<usize> = self.events.keys().cloned().collect();
        for event in events {
            let current_event = if let Some(current) = self.events.get_mut(&event) {
                current
            } else {
                debug_assert!(false, "Event {:?} not found.", event);
                log::error!("Event {:?} not found.", event);
                continue;
            };
            if current_event.event_type.is_timer() {
                if let Some(trigger_time) = current_event.trigger_time {
                    if trigger_time <= current_time {
                        if let Some(period) = current_event.period {
                            current_event.trigger_time = Some(current_time + period);
                        } else {
                            //no period means it's a one-shot event; another call to set_timer is required to "re-arm"
                            current_event.trigger_time = None;
                        }
                        if let Err(e) = self.signal_event(event as *mut c_void) {
                            log::error!("Error {:?} signaling event {:?}.", e, event);
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn consume_next_event_notify(&mut self, tpl_level: efi::Tpl) -> Option<EventNotification> {
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
                self.events.get_mut(&(item.0.event as usize))?.signaled = false;
                return Some(item.0);
            } else {
                log::error!("Pending_notifies was empty, but it should have at least one item.");
            }
        }
        None
    }

    /// Indicates whether a given event is valid.
    ///
    /// ## Examples
    ///
    /// ```
    /// use uefi_event::EventDb;
    ///
    /// let mut event_db = EventDb::new();
    /// let result = event_db.create_event(
    ///   0,
    ///   0,
    ///   None,
    ///   None,
    ///   None,
    /// );
    /// let handle = result.unwrap();
    /// assert!(event_db.is_valid(handle));
    /// ```
    pub fn is_valid(&mut self, event: efi::Event) -> bool {
        self.events.contains_key(&(event as usize))
    }

    /// Atomically reads and clears the signaled state.
    ///
    /// ## Errors
    ///
    /// Returns r_efi:efi::Status::INVALID_PARAMETER if incorrect parameters are given.
    ///
    /// ## Examples
    ///
    /// ```
    /// use uefi_event::EventDb;
    ///
    /// let mut event_db = EventDb::new();
    /// let result = event_db.create_event(
    ///   0,
    ///   0,
    ///   None,
    ///   None,
    ///   None,
    /// );
    /// let handle = result.unwrap();
    ///
    /// event_db.signal_event(handle);
    /// assert!(event_db.is_signaled(handle));
    ///
    /// let result = event_db.read_and_clear_signaled(handle);
    /// assert_eq!(result, Ok(true));
    /// assert!(!event_db.is_signaled(handle));
    ///
    /// let result = event_db.read_and_clear_signaled(handle);
    /// assert_eq!(result, Ok(false));
    /// ```
    pub fn read_and_clear_signaled(&mut self, event: efi::Event) -> Result<bool, efi::Status> {
        let signaled = self.is_signaled(event);
        if signaled {
            self.clear_signal(event)?;
        }
        Ok(signaled)
    }

    /// Returns an iterator over pending event notifications that should be dispatched at or above the given efi::TPL level.
    ///
    /// Events can be added to the pending queue directly via
    /// [`queue_event_notify`](SpinLockedEventDb::queue_event_notify) or via timer expiration configured via
    /// [`set_timer`](SpinLockedEventDb::set_timer) followed by a [`timer_tick`](SpinLockedEventDb::timer_tick) that
    /// causes the timer to expire.
    ///
    /// Any new events added to the dispatch queue between calls to next() on the iterator will also be returned by the
    /// iterator - the iterator will only stop if there are no pending dispatches at or above the given efi::TPL on a call to
    /// next().
    ///
    /// ## Examples
    ///
    /// ```
    /// use uefi_event::{EventDb, TimerDelay, EventNotification};
    /// use core::ffi::c_void;
    /// use r_efi::efi;
    ///
    /// extern "efiapi" fn notify_function(_:efi::Event, _:*mut core::ffi::c_void) {}
    ///
    /// let notify_context = 0x1234 as *mut c_void;
    /// let mut event_db = EventDb::new();
    /// let result = event_db.create_event(
    ///   efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
    ///   efi::TPL_CALLBACK,
    ///   Some(notify_function),
    ///   Some(notify_context),
    ///   None,
    /// );
    /// let handle = result.unwrap();
    ///
    /// event_db.set_timer(handle, TimerDelay::TimerRelative, Some(0x100), None).unwrap();
    /// event_db.timer_tick(0x200);
    /// assert_eq!(
    ///   event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>().len(),
    ///   1
    /// );
    /// ```
    pub fn event_notification_iter<'a>(&'a mut self, tpl_level: efi::Tpl) -> impl Iterator<Item = EventNotification> + 'a {
        EventNotificationIterator::new(self, tpl_level)
    }
}

struct EventNotificationIterator<'a> {
    event_db: &'a mut EventDb,
    tpl_level: efi::Tpl,
}

impl<'a> EventNotificationIterator<'a> {
    fn new(event_db: &'a mut EventDb, tpl_level: efi::Tpl) -> Self {
        EventNotificationIterator { event_db, tpl_level }
    }
}

impl<'a> Iterator for EventNotificationIterator<'a> {
    type Item = EventNotification;
    fn next(&mut self) -> Option<EventNotification> {
        self.event_db.consume_next_event_notify(self.tpl_level)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use core::str::FromStr;

    use alloc::{vec, vec::Vec};
    use r_efi::efi;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn new_should_create_event_db_local() {
        let event_db = EventDb::new();
        let events = &event_db.events;
        assert_eq!(events.len(), 0);
    }

    #[test]
    fn new_should_create_event_db() {
        let event_db = EventDb::new();
        assert_eq!(event_db.events.len(), 0)
    }

    extern "efiapi" fn test_notify_function(_: efi::Event, _: *mut core::ffi::c_void) {}

    #[test]
    fn create_event_should_create_event() {
        let mut event_db = EventDb::new();
        let result = event_db.create_event(
            efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
            efi::TPL_NOTIFY,
            Some(test_notify_function),
            None,
            None,
        );
        assert!(result.is_ok());
        let event = result.unwrap();
        let index = event as usize;
        assert!(index < event_db.next_event_id);
        let events = &event_db.events;
        assert_eq!(events.get(&index).unwrap().event_type, EventType::TimerNotifyEvent);
        assert_eq!(events.get(&index).unwrap().event_type as u32, efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL);
        assert_eq!(events.get(&index).unwrap().notify_tpl, efi::TPL_NOTIFY);
        assert_eq!(events.get(&index).unwrap().notify_function.unwrap() as usize, test_notify_function as usize);
        assert_eq!(events.get(&index).unwrap().notify_context, None);
        assert_eq!(events.get(&index).unwrap().event_group, None);
    }

    #[test]
    fn create_event_with_bad_input_should_not_create_event() {
        let mut event_db = EventDb::new();

        //Try with an invalid event type.
        let result =
            event_db.create_event(efi::EVT_SIGNAL_EXIT_BOOT_SERVICES, efi::TPL_NOTIFY, None, None, None);
        assert_eq!(result, Err(efi::Status::INVALID_PARAMETER));

        //if type has efi::EVT_NOTIFY_SIGNAL or efi::EVT_NOTIFY_WAIT, then NotifyFunction must be non-NULL and NotifyTpl must be a valid efi::TPL.
        //Try to create a notified event with None notify_function - should fail.
        let result = event_db.create_event(
            efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
            efi::TPL_NOTIFY,
            None,
            None,
            None,
        );
        assert_eq!(result, Err(efi::Status::INVALID_PARAMETER));

        //Try to create a notified event with Some notify_function but invalid efi::TPL - should fail.
        let result = event_db.create_event(
            efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
            efi::TPL_HIGH_LEVEL + 1,
            Some(test_notify_function),
            None,
            None,
        );
        assert_eq!(result, Err(efi::Status::INVALID_PARAMETER));
    }

    #[test]
    fn close_event_should_delete_event() {
        let mut event_db = EventDb::new();
        let mut events: Vec<efi::Event> = Vec::new();
        for _ in 0..10 {
            events.push(
                event_db
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
            assert!(event_db.is_valid(event));
            let result = event_db.close_event(event);
            assert!(result.is_ok());
            assert_eq!(event_db.events.len(), 10 - consumed);
            assert!(!event_db.is_valid(event));
        }
    }

    #[test]
    fn signal_event_should_put_events_in_signaled_state() {
        let mut event_db = EventDb::new();
        let mut events: Vec<efi::Event> = Vec::new();
        for _ in 0..10 {
            events.push(
                event_db
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
            let result: Result<(), efi::Status> = event_db.signal_event(event);
            assert!(result.is_ok());
            assert!(event_db.is_signaled(event));
        }
    }

    #[test]
    fn signal_event_on_an_event_group_should_put_all_members_in_signaled_state() {
        let uuid = Uuid::from_str("aefcf33c-ce02-47b4-89f6-4bacdeda3377").unwrap();
        let group1: efi::Guid = unsafe { core::mem::transmute(*uuid.as_bytes()) };
        let uuid = Uuid::from_str("3a08a8c7-054b-4268-8aed-bc6a3aef999f").unwrap();
        let group2: efi::Guid = unsafe { core::mem::transmute(*uuid.as_bytes()) };
        let uuid = Uuid::from_str("745e8316-4889-4f58-be3c-6b718b7170ec").unwrap();
        let group3: efi::Guid = unsafe { core::mem::transmute(*uuid.as_bytes()) };

        let mut event_db = EventDb::new();
        let mut group1_events: Vec<efi::Event> = Vec::new();
        let mut group2_events: Vec<efi::Event> = Vec::new();
        let mut group3_events: Vec<efi::Event> = Vec::new();
        let mut ungrouped_events: Vec<efi::Event> = Vec::new();

        for _ in 0..10 {
            group1_events.push(
                event_db
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
                event_db
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
                event_db
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
                event_db
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
        event_db.signal_event(ungrouped_events.pop().unwrap()).unwrap();

        //all other events should remain un-signaled
        for event in group1_events.clone() {
            assert!(!event_db.is_signaled(event));
        }

        for event in group2_events.clone() {
            assert!(!event_db.is_signaled(event));
        }

        for event in ungrouped_events.clone() {
            assert!(!event_db.is_signaled(event));
        }

        //signal an event in a group
        event_db.signal_event(group1_events[0]).unwrap();

        //events in the same group should be signaled.
        for event in group1_events.clone() {
            assert!(event_db.is_signaled(event));
        }

        //events in another group should not be signaled.
        for event in group2_events.clone() {
            assert!(!event_db.is_signaled(event));
        }

        //ungrouped events should not be signaled.
        for event in ungrouped_events.clone() {
            assert!(!event_db.is_signaled(event));
        }

        //signal an event in a different group
        event_db.signal_event(group2_events[0]).unwrap();

        //first event group should remain signaled.
        for event in group1_events.clone() {
            assert!(event_db.is_signaled(event));
        }

        //second event group should now be signaled.
        for event in group2_events.clone() {
            assert!(event_db.is_signaled(event));
        }

        //third event group should not be signaled.
        for event in group3_events.clone() {
            assert!(!event_db.is_signaled(event));
        }

        //signal events in third group using signal_group
        event_db.signal_group(group3);
        //first event group should remain signaled.
        for event in group1_events.clone() {
            assert!(event_db.is_signaled(event));
        }

        //second event group should remain signaled.
        for event in group2_events.clone() {
            assert!(event_db.is_signaled(event));
        }

        //third event group should now be signaled.
        for event in group3_events.clone() {
            assert!(event_db.is_signaled(event));
        }

        //ungrouped events should not be signaled.
        for event in ungrouped_events.clone() {
            assert!(!event_db.is_signaled(event));
        }
    }

    #[test]
    fn clear_signal_should_clear_signaled_state() {
        let mut event_db = EventDb::new();
        let event = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        event_db.signal_event(event).unwrap();
        assert!(event_db.is_signaled(event));
        let result = event_db.clear_signal(event);
        assert!(result.is_ok());
        assert!(!event_db.is_signaled(event));
    }

    #[test]
    fn is_signaled_should_return_false_for_closed_or_non_existent_event() {
        let mut event_db = EventDb::new();
        let event = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        event_db.signal_event(event).unwrap();
        assert!(event_db.is_signaled(event));
        event_db.close_event(event).unwrap();
        assert!(!event_db.is_signaled(event));
        assert!(!event_db.is_signaled(0x1234 as *mut c_void));
    }

    #[test]
    fn signaled_events_with_notifies_should_be_put_in_pending_queue_in_tpl_order() {
        let mut event_db = EventDb::new();
        let callback_evt1 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_CALLBACK,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        let callback_evt2 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_CALLBACK,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        let notify_evt1 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        let notify_evt2 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        let high_evt1 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_HIGH_LEVEL,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        let high_evt2 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_HIGH_LEVEL,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        event_db.signal_event(callback_evt1).unwrap();
        event_db.signal_event(notify_evt1).unwrap();
        event_db.signal_event(high_evt1).unwrap();

        event_db.signal_event(callback_evt2).unwrap();
        event_db.signal_event(notify_evt2).unwrap();
        event_db.signal_event(high_evt2).unwrap();

        {
            let mut event_db = event_db;
            let queue = &mut event_db.pending_notifies;
            assert_eq!(queue.pop_first().unwrap().0.event, high_evt1);
            assert_eq!(queue.pop_first().unwrap().0.event, high_evt2);
            assert_eq!(queue.pop_first().unwrap().0.event, notify_evt1);
            assert_eq!(queue.pop_first().unwrap().0.event, notify_evt2);
            assert_eq!(queue.pop_first().unwrap().0.event, callback_evt1);
            assert_eq!(queue.pop_first().unwrap().0.event, callback_evt2);
        }
    }

    #[test]
    fn signaled_event_iterator_should_return_next_events_in_tpl_order() {
        let mut event_db = EventDb::new();

        assert_eq!(
            event_db
                .event_notification_iter(efi::TPL_APPLICATION)
                .collect::<Vec<EventNotification>>()
                .len(),
            0
        );

        let callback_evt1 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_CALLBACK,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        let callback_evt2 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_CALLBACK,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        let notify_evt1 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        let notify_evt2 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        let high_evt1 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_HIGH_LEVEL,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        let high_evt2 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_HIGH_LEVEL,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();
        event_db.signal_event(callback_evt1).unwrap();
        event_db.signal_event(notify_evt1).unwrap();
        event_db.signal_event(high_evt1).unwrap();

        event_db.signal_event(callback_evt2).unwrap();
        event_db.signal_event(notify_evt2).unwrap();
        event_db.signal_event(high_evt2).unwrap();

        let events = event_db.event_notification_iter(efi::TPL_NOTIFY).zip(vec![high_evt1, high_evt2]).collect::<Vec<_>>();

        for (event_notification, expected_event) in events {
            assert_eq!(event_notification.event, expected_event);
            assert!(!event_db.is_signaled(expected_event));
        }

        //re-signal the consumed events
        event_db.signal_event(high_evt1).unwrap();
        event_db.signal_event(high_evt2).unwrap();

        let events = event_db.event_notification_iter(efi::TPL_CALLBACK).zip(vec![high_evt1, high_evt2, notify_evt1, notify_evt2]).collect::<Vec<_>>();
        for (event_notification, expected_event) in events {
            assert_eq!(event_notification.event, expected_event);
            assert!(!event_db.is_signaled(expected_event));
        }

        //re-signal the consumed events
        event_db.signal_event(high_evt1).unwrap();
        event_db.signal_event(high_evt2).unwrap();
        event_db.signal_event(notify_evt1).unwrap();
        event_db.signal_event(notify_evt2).unwrap();

        let events = event_db.event_notification_iter(efi::TPL_CALLBACK).zip(vec![high_evt1, high_evt2, notify_evt1, notify_evt2, callback_evt1, callback_evt2]).collect::<Vec<_>>();
        for (event_notification, expected_event) in events {
            assert_eq!(event_notification.event, expected_event);
            assert!(!event_db.is_signaled(expected_event));
        }

        //re-signal the consumed events
        event_db.signal_event(high_evt1).unwrap();
        event_db.signal_event(high_evt2).unwrap();
        event_db.signal_event(notify_evt1).unwrap();
        event_db.signal_event(notify_evt2).unwrap();
        event_db.signal_event(callback_evt1).unwrap();
        event_db.signal_event(callback_evt2).unwrap();

        //close or clear some of the events before consuming
        event_db.close_event(high_evt1).unwrap();
        event_db.close_event(notify_evt1).unwrap();
        event_db.close_event(callback_evt1).unwrap();

        let events = event_db.event_notification_iter(efi::TPL_APPLICATION).zip(vec![high_evt2, notify_evt2, callback_evt2]).collect::<Vec<_>>();
        for (event_notification, expected_event) in events {
            assert_eq!(event_notification.event, expected_event);
            assert!(!event_db.is_signaled(expected_event));
        }
    }

    #[test]
    fn signalling_an_event_more_than_once_should_not_queue_it_more_than_once() {
        let mut event_db = EventDb::new();

        let callback_evt1 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_CALLBACK,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();

        event_db.signal_event(callback_evt1).unwrap();
        event_db.signal_event(callback_evt1).unwrap();
        event_db.signal_event(callback_evt1).unwrap();
        event_db.signal_event(callback_evt1).unwrap();
        event_db.signal_event(callback_evt1).unwrap();

        assert_eq!(event_db.pending_notifies.len(), 1);
        assert_eq!(
            event_db
                .event_notification_iter(efi::TPL_APPLICATION)
                .collect::<Vec<EventNotification>>()
                .len(),
            1
        );
    }

    #[test]
    fn read_and_clear_signaled_should_clear_signal() {
        let mut event_db = EventDb::new();

        let callback_evt1 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_CALLBACK,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();

        event_db.signal_event(callback_evt1).unwrap();

        assert_eq!(event_db.pending_notifies.len(), 1);

        let result = event_db.read_and_clear_signaled(callback_evt1);
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result);
        let result = event_db.read_and_clear_signaled(callback_evt1);
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(!result);
    }

    #[test]
    fn signalling_a_notify_wait_event_should_not_queue_it() {
        let mut event_db = EventDb::new();

        let callback_evt1 = event_db
            .create_event(efi::EVT_NOTIFY_WAIT, efi::TPL_CALLBACK, Some(test_notify_function), None, None)
            .unwrap();

        event_db.signal_event(callback_evt1).unwrap();

        assert_eq!(
            event_db
                .event_notification_iter(efi::TPL_APPLICATION)
                .collect::<Vec<EventNotification>>()
                .len(),
            0
        );
    }

    #[test]
    fn queue_event_notify_should_queue_event_notify() {
        let mut event_db = EventDb::new();

        let callback_evt1 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_CALLBACK,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();

        event_db.queue_event_notify(callback_evt1).unwrap();
        event_db.queue_event_notify(callback_evt1).unwrap();
        event_db.queue_event_notify(callback_evt1).unwrap();
        event_db.queue_event_notify(callback_evt1).unwrap();
        event_db.queue_event_notify(callback_evt1).unwrap();

        assert_eq!(
            event_db
                .event_notification_iter(efi::TPL_APPLICATION)
                .collect::<Vec<EventNotification>>()
                .len(),
            1
        );
    }

    #[test]
    fn queue_event_notify_should_work_for_both_notify_wait_and_notify_signal() {
        let mut event_db = EventDb::new();

        let callback_evt1 = event_db
            .create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(test_notify_function), None, None)
            .unwrap();

        let callback_evt2 = event_db
            .create_event(efi::EVT_NOTIFY_WAIT, efi::TPL_CALLBACK, Some(test_notify_function), None, None)
            .unwrap();

        event_db.queue_event_notify(callback_evt1).unwrap();
        event_db.queue_event_notify(callback_evt2).unwrap();

        assert_eq!(
            event_db
                .event_notification_iter(efi::TPL_APPLICATION)
                .collect::<Vec<EventNotification>>()
                .len(),
            2
        );
    }

    #[test]
    fn get_event_type_should_return_event_type() {
        let mut event_db = EventDb::new();
        let event = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();

        let result = event_db.get_event_type(event);
        assert_eq!(result.unwrap(), EventType::TimerNotifyEvent);

        let event = (event as usize + 1) as *mut c_void;
        let result = event_db.get_event_type(event);
        assert_eq!(result, Err(efi::Status::INVALID_PARAMETER));
    }

    #[test]
    fn get_notification_data_should_return_notification_data() {
        let mut event_db = EventDb::new();
        let test_context: *mut c_void = 0x1234 as *mut c_void;
        let event = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                Some(test_context),
                None,
            )
            .unwrap();

        let notification_data = event_db.get_notification_data(event);
        assert!(notification_data.is_ok());
        let event_notification = notification_data.unwrap();
        assert_eq!(event_notification.notify_tpl, efi::TPL_NOTIFY);
        assert_eq!(event_notification.notify_function.unwrap() as usize, test_notify_function as usize);
        assert_eq!(event_notification.notify_context.unwrap(), test_context);

        let event = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();

        let notification_data = event_db.get_notification_data(event);
        assert!(notification_data.is_ok());
        let event_notification = notification_data.unwrap();
        assert_eq!(event_notification.notify_tpl, efi::TPL_NOTIFY);
        assert_eq!(event_notification.notify_function.unwrap() as usize, test_notify_function as usize);
        assert!(event_notification.notify_context.is_none());

        let event = event_db.create_event(efi::EVT_TIMER, efi::TPL_NOTIFY, None, None, None).unwrap();
        let notification_data = event_db.get_notification_data(event);
        assert_eq!(notification_data.err(), Some(efi::Status::NOT_FOUND));

        let notification_data = event_db.get_notification_data(0x1234 as *mut c_void);
        assert_eq!(notification_data.err(), Some(efi::Status::NOT_FOUND));
    }

    #[test]
    fn set_timer_on_event_should_set_timer_on_event() {
        let mut event_db = EventDb::new();
        let event = event_db
            .create_event(efi::EVT_TIMER, efi::TPL_NOTIFY, Some(test_notify_function), None, None)
            .unwrap();

        let index = event as usize;

        let result = event_db.set_timer(event, TimerDelay::TimerRelative, Some(0x100), None);
        assert!(result.is_ok());
        {
            let events = &event_db.events;
            assert_eq!(events.get(&index).unwrap().trigger_time, Some(0x100));
            assert_eq!(events.get(&index).unwrap().period, None);
        }

        let result = event_db.set_timer(event, TimerDelay::TimerPeriodic, Some(0x100), Some(0x200));
        assert!(result.is_ok());
        {
            let events = &event_db.events;
            assert_eq!(events.get(&index).unwrap().trigger_time, Some(0x100));
            assert_eq!(events.get(&index).unwrap().period, Some(0x200));
        }

        let result = event_db.set_timer(event, TimerDelay::TimerCancel, None, None);
        assert!(result.is_ok());
        {
            let events = &event_db.events;
            assert_eq!(events.get(&index).unwrap().trigger_time, None);
            assert_eq!(events.get(&index).unwrap().period, None);
        }

        let event = event_db
            .create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_NOTIFY, Some(test_notify_function), None, None)
            .unwrap();

        let result = event_db.set_timer(event, TimerDelay::TimerPeriodic, Some(0x100), Some(0x200));
        assert_eq!(result.err(), Some(efi::Status::INVALID_PARAMETER));

        let event = event_db
            .create_event(efi::EVT_TIMER, efi::TPL_NOTIFY, Some(test_notify_function), None, None)
            .unwrap();
        let result = event_db.set_timer(event, TimerDelay::TimerCancel, Some(0x100), None);
        assert_eq!(result.err(), Some(efi::Status::INVALID_PARAMETER));

        let event = event_db
            .create_event(efi::EVT_TIMER, efi::TPL_NOTIFY, Some(test_notify_function), None, None)
            .unwrap();
        let result = event_db.set_timer(event, TimerDelay::TimerPeriodic, None, None);
        assert_eq!(result.err(), Some(efi::Status::INVALID_PARAMETER));

        let event = event_db
            .create_event(efi::EVT_TIMER, efi::TPL_NOTIFY, Some(test_notify_function), None, None)
            .unwrap();
        let result = event_db.set_timer(event, TimerDelay::TimerRelative, None, Some(0x100));
        assert_eq!(result.err(), Some(efi::Status::INVALID_PARAMETER));

        let result = event_db.set_timer(event, TimerDelay::TimerRelative, None, Some(0x100));
        assert_eq!(result.err(), Some(efi::Status::INVALID_PARAMETER));

        let result =
            event_db.set_timer(0x1234 as *mut c_void, TimerDelay::TimerRelative, Some(0x100), None);
        assert_eq!(result.err(), Some(efi::Status::INVALID_PARAMETER));
    }

    #[test]
    fn timer_tick_should_signal_expired_timers() {
        let mut event_db = EventDb::new();
        let event = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();

        let event2 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();

        event_db.set_timer(event, TimerDelay::TimerRelative, Some(0x100), None).unwrap();
        event_db.set_timer(event2, TimerDelay::TimerRelative, Some(0x400), None).unwrap();
        assert_eq!(
            event_db
                .event_notification_iter(efi::TPL_APPLICATION)
                .collect::<Vec<EventNotification>>()
                .len(),
            0
        );

        //tick past the first timer
        event_db.timer_tick(0x200);

        let events =
            event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, event);

        //tick again, but not enough to trigger second timer.
        event_db.timer_tick(0x300);

        let events =
            event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>();
        assert_eq!(events.len(), 0);

        //tick past the second timer.
        event_db.timer_tick(0x400);

        let events =
            event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, event2);
    }

    #[test]
    fn periodic_timers_should_rearm_after_tick() {
        let mut event_db = EventDb::new();
        let event = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();

        let event2 = event_db
            .create_event(
                efi::EVT_TIMER | efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_NOTIFY,
                Some(test_notify_function),
                None,
                None,
            )
            .unwrap();

        event_db.set_timer(event, TimerDelay::TimerPeriodic, Some(0x100), Some(0x100)).unwrap();
        event_db.set_timer(event2, TimerDelay::TimerPeriodic, Some(0x500), Some(0x500)).unwrap();

        assert_eq!(
            event_db
                .event_notification_iter(efi::TPL_APPLICATION)
                .collect::<Vec<EventNotification>>()
                .len(),
            0
        );

        //tick past the first timer
        event_db.timer_tick(0x100);
        let events =
            event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, event);

        //tick just prior to re-armed first timer
        event_db.timer_tick(0x1FF);
        let events =
            event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>();
        assert_eq!(events.len(), 0);

        //tick past the re-armed first timer
        event_db.timer_tick(0x210);
        let events =
            event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, event);

        //tick past the second timer.
        event_db.timer_tick(0x500);
        let events =
            event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, event);
        assert_eq!(events[1].event, event2);

        //tick past the rearmed first timer
        event_db.timer_tick(0x600);
        let events =
            event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, event);

        //cancel the first timer
        event_db.set_timer(event, TimerDelay::TimerCancel, None, None).unwrap();

        //tick past where it would have been.
        event_db.timer_tick(0x700);
        let events =
            event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>();
        assert_eq!(events.len(), 0);

        //close the event for the second timer
        event_db.close_event(event2).unwrap();

        //tick past where it would have been.
        event_db.timer_tick(0x1000);
        let events =
            event_db.event_notification_iter(efi::TPL_APPLICATION).collect::<Vec<EventNotification>>();
        assert_eq!(events.len(), 0);
    }
}
