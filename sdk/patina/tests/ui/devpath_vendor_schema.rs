//! `devpath!` rejects vendor schemas that redefine built-in nodes.

use patina::devpath;

const _: [u8; 0] = devpath!(
    vendor-defined {
        Pci {
            type: hardware,
            guid: "00112233-4455-6677-8899-aabbccddeeff",
            fields: [],
        },
    };
    "Pci()"
);

fn main() {}
