//! Test that &mut Storage and &Storage parameters cannot be mixed (reverse order).

use patina::{component::{component, Storage}, error::Result};

pub struct TestComponent;

#[component]
impl TestComponent {
    fn entry_point(self, _s1: &mut Storage, _s2: &Storage) -> Result<()> {
        Ok(())
    }
}

fn main() {}
