//! Test that &mut Storage with Config<T> is rejected at compile time.

use patina::{
    component::{Storage, component, params::Config},
    error::Result,
};

pub struct TestComponent;

#[component]
impl TestComponent {
    fn entry_point(self, _storage: &mut Storage, _config: Config<u32>) -> Result<()> {
        Ok(())
    }
}

fn main() {}
