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
//! use patina_mm_supervisor::{CpuInfo, PlatformInfo, SupervisorMmiHandler};
//! use r_efi::efi;
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
//! impl CpuInfo for MyPlatform {
//!     fn ap_poll_timeout_us() -> u64 { 1000 }
//! }
//!
//! impl PlatformInfo for MyPlatform {
//!     type CpuInfo = Self;
//!
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

use r_efi::efi;

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
/// [`EfiMmCommunicateHeader::header_guid`](patina::pi::protocols::communication::EfiMmCommunicateHeader::header_guid)
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

// SAFETY: SupervisorMmiHandler contains only a &'static str, a Guid (plain data), and a fn pointer.
// All of these are inherently Sync.
unsafe impl Sync for SupervisorMmiHandler {}
