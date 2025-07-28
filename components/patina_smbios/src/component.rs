//! SMBIOS Service Implementation
//!
//! Defines the SMBIOS provider for use as a service
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use patina_sdk::{component::IntoComponent, error::Result};

/// Initializes the SMBIOS provider service
#[derive(IntoComponent)]
pub struct SmbiosProviderManager;

impl SmbiosProviderManager {
    fn entry_point(self) -> Result<()> {
        log::info!("Hello from SmbiosProviderManager");
        Ok(())
    }
}
