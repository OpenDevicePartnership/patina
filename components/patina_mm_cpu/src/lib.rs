//! MM CPU Component
//!
//! This crate provides the [`MmCpuComponent`](component::MmCpuComponent), which
//! produces the PI `EFI_MM_CPU_PROTOCOL` (`gEfiMmCpuProtocolGuid`) inside the MM
//! User Core. That protocol lets MM drivers read architecture-standard registers
//! from any CPU's MM save state (for example, the trapping I/O instruction that
//! generated a software MMI).
//!
//! The MM save state lives in supervisor-only SMRAM, so `ReadSaveState` forwards
//! each read to the MM Supervisor, which enforces the save-state security policy
//! before returning the value. This component is the Rust replacement for the C
//! `MmSupervisorPkg/Drivers/MmSupervisedCpu` driver.
//!
//! ## Usage
//!
//! Register the component with the MM User Core via the platform's
//! `MmComponentInfo` implementation:
//!
//! ```rust,ignore
//! use patina_mm_user_core::component_dispatcher::{Add, Component, MmComponentInfo};
//!
//! struct MyMmPlatform;
//! impl MmComponentInfo for MyMmPlatform {
//!     fn components(mut add: Add<Component>) {
//!         add.component(patina_mm_cpu::component::MmCpuComponent::new());
//!     }
//! }
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![deny(missing_docs)]

extern crate alloc;

pub mod component;
pub mod protocol;

mod save_state;
