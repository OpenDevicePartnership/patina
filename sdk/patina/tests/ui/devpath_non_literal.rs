//! `devpath!` only accepts a string literal.

use patina::devpath;

const DEVICE_PATH: &str = "Pci(1,0)";
const _: [u8; 0] = devpath!(DEVICE_PATH);

fn main() {}
