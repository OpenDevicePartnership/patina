//! Test that multiple &Storage parameters are allowed (compile-success test).
//! Note: This should compile successfully.

use patina::{component::{component, Storage}, error::Result};

pub struct TestComponent;

#[component]
impl TestComponent {
    fn entry_point(self, _s1: &Storage, _s2: &Storage, _s3: &Storage) -> Result<()> {
        Ok(())
    }
}

fn main() {}
