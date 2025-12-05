//! Test that duplicate StandardBootServices parameters are rejected at compile time.

use patina::{
    boot_services::StandardBootServices,
    component::component,
    error::Result,
};

pub struct TestComponent;

#[component]
impl TestComponent {
    fn entry_point(self, _bs1: StandardBootServices, _bs2: StandardBootServices) -> Result<()> {
        Ok(())
    }
}

fn main() {}
