//! UEFI Driver Model services for Patina components.
//!
//! The UEFI Driver Model is the set of protocols and boot services a driver uses to bind itself to a controller,
//! report its status, and describe itself to a user. This module groups the pieces of that model that a Patina
//! component can produce or consume:
//!
//! - [`component_name::UefiDriverModelComponentName`] - Publishing a driver and controller name.
//! - [`controller_driver::ControllerDriver`] - A simple way to write a UEFI Driver Model driver that manages
//!   controllers.
//! - [`driver::DriverServices`] - Connecting and disconnecting drivers to controllers.
//! - [`driver_binding::DriverBinding`] - Producing a driver binding protocol for a component.
//! - [`driver_supported_efi_version`] - Publishing the UEFI Specification revision a driver conforms to.
//! - [`language::LanguageTable`] - Language matching for driver and controller name tables.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub mod component_name;
pub mod controller_driver;
pub mod driver;
pub mod driver_binding;
pub mod driver_supported_efi_version;
pub mod language;
