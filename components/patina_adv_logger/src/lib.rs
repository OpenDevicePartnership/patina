//! UEFI Advanced Logger Support
//!
//! This library provides a logger for logging to a hardware port and the
//! advanced logger memory buffer, as well as a component for publishing the
//! advanced logger protocol.
//!
//! ## Examples and Usage
//!
//! This create includes two primary traits intended for consumer use; the logger
//! implementation to use with the log create and the AdvLogger DXE component. These
//! two entities should both be used by the DXE core for a complete advanced logger
//! solution.
//!
//! To initialize the advanced logger structs, the platform DxeCore crate should
//! specify the static logger as required by the Log crate and a static component.
//! The logger definition should be customized with the format, filters, log level,
//! and the SerialIO for the hardware port.
//!
//! In the platform start routine, then set the logger. This should be as early
//! as possible. After the logger has been set, the platform should initialize the
//! advanced logger using the ini_advanced_logger routine, passing it the physical
//! hob list. This routine will initialize the memory log if discovered in the physical
//! hob list.
//!
//! ```
//! # use core::ffi::c_void;
//! use patina_adv_logger::{component::AdvancedLoggerComponent, logger::AdvancedLogger};
//!
//! static LOGGER: AdvancedLogger<patina_sdk::serial::uart::UartNull> = AdvancedLogger::new(
//!      patina_sdk::log::Format::Standard,
//!      &[("goblin", log::LevelFilter::Off), ("patina_internal_depex", log::LevelFilter::Off)],
//!      log::LevelFilter::Trace,
//!      patina_sdk::serial::uart::UartNull{},
//! );
//!
//! static ADV_LOGGER: AdvancedLoggerComponent<patina_sdk::serial::uart::UartNull> = AdvancedLoggerComponent::new(&LOGGER);
//!
//! fn _start(physical_hob_list: *const c_void) {
//!     log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Trace)).unwrap();
//!     let _ = ADV_LOGGER.init_advanced_logger(physical_hob_list);
//! }
//! ```
//!
//! For the protocol to be created for use of by external components, the platform
//! should invoke dxecore.start with the advanced logger component.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod component;
pub mod logger;
pub mod protocol;

#[cfg(feature = "std")]
pub mod parser;

mod integration_test;
mod memory_log;
