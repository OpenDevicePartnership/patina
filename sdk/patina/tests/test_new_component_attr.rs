//! Test for new component attribute on impl blocks

use patina::{
    component::{IntoComponent, component, params::Config},
    error::Result,
};

pub struct TestComponent {
    value: u32,
}

#[component]
impl TestComponent {
    fn entry_point(self, _config: Config<u32>) -> Result<()> {
        let _ = self.value; // Use the field to avoid dead code warning
        Ok(())
    }
}

#[test]
fn test_component_compiles() {
    // If this compiles, the macro is working
    let component = TestComponent { value: 42 };
    let _result = component.into_component();
}
