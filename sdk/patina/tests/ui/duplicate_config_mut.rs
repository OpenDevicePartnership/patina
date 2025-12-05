//! Test that duplicate ConfigMut<T> parameters are rejected at compile time.

use patina::{
    component::{component, params::ConfigMut},
    error::Result,
};

pub struct TestComponent;

#[component]
impl TestComponent {
    fn entry_point(self, _config1: ConfigMut<u32>, _config2: ConfigMut<u32>) -> Result<()> {
        Ok(())
    }
}

fn main() {}
