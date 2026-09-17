//! Supervisor MMI Handler Registry
//!
//! This module provides the built-in supervisor MMI handlers for the MM Supervisor Core, along
//! with the [`SupervisorMmiHandler`] type that platforms use to register their own handlers.
//!
//! ## Architecture
//!
//! The core's built-in handlers are collected in the [`DEFAULT_SUPERVISOR_MMI_HANDLERS`] slice.
//! During supervisor request processing, the core iterates these handlers followed by any
//! platform-provided handlers (see [`PlatformInfo::mmi_handlers`](crate::PlatformInfo::mmi_handlers))
//! to find a handler matching the communicate header GUID.
//!
//! ## Adding Platform-Specific Handlers
//!
//! To register handlers from a platform crate, implement
//! [`PlatformInfo::mmi_handlers`](crate::PlatformInfo::mmi_handlers) and return a static slice:
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

mod supv_request;
mod system_handlers;

pub use supv_request::unblock_memory::UnblockedMemoryTracker;

pub(crate) use supv_request::mm_supv_request_handler;
pub(crate) use system_handlers::{mm_exit_boot_services_handler, mm_ready_to_lock_handler};

use patina::standard::efi;

// GUID for gEfiDxeMmReadyToLockProtocolGuid
// { 0x60ff8964, 0xe906, 0x41d0, { 0xaf, 0xed, 0xf2, 0x41, 0xe9, 0x74, 0xe0, 0x8e } }
/// GUID for the DXE MM Ready To Lock protocol.
pub const EFI_DXE_MM_READY_TO_LOCK_PROTOCOL_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("60ff8964-e906-41d0-afed-f241e974e08e");

/// Supervisor version. Encodes major.minor as (major << 16) | minor.
pub const VERSION: u32 = 0x00130008;

/// Supervisor patch level.
pub const PATCH_LEVEL: u32 = 0x00010001;

/// A build-time registered supervisor MMI handler.
///
/// Each entry represents a handler that the supervisor core will consider when dispatching
/// supervisor-channel requests. Handlers are matched by comparing the
/// [`EfiMmCommunicateHeader::header_guid`](patina::pi::protocol::communication::EfiMmCommunicateHeader::header_guid)
/// against [`handler_guid`](SupervisorMmiHandler::handler_guid).
///
/// ## Handler Function Signature
///
/// The [`handle`](SupervisorMmiHandler::handle) function receives:
/// - `comm_buffer`: Pointer to the data portion of the communicate buffer (after the header).
/// - `comm_buffer_size`: On input, the message length. On output, the response data length.
///
/// The handler should return an [`efi::Status`] code.
#[derive(Debug)]
pub struct SupervisorMmiHandler {
    /// Human-readable name for logging and debugging.
    pub name: &'static str,
    /// GUID identifying the request type this handler processes.
    pub handler_guid: efi::Guid,
    /// The handler function.
    pub handle: fn(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status,
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::{
        state::DEFAULT_SUPERVISOR_MMI_HANDLERS,
        supervisor_handlers::{mm_exit_boot_services_handler, mm_ready_to_lock_handler, mm_supv_request_handler},
    };
    use patina::{
        guid::EVENT_EXIT_BOOT_SERVICES,
        management_mode::protocol::mm_supervisor_request::MM_SUPERVISOR_REQUEST_HANDLER_GUID,
    };

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
                EFI_DXE_MM_READY_TO_LOCK_PROTOCOL_GUID.into_inner(),
                mm_ready_to_lock_handler as fn(*mut u8, &mut usize) -> efi::Status,
            ),
            (
                "MmSupvRequest",
                MM_SUPERVISOR_REQUEST_HANDLER_GUID.into_inner(),
                mm_supv_request_handler as fn(*mut u8, &mut usize) -> efi::Status,
            ),
            (
                "MmExitBootServices",
                EVENT_EXIT_BOOT_SERVICES.into_inner(),
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

    #[test]
    fn version_constants_encode_the_supported_release() {
        assert_eq!(VERSION >> 16, 0x13);
        assert_eq!(VERSION & 0xFFFF, 0x08);
        assert_eq!(PATCH_LEVEL >> 16, 0x01);
        assert_eq!(PATCH_LEVEL & 0xFFFF, 0x01);
    }
}
