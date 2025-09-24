//! Management Mode (MM) Services
//!
//! The services available to interact with MM in Patina firmware.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
pub mod platform_mm_control;

pub use crate::component::communicator::MmCommunication;
pub use crate::component::sw_mmi_manager::SwMmiTrigger;
pub use platform_mm_control::PlatformMmControl;
