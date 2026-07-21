//! `devpath!` rejects unknown node names.

use patina::devpath;

const _: [u8; 0] = devpath!("Unknown(1)");

fn main() {}
