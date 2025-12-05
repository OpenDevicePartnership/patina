//! Test that duplicate &mut Storage parameters are rejected at compile time.

use patina::{component::{component, Storage}, error::Result};

pub struct TestComponent;

#[component]
impl TestComponent {
    fn entry_point(self, _s1: &mut Storage, _s2: &mut Storage) -> Result<()> {
        Ok(())
    }
}

fn main() {}
