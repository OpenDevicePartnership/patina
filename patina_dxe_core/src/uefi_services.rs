//! DXE Core implementation of the Patina UEFI Services.
//!
//! Each submodule implements one of the [`patina::component::service::uefi_services`] traits
//! directly against the core's internal Rust APIs (the `core_*` functions and the protocol/event
//! databases) instead of calling out to the C `EFI_BOOT_SERVICES` function-pointer table. The
//! implementations are registered as services by the core during initialization so that components
//! can consume them by declaring a `Service<dyn Trait>` dependency.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

mod config_table;
mod driver;
mod event;
mod image;
mod protocol;
mod timer_event;
mod timing;
mod tpl;

pub(crate) use config_table::CoreConfigurationTableServices;
pub(crate) use driver::CoreDriverServices;
pub(crate) use event::CoreEventServices;
pub(crate) use image::CoreImageServices;
pub(crate) use protocol::CoreProtocolServices;
pub(crate) use timer_event::CoreTimerEventServices;
pub(crate) use timing::CoreTimingServices;
pub(crate) use tpl::CoreTplServices;
