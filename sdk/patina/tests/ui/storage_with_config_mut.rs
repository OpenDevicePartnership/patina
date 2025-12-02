//! Test that &Storage with ConfigMut<T> is rejected at compile time.

use patina::{
    component::{component, params::ConfigMut, Storage},
    error::Result,
};

pub struct TestComponent;

#[component]
impl TestComponent {
    fn entry_point(self, _storage: &Storage, _config: ConfigMut<u32>) -> Result<()> {
        Ok(())
    }
}

fn main() {}
