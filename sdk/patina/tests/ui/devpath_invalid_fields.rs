//! `devpath!` rejects malformed structured fields.

use patina::devpath;

const _: [u8; 0] = devpath!("VenHw(not-a-guid)");
const _: [u8; 0] = devpath!("IPv4(999.0.0.1)");
const _: [u8; 0] = devpath!("MAC(0011,1)");
const _: [u8; 0] = devpath!("Path(1,1,0)");

fn main() {}
