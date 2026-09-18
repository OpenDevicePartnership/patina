//! UEFI Event types and event group GUIDs.
//!
//! This module defines the UEFI event mechanism types used with the Boot Services event APIs
//! ([`EventType`], [`EventTimerType`], [`EventNotifyCallback`]) and the event group GUIDs that
//! identify UEFI-specification-defined event groups.
//!
//! Event groups defined by other specifications live in their own `events` module:
//! [`crate::pi::event`] for PI-defined groups and [`crate::management_mode::event`] for MM-defined
//! groups.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ops;

use crate::BinaryGuid;
use crate::standard::efi;

/// Function signature for event notify function.
pub type EventNotifyCallback<T> = unsafe extern "efiapi" fn(efi::Event, T);

/// The type of time that is specified in `TriggerTime`. See the timer delay types in “Related Definitions.”
#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum EventTimerType {
    /// The event’s timer setting is to be cancelled and no timer trigger is to be set.
    /// `TriggerTime` is ignored when canceling a timer.
    Cancel = efi::TIMER_CANCEL,

    /// The event is to be signaled periodically at `TriggerTime` intervals from the current time.
    /// This is the only timer trigger Type for which the event timer does not need to be reset for each notification.
    /// All other timer trigger types are “one shot.”
    Periodic = efi::TIMER_PERIODIC,

    /// The event is to be signaled in `TriggerTime` 100ns units.
    Relative = efi::TIMER_RELATIVE,
}

impl From<EventTimerType> for u32 {
    fn from(val: EventTimerType) -> Self {
        val as u32
    }
}

/// Type of event to create and its mode and attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct EventType(u32);

impl EventType {
    /// The event is a timer event and may be passed to
    /// [`BootServices::set_timer`](crate::uefi::boot_services::BootServices::set_timer).
    /// Note that timers only function during boot services time.
    pub const TIMER: EventType = EventType(efi::EVT_TIMER);

    /// The event is allocated from runtime memory.
    /// If an event is to be signaled after the call to
    /// [`BootServices::exit_boot_services`](crate::uefi::boot_services::BootServices::exit_boot_services)
    /// the event’s data structure and notification function need to be allocated from runtime memory.
    /// For more information, see
    /// <a href="https://uefi.org/specs/UEFI/2.10/08_Services_Runtime_Services.html#setvirtualaddressmap" target="_blank">
    ///   `SetVirtualAddressMap()`
    /// </a> .
    pub const RUNTIME: EventType = EventType(efi::EVT_RUNTIME);

    /// If an event of this type is not already in the signaled state,
    /// then the event’s `NotificationFunction` will be queued at the event’s `NotifyTpl` whenever the event is being waited
    /// on via [`BootServices::wait_for_event`](crate::uefi::boot_services::BootServices::wait_for_event) or
    /// [`BootServices::check_event`](crate::uefi::boot_services::BootServices::check_event).
    pub const NOTIFY_WAIT: EventType = EventType(efi::EVT_NOTIFY_WAIT);

    /// The event’s `NotifyFunction` is queued whenever the event is signaled.
    pub const NOTIFY_SIGNAL: EventType = EventType(efi::EVT_NOTIFY_SIGNAL);

    /// This event is of type [`Self::NOTIFY_SIGNAL`].
    /// It should not be combined with any other event types.
    /// This event type is functionally equivalent to the `EFI_EVENT_GROUP_EXIT_BOOT_SERVICES` event group.
    /// Refer to `EFI_EVENT_GROUP_EXIT_BOOT_SERVICES` event group description in
    /// [`BootServices::create_event_ex`](crate::uefi::boot_services::BootServices::create_event_ex) section below for
    /// additional details.
    pub const SIGNAL_EXIT_BOOT_SERVICES: EventType = EventType(efi::EVT_SIGNAL_EXIT_BOOT_SERVICES);

