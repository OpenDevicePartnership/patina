//! UEFI driver model services for Patina components.
//!
//! The UEFI Driver Model is the set of protocols and boot services a driver uses to bind itself
//! to a controller, report its status, and describe itself to a user. This module groups the
//! pieces of that model that a Patina component can produce or consume:
//!
//! - [`driver::DriverServices`] - Connecting and disconnecting drivers to controllers.
//! - [`driver_binding::DriverBinding`] - Producing a driver binding protocol for a component.
//!
//! ## UEFI Driver Model Overview
//!
//! Drivers (and Patina components) within the UEFI Driver Model are not allowed to search for controllers to manage.
//! When a specific controller is needed, [`driver::DriverServices::connect_controller`] is used along with the EFI
//! Driver Binding Protocol once [`driver::DriverServices::connect_controller`] has identified the best drivers
//! for a controller, the start service in the EFI Driver Binding Protocol is used by
//! [`driver::DriverServices::connect_controller`] to start each driver on the controller. Once a controller is no
//! longer needed, it can be released with the EFI boot service [`driver::DriverServices::disconnect_controller`].
//!
//! [`driver::DriverServices::disconnect_controller`] calls the stop service in each EFI Driver Binding Protocol to
//! stop the controller.
//!
//! The driver initialization routine of an UEFI driver (and Patina component) is not allowed to touch any device
//! hardware. Instead, [`driver_binding::DriverBinding`] is created and passed to
//! [`super::protocol::ProtocolServicesExt::install_driver_binding`] so an EFI Driver Binding Protocol is installed
//! onto a generated agent [`super::handle::Handle`].
//!
//! The test to determine if a driver supports a given controller must be performed in as little time as possible
//! without causing any side effects on any of the controllers it is testing. As a result, most of the controller
//! initialization code is present in the start and stop functions of [`driver_binding::DriverBinding`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub mod driver;
pub mod driver_binding;
pub mod language;
