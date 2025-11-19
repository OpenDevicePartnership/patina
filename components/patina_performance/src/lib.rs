//! A component that enables performance analysis in the UEFI boot environment.
//!
//! The Performance component installs a protocol that can be used by other libraries or drivers to publish performance
//! reports.
//!
//! These reports are saved in the Firmware Basic Boot Performance Table (FBPT), so they can be extracted later in
//! the operating system.
//!
//! ## Integration Example
//!
//! Enabling performance in Patina is done by adding the `Performance` component to the Patina DXE Core build.
//!
//! ```rust,ignore
//! // ...
//!
//! Core::default()
//!  // ...
//!  .with_component(patina_performance::Performance)
//!  .start()
//!  .unwrap();
//!
//! // ...
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#![cfg_attr(not(test), no_std)]
#![allow(unexpected_cfgs)]
#![feature(coverage_attribute)]

pub mod component;
pub mod config;
mod mm;
