//! Test that &Storage and &mut Storage parameters cannot be mixed.

use patina::{component::{component, Storage}, error::Result};

pub struct TestComponent;

#[component]
impl TestComponent {
    fn entry_point(self, _s1: &Storage, _s2: &mut Storage) -> Result<()> {
        Ok(())
    }
}

fn main() {}
