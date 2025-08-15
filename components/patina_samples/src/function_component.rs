//! A Hello world component implementation example using a function component.
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
use patina_sdk::{component::params::Config, error::Result};

/// An example of a config parameter that is consumed by a component.
#[derive(Default, Clone, Copy)]
pub struct Name(pub &'static str);

/// A simple function component example that uses consumes a configuration value.
pub fn log_hello(name: Config<Name>) -> Result<()> {
    info!("Hello, {}!", name.0);
    Ok(())
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use patina_sdk::component::IntoComponent;

    #[test]
    fn test_func_implements_into_component() {
        let _ = log_hello.into_component();
    }
}
