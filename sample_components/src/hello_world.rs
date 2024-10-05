//! Hello World Sample Component Implementation
//!
//! A simple component implementation used to demonstrate how to build a component.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use dxe_component_interface::{DxeComponent, DxeComponentInterface};
use log::info;
use uefi_core::error::Result;

pub struct HelloWorldComponent;

impl DxeComponent for HelloWorldComponent {
    fn entry_point(&self, _interface: &dyn DxeComponentInterface) -> Result<()> {
        // Main component functionality
        info!("Hello, World!");

        // Return value
        Ok(())
    }
}
