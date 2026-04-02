//! Default connection strategy: connect all controllers.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{boot_services::StandardBootServices, error::Result};

use crate::{connect_controller::ConnectController, helpers};

/// Default connection strategy that connects all controllers recursively.
///
/// Delegates to [`helpers::connect_all()`](crate::helpers::connect_all),
/// preserving the existing behavior. This is the strategy used by
/// [`SimpleBootManager::new()`](crate::SimpleBootManager::new) when no
/// custom strategy is provided.
pub struct ConnectAllStrategy;

impl ConnectController for ConnectAllStrategy {
    #[coverage(off)]
    fn connect(&self, boot_services: &StandardBootServices) -> Result<()> {
        helpers::connect_all(boot_services)
    }
}
