//! `devpath!` rejects integers outside their wire field.

use patina::devpath;

const _: [u8; 0] = devpath!("Ctrl(0x100000000)");

fn main() {}
