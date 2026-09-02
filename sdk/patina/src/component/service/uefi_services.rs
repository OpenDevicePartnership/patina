//! UEFI Services for Patina components.
//!
//! This module defines a set of service traits that expose UEFI Boot Services (and,
//! over time, Runtime Services) to Patina components as Rust APIs.
//!
//! These services are the recommended way for components to consume UEFI functionality.
//! Unlike the raw [`crate::uefi::boot_services`] abstractions, which wrap the C
//! `EFI_BOOT_SERVICES` function-pointer table, these services are implemented by the
//! Patina DXE Core directly against its internal Rust APIs. Components therefore do not
//! interact with C-style constructs such as the boot services table or raw pointers until
//! they go down an unavoidable path into C code, such as calling a protocol function
//! pointer when the protocol is produced by a C driver.
//!
//! # Design
//!
//! Each service group is defined as a trait in this module and implemented by the core.
//! Components declare a dependency on a service by adding a [`Service<dyn Trait>`] parameter
//! to their entry point. The trait is only made available once the core registers its
//! implementation, so a component depending on a service is guaranteed the service is ready.
//! Some services may be deferred until underlying dependencies such as an architectural protocol
//! or other platform-dependent service is available.
//!
//! Services are split into cohesive functional groups so that a component can declare a more
//! granular dependency on the functionality it needs:
//!
//! - [`config_table::ConfigurationTableServices`] - Configuration table installation and lookup.
//! - [`driver::DriverServices`] - Connecting and disconnecting drivers to controllers.
//! - [`driver_binding::DriverBinding`] - Producing a driver binding protocol for a component.
//! - [`event::EventServices`] - Events, using Rust closures for notifications.
//! - [`image::ImageServices`] - Loading, starting, and unloading UEFI images.
//! - [`protocol::ProtocolServices`] - Typed protocol installation and discovery.
//! - [`timer_event::TimerEventServices`] - Timer events, available once the Timer Architectural
//!   Protocol is installed.
//! - [`timing::TimingServices`] - Delays and the Watchdog timer.
//! - [`tpl::TplServices`] - Raising and restoring the Task Priority Level (TPL).
//!
//! [`Service<dyn Trait>`]: crate::component::service::Service
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub mod config_table;
pub mod driver;
pub mod driver_binding;
pub mod event;
pub mod handle;
pub mod image;
pub mod protocol;
pub mod timer_event;
pub mod timing;
pub mod tpl;
