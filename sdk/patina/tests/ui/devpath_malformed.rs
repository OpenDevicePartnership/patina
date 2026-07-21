//! `devpath!` rejects malformed grammar.

use patina::devpath;

const _: [u8; 0] = devpath!("Pci(1,0");

fn main() {}
