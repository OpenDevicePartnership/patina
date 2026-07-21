//! `devpath!` rejects invalid symbolic values.

use patina::devpath;

const _: [u8; 0] = devpath!("Ata(Tertiary,Master,0)");

fn main() {}
