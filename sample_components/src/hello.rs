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
use r_efi::efi;
use uefi_component_interface::{DxeComponent, DxeComponentInterface};
use uefi_core::error::Result;

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

    fn guid(&self) -> efi::Guid {
        efi::Guid::from_bytes(&uuid::uuid!("582acbb5-7d72-4753-8efe-3d605fb3d9ae").to_bytes_le())
    }
}
