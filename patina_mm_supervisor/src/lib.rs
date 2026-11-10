//! MM Supervisor Core
//!
//! A Rust implementation of the MM Supervisor Core for standalone MM.
//!
//! Every core enters through [`MmSupervisorCore::entry_point`]. The BSP runs one-time
//! initialization and then serves MMI requests, while the APs wait in a holding pen and poll
//! their mailboxes for commands from the BSP. The supervisor does not use a heap: its structures
//! are fixed-size arrays sized by const generics.
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
//! // The const generic argument is the maximum CPU count used to size internal arrays.
//! static SUPERVISOR: MmSupervisorCore<MyPlatform, 8> = MmSupervisorCore::new();
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
mod tests {
    use super::*;

    struct TestPlatform;

    impl PlatformInfo for TestPlatform {}

    #[test]
    fn test_supervisor_creation() {
        let _supervisor: MmSupervisorCore<TestPlatform, 4> = MmSupervisorCore::new();
        // Just verify it compiles and creates without panic
    }

    #[test]
    fn test_supervisor_is_const() {
        // Verify we can create a static instance (no heap allocation)
        static _SUPERVISOR: MmSupervisorCore<TestPlatform, 4> = MmSupervisorCore::new();
    }
}
