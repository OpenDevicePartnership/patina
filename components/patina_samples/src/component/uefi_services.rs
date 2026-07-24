//! UEFI Services Sample Components
//!
//! This module collects sample components that demonstrate the Patina UEFI Services from
//! [`patina::component::service::uefi_services`]. Each sample is a small, self-contained
//! component:
//!
//! - [`overview`] - A quick tour of timer, event, and protocol usage in one component.
//! - [`configuration_table`] - Installs a vendor configuration table and reads it back.
//! - [`driver_connect`] - Discovers controllers by protocol and connects drivers to them.
//! - [`end_of_dxe_protocol_consumer`] - Defers protocol consumption to End-of-DXE.
//! - [`protocol_consumer`] - Shows different ways to consume a protocol.
//! - [`protocol_publisher`] - Demonstrates one component publishing a protocol and another consuming it.
//! - [`timers`] - Drives work from one-shot and periodic timers using Rust closures.
//! - [`tpl_critical_section`] - Serializes access to shared state with TPL.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub mod configuration_table;
pub mod driver_connect;
pub mod end_of_dxe_protocol_consumer;
pub mod overview;
pub mod protocol_consumer;
pub mod protocol_publisher;
pub mod timers;
pub mod tpl_critical_section;

pub use overview::UefiServicesSample;
