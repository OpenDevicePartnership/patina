//! MM Supervisor Core
//!
//! A pure Rust implementation of the MM Supervisor Core for standalone MM mode environments.
//!
//! This crate provides the core functionality for running a supervisor in MM (Management Mode)
//! that orchestrates incoming requests on the BSP while APs wait in a holding pen.
//!
//! ## Architecture
//!
//! The entry point is executed on all cores:
//! - **BSP**: Performs one-time initialization and enters the request serving loop
//! - **APs**: Enter a holding pen and poll mailboxes for commands from BSP
//!
//! ## Memory Model
//!
//! This is a core component that manages its own memory. It does **not** use heap allocation.
//! All structures use fixed-size arrays with compile-time constants provided via const generics.
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

/// Re-export the MM Supervisor Core for external use. The actual implementation
/// resides in the `mmcore` module.
pub use mmcore::MmSupervisorCore;

/// Re-export the MMI handler descriptor for external use. Platforms name it in the
/// signatures of the handlers they register, and the actual definition resides in the
/// `handlers` module.
pub use handlers::SupervisorMmiHandler;

// The entry-point shim references `rust_main`, which is provided by the platform binary, and is
// only meaningful on the firmware (UEFI) target. Exclude it from host builds (tests, doctests)
// so their harnesses can link.
#[cfg(target_os = "uefi")]
core::arch::global_asm!(include_str!("entry_point.asm"));

/// A trait to be implemented by the platform to provide configuration values and types to be used
/// by the MM Supervisor Core.
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
    /// The supervisor dispatch loop iterates the core's built-in handlers first and then
    /// the handlers returned here, so platforms can register additional handlers (for
    /// example platform-specific or test handlers) without modifying the core.
    ///
    /// The default implementation returns an empty slice.
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
