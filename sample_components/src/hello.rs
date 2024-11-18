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
use log::info;
use uefi_component_interface::{DxeComponent, DxeComponentInterface};
use uefi_sdk::error::Result;

#[derive(Default)]
pub struct HelloComponent {
    name: &'static str,
}

impl HelloComponent {
    pub fn with_name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }
}

impl DxeComponent for HelloComponent {
    fn entry_point(&self, _interface: &dyn DxeComponentInterface) -> Result<()> {
        // Main component functionality
        info!("Hello, {}!", self.name);

        // Return value
        Ok(())
    }
}
