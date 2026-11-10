//! Supervisor MMI handler registry.
//!
//! Defines the built-in supervisor MMI handlers and the [`SupervisorMmiHandler`] type that
//! platforms use to register their own.
//!
//! ## Architecture
//!
//! The built-in handlers are in [`DEFAULT_SUPERVISOR_MMI_HANDLERS`]. For a supervisor request,
//! the core searches these handlers and then the platform handlers (see
//! [`PlatformInfo::mmi_handlers`](crate::PlatformInfo::mmi_handlers)) for one matching the
//! communicate header GUID. Only the first match runs, so a built-in handler takes precedence
//! over a platform handler with the same GUID.
//!
//! ## Adding Platform-Specific Handlers
//!
//! Implement [`PlatformInfo::mmi_handlers`](crate::PlatformInfo::mmi_handlers) in the platform
//! crate and return a static slice:
//!
//! ```rust,no_run
//! # #[cfg(target_arch = "x86_64")]
//! # mod example {
//! use patina_mm_supervisor::{PlatformInfo, SupervisorMmiHandler};
//! use patina::standard::efi;
//!
//! struct MyPlatform;
//!
//! fn my_handler(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
//!     // Handle the request...
//!     efi::Status::SUCCESS
//! }
//!
//! static MY_HANDLERS: &[SupervisorMmiHandler] = &[SupervisorMmiHandler {
//!     name: "MyPlatformHandler",
//!     handler_guid: patina::BinaryGuid::from_string("12345678-abcd-ef01-2345-6789abcdef01").into_inner(),
//!     handle: my_handler,
//! }];
//!
//! impl PlatformInfo for MyPlatform {
//!     fn mmi_handlers() -> &'static [SupervisorMmiHandler] {
//!         MY_HANDLERS
//!     }
//! }
//! # }
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub(crate) mod supv_request;
mod system;

pub(crate) use supv_request::unblock_memory::UnblockedMemoryTracker;

pub(crate) use supv_request::mm_supv_request_handler;
pub(crate) use system::{mm_exit_boot_services_handler, mm_ready_to_lock_handler};

use patina::standard::efi;
use patina::{
    management_mode::{
        guid::MM_DXE_READY_TO_LOCK_PROTOCOL_GUID, protocol::mm_supervisor_request::MM_SUPERVISOR_REQUEST_HANDLER_GUID,
    },
    uefi::event::EXIT_BOOT_SERVICES_EVENT_GROUP_GUID,
};

/// A supervisor MMI handler registered at build time.
///
/// The core considers each entry when dispatching supervisor-channel requests, matching the
/// [`EfiMmCommunicateHeader::header_guid`](patina::pi::protocol::communication::EfiMmCommunicateHeader::header_guid)
/// against [`handler_guid`](SupervisorMmiHandler::handler_guid).
///
/// The [`handle`](SupervisorMmiHandler::handle) function receives `comm_buffer`, a pointer to
/// the data after the communicate header, and `comm_buffer_size`, which holds the message length
/// on input and the response length on output. It returns an [`efi::Status`].
#[derive(Debug)]
pub struct SupervisorMmiHandler {
    /// Human-readable name for logging and debugging.
    pub name: &'static str,
    /// GUID identifying the request type this handler processes.
    pub handler_guid: efi::Guid,
    /// The handler function.
    pub handle: fn(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status,
}

/// The core's built-in supervisor MMI handlers.
///
/// - `MmReadyToLock`: triggered from the non-MM environment on the `DxeMmReadyToLock` event.
///   Afterwards some features, such as unblock memory, are no longer available.
/// - `MmSupvRequest`: handles supervisor requests such as unblock memory, fetch policy, version
///   info, and communication buffer updates.
/// - `MmExitBootServices`: triggered from the non-MM environment on `ExitBootServices`.
///   Afterwards the supervisor communication channel is closed and supervisor requests are
///   rejected.
///
/// An immutable, compile-time dispatch table with no runtime state.
pub(crate) static DEFAULT_SUPERVISOR_MMI_HANDLERS: &[SupervisorMmiHandler] = &[
    SupervisorMmiHandler {
        name: "MmReadyToLock",
        handler_guid: MM_DXE_READY_TO_LOCK_PROTOCOL_GUID.into_inner(),
        handle: mm_ready_to_lock_handler,
    },
    SupervisorMmiHandler {
        name: "MmSupvRequest",
        handler_guid: MM_SUPERVISOR_REQUEST_HANDLER_GUID.into_inner(),
        handle: mm_supv_request_handler,
    },
    SupervisorMmiHandler {
        name: "MmExitBootServices",
        handler_guid: EXIT_BOOT_SERVICES_EVENT_GROUP_GUID.into_inner(),
        handle: mm_exit_boot_services_handler,
    },
];

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    fn assert_sync<T: Sync>() {}

    #[test]
    fn supervisor_mmi_handler_is_sync_without_an_unsafe_impl() {
        assert_sync::<SupervisorMmiHandler>();
    }

    #[test]
    fn default_handlers_have_the_expected_order_and_metadata() {
        let expected = [
            (
                "MmReadyToLock",
                MM_DXE_READY_TO_LOCK_PROTOCOL_GUID.into_inner(),
                mm_ready_to_lock_handler as fn(*mut u8, &mut usize) -> efi::Status,
            ),
            (
                "MmSupvRequest",
                MM_SUPERVISOR_REQUEST_HANDLER_GUID.into_inner(),
                mm_supv_request_handler as fn(*mut u8, &mut usize) -> efi::Status,
            ),
            (
                "MmExitBootServices",
                EXIT_BOOT_SERVICES_EVENT_GROUP_GUID.into_inner(),
                mm_exit_boot_services_handler as fn(*mut u8, &mut usize) -> efi::Status,
            ),
        ];

        assert_eq!(DEFAULT_SUPERVISOR_MMI_HANDLERS.len(), expected.len());
        for (handler, (name, guid, handle)) in DEFAULT_SUPERVISOR_MMI_HANDLERS.iter().zip(expected) {
            assert_eq!(handler.name, name);
            assert_eq!(handler.handler_guid, guid);
            assert!(core::ptr::fn_addr_eq(handler.handle, handle));
        }
    }
}
