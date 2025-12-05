//! Test that Config<T> and ConfigMut<T> for the same type are rejected at compile time.

use patina::{
    component::{component, params::{Config, ConfigMut}},
    error::Result,
};

pub struct TestComponent;

#[component]
impl TestComponent {
    fn entry_point(self, _config1: Config<u32>, _config2: ConfigMut<u32>) -> Result<()> {
        Ok(())
    }
}

fn main() {}
