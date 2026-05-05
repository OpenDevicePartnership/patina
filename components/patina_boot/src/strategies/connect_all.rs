//! Connect-all strategy: connect all controllers.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{boot_services::BootServices, error::Result};

use crate::{connect_controller::ConnectController, helpers};

/// Connects all controllers recursively via
/// [`helpers::connect_all()`](crate::helpers::connect_all).
pub struct ConnectAllStrategy;

impl<B: BootServices> ConnectController<B> for ConnectAllStrategy {
    fn connect(&self, boot_services: &B) -> Result<()> {
        helpers::connect_all(boot_services)
    }
}
