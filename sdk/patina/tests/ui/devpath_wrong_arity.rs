//! `devpath!` rejects missing required arguments.

use patina::devpath;

const _: [u8; 0] = devpath!("Pci(1)");

fn main() {}
