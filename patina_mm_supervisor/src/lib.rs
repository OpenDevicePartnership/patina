//! MM Supervisor Core
//!
//! A Rust implementation of the MM Supervisor Core for standalone MM.
//!
//! Every core enters through [`MmSupervisorCore::entry_point`]. The BSP runs one-time
//! initialization and then serves MMI requests, while the APs wait in a holding pen and poll
//! their mailboxes for commands from the BSP. The supervisor does not use a heap. Its per-core
//! arrays are sized from the core count the MP Information HOB reports and allocated from MMRAM
//! pages during BSP initialization.
//!
//! ## Examples
//!
//! ```rust,no_run
//! # #[cfg(target_arch = "x86_64")]
//! # mod example {
//! use patina_mm_supervisor::*;
//!
//! struct MyPlatform;
//!
//! impl PlatformInfo for MyPlatform {}
//!
//! static SUPERVISOR: MmSupervisorCore<MyPlatform> = MmSupervisorCore::new();
//! # }
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![cfg(target_arch = "x86_64")]
#![cfg_attr(coverage, feature(coverage_attribute))]

mod comm_buffer;
mod error;
mod handlers;
mod hob;
mod intrinsics;
mod memory;
mod mmcore;
mod policy;
mod privilege_mgmt;
mod state;
#[cfg(test)]
mod test_support;
mod user_access_guard;
mod version;

pub use mmcore::MmSupervisorCore;

pub use handlers::SupervisorMmiHandler;

// The entry-point shim references `rust_main`, which is provided by the platform binary, and is
// only meaningful on the firmware (UEFI) target. Exclude it from host builds (tests, doctests)
// so their harnesses can link.
#[cfg(target_os = "uefi")]
core::arch::global_asm!(include_str!("entry_point.asm"));

/// Platform hooks for the MM Supervisor Core.
///
/// Implement it on a platform type and name that type in [`MmSupervisorCore`].
///
/// ## Examples
///
/// ```rust,no_run
/// # #[cfg(target_arch = "x86_64")]
/// # mod example {
/// use patina_mm_supervisor::*;
///
/// struct ExamplePlatform;
///
/// impl PlatformInfo for ExamplePlatform {}
/// # }
/// ```
pub trait PlatformInfo: 'static {
    /// Returns the platform-specific supervisor MMI handlers.
    ///
    /// The supervisor dispatches to the core's built-in handlers first and then to the handlers
    /// returned here, so a platform can add handlers (for example platform-specific or test
    /// handlers) without modifying the core. The default returns an empty slice.
    fn mmi_handlers() -> &'static [SupervisorMmiHandler] {
        &[]
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use crate::comm_buffer::MM_COMMON_REGION_HOB_GUID;

    use super::*;
    use patina::standard::efi;

    struct TestPlatform;

    impl PlatformInfo for TestPlatform {}

    fn test_mmi_handler(_: *mut u8, _: &mut usize) -> efi::Status {
        efi::Status::SUCCESS
    }

    static TEST_MMI_HANDLERS: &[SupervisorMmiHandler] = &[SupervisorMmiHandler {
        name: "TestHandler",
        handler_guid: MM_COMMON_REGION_HOB_GUID,
        handle: test_mmi_handler,
    }];

    struct PlatformWithHandler;

    impl PlatformInfo for PlatformWithHandler {
        fn mmi_handlers() -> &'static [SupervisorMmiHandler] {
            TEST_MMI_HANDLERS
        }
    }

    #[test]
    fn test_platform_info_default_and_custom_handlers() {
        assert!(TestPlatform::mmi_handlers().is_empty());

        let handlers = PlatformWithHandler::mmi_handlers();
        assert!(core::ptr::eq(handlers, TEST_MMI_HANDLERS));
        assert_eq!(handlers[0].name, "TestHandler");

        let mut buffer_size = 0;
        // SAFETY: a zero-length request with a null buffer, which the test handler ignores.
        assert_eq!(unsafe { (handlers[0].handle)(core::ptr::null_mut(), &mut buffer_size) }, efi::Status::SUCCESS);
    }
}