    /// The event is to be notified by the system when `SetVirtualAddressMap()` is performed.
    /// This event type is a composite of [`Self::NOTIFY_SIGNAL`], [`Self::RUNTIME`], and [`Self::RUNTIME`] and should not be combined with any other event types.
    pub const SIGNAL_VIRTUAL_ADDRESS_CHANGE: EventType = EventType(efi::EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE);
}

impl ops::BitOr for EventType {
    type Output = EventType;

    fn bitor(self, rhs: Self) -> Self::Output {
        EventType(self.0 | rhs.0)
    }
}

impl ops::BitOrAssign for EventType {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl From<EventType> for u32 {
    fn from(val: EventType) -> Self {
        val.0
    }
}

/// Exit Boot Services event group GUID.
///
/// The GUID for the event group signaled when `ExitBootServices()` is called.
///
/// In MM, this is forwarded as an MMI to allow MM drivers to perform cleanup.
///
/// Defined in UEFI as `gEfiEventExitBootServicesGuid`.
///
/// (`27ABF055-B1B8-4C26-8048-748F37BAA2DF`)
/// ```
/// # use patina::{Guid, uefi::event::EXIT_BOOT_SERVICES_EVENT_GROUP_GUID};
/// # assert_eq!(
/// #     "27ABF055-B1B8-4C26-8048-748F37BAA2DF",
/// #     format!("{:?}", Guid::from_ref(&EXIT_BOOT_SERVICES_EVENT_GROUP_GUID))
/// # );
/// ```
pub const EXIT_BOOT_SERVICES_EVENT_GROUP_GUID: BinaryGuid = BinaryGuid(efi::EVENT_GROUP_EXIT_BOOT_SERVICES);

/// Ready to Boot event group GUID.
///
/// The GUID for the event group signaled when the platform is ready to boot.
///
/// In MM, this is forwarded as an MMI to allow MM drivers to perform final setup.
///
/// Defined in UEFI as `gEfiEventReadyToBootGuid`.
///
/// (`7CE88FB3-4BD7-4679-87A8-A8D8DEE50D2B`)
/// ```
/// # use patina::{Guid, uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID};
/// # assert_eq!(
/// #     "7CE88FB3-4BD7-4679-87A8-A8D8DEE50D2B",
/// #     format!("{:?}", Guid::from_ref(&READY_TO_BOOT_EVENT_GROUP_GUID))
/// # );
/// ```
pub const READY_TO_BOOT_EVENT_GROUP_GUID: BinaryGuid = BinaryGuid(efi::EVENT_GROUP_READY_TO_BOOT);

/// Exit Boot Services Failed event group GUID.
///
/// The GUID for the event group signaled when `ExitBootServices()` fails. For example, the
/// implementation may find that the memory map key provided does not match the current memory map
/// key and return an error code. This event group will be signaled in that case just before
/// returning to the caller.
///
/// (`4F6C5507-232F-4787-B95E-72F862490CB1`)
/// ```
/// # use patina::uefi::event::EXIT_BOOT_SERVICES_FAILED_EVENT_GROUP_GUID;
/// # assert_eq!(
/// #     "4F6C5507-232F-4787-B95E-72F862490CB1",
/// #     format!("{}", EXIT_BOOT_SERVICES_FAILED_EVENT_GROUP_GUID)
/// # );
/// ```
pub const EXIT_BOOT_SERVICES_FAILED_EVENT_GROUP_GUID: BinaryGuid =
    BinaryGuid::from_string("4F6C5507-232F-4787-B95E-72F862490CB1");

/// Cache Attribute Change event group GUID.
///
/// The GUID for an event group signaled when the cache attributes for a memory region are changed.
/// The event group is intended for architectures, such as x86, that require cache attribute changes
/// to be propagated to all APs.
///
/// (`B8E477C7-26A9-4B9A-A7C9-5F8F1F3D9C7B`)
pub const CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP_GUID: BinaryGuid =
    BinaryGuid::from_string("B8E477C7-26A9-4B9A-A7C9-5F8F1F3D9C7B");
