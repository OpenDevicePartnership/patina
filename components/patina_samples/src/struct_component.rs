//! A Hello world component implementation example using a struct component.
//!
//! A simple component implementation used to demonstrate how to build a component.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina::{
    component::{IntoComponent, params::Config},
    error::Result,
};

/// A simple struct component example that uses the default entry point function.
#[derive(IntoComponent)]
pub struct HelloStruct(pub &'static str);

impl HelloStruct {
    fn entry_point(self, age: Config<i32>) -> Result<()> {
        log::info!("Hello, {}! You are age {}!", self.0, *age);
        Ok(())
    }
}

/// A simple enum component implementation example that uses a custom entry point function.
#[derive(IntoComponent)]
#[entry_point(path = my_function)]
pub enum GreetingsEnum {
    /// Represents a greeting message.
    Hello(&'static str),
    /// Represents a farewell message.
    Goodbye(&'static str),
}

// This example shows that the entry point function can be defined outside of the enum.
fn my_function(s: GreetingsEnum) -> Result<()> {
    match s {
        GreetingsEnum::Hello(name) => log::info!("Hello, {name}!"),
        GreetingsEnum::Goodbye(name) => log::info!("Goodbye, {name}!"),
    }
    Ok(())
}
