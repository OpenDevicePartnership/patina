//! A Hello world component implementation example using a struct component.
//!
//! A simple component implementation used to demonstrate how to build a component.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use patina_sdk::{
    component::{params::Config, IntoComponent},
    error::Result,
};

#[derive(IntoComponent)]
pub struct HelloStruct(pub &'static str);

impl HelloStruct {
    fn entry_point(self, age: Config<i32>) -> Result<()> {
        log::info!("Hello, {}! You are age {}!", self.0, *age);
        Ok(())
    }
}

#[derive(IntoComponent)]
#[entry_point(path = my_function)]
pub enum GreetingsEnum {
    Hello(&'static str),
    Goodbye(&'static str),
}

fn my_function(s: GreetingsEnum) -> Result<()> {
    match s {
        GreetingsEnum::Hello(name) => log::info!("Hello, {}!", name),
        GreetingsEnum::Goodbye(name) => log::info!("Goodbye, {}!", name),
    }
    Ok(())
}
