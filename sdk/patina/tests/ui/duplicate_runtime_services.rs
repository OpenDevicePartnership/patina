//! Test that duplicate StandardRuntimeServices parameters are rejected at compile time.

use patina::{
    component::component,
    error::Result,
    runtime_services::StandardRuntimeServices,
};

pub struct TestComponent;

#[component]
impl TestComponent {
    fn entry_point(self, _rt1: StandardRuntimeServices, _rt2: StandardRuntimeServices) -> Result<()> {
        Ok(())
    }
}

fn main() {}
