# Patina Macro Crate

`patina_macro` hosts the procedural macros used in Patina. This includes those that support the Patina component
system, service registration, guided HOB parsing, on-target test discovery, and more. The
[`patina`](https://crates.io/crates/patina) crate re-export these macros, so most cases only need a dependency on
`patina`.

## Notable Macros

### `#[component]`

- Applied to impl blocks containing an `entry_point` method to define components.
- Validates parameters at compile time and generates the boilerplate required to satisfy `patina::component::IntoComponent`.
- The `entry_point` method must consume `self` and takes dependency-injected parameters implementing `ComponentParam`.
- Compile-time validation detects parameter conflicts such as duplicate `ConfigMut<T>` or mixing `Config<T>` and `ConfigMut<T>`.

```rust
use patina::component::{component, params::Config};

struct BoardInit;

#[component]
impl BoardInit {
    fn entry_point(self, config: Config<u32>) -> patina::error::Result<()> {
        patina::log::info!("Selected profile: {}", *config);
        Ok(())
    }
}
```

### `#[derive(IntoService)]`

- Implements `patina::component::service::IntoService` for a concrete provider.
- Specify one or more service interfaces with `#[service(dyn TraitA, dyn TraitB)]`.

> Note: The macro leaks the provider once and registers `'static` references so every component receives the same
> backing instance.

```rust
use patina::component::service::IntoService;

trait Uart {
    fn write(&self, bytes: &[u8]) -> patina::error::Result<()>;
}

#[derive(IntoService)]
#[service(dyn Uart)]
struct SerialPort;

impl Uart for SerialPort {
    fn write(&self, bytes: &[u8]) -> patina::error::Result<()> {
        patina::log::info!("UART: {:?}", bytes);
        Ok(())
    }
}
```

### `#[derive(FromHob)]`

- Bridges GUIDed Hand-Off Blocks (HOBs) into strongly typed Rust values.
- Attach the GUID with `#[hob = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"]`.

```rust
use patina::component::hob::FromHob;

#[derive(FromHob, zerocopy_derive::FromBytes)]
#[repr(C)]
#[hob = "8be4df61-93ca-11d2-aa0d-00e098032b8c"]
struct FirmwareVolumeHeader {
    length: u32,
    revision: u16,
}
```

### `#[patina_test]`

- Registers a function with the Patina test runner that executes inside the DXE environment.
- Gate platform-specific tests with `cfg_attr` so they only compile when the runner is active.
- Optional attributes:
  - `#[should_fail]` or `#[should_fail = "message"]`
  - `#[skip]`

```rust
use patina_test::{patina_test, error::Result};

#[cfg_attr(target_arch = "x86_64", patina_test)]
fn spi_smoke_test() -> Result {
    patina::u_assert!(spi::probe(), "SPI controller missing");
    Ok(())
}

#[patina_test]
#[should_fail = "Expected watchdog trip"]
fn watchdog_negative_path() -> Result {
    patina::u_assert_eq!(watchdog::arm(), Err("trip"));
    Ok(())
}
```

### `devpath!`

- Converts a Device Path from a text string literal into an owned `[u8; N]` at compile time.
- Supports standard device path nodes, aliases, decimal and hexadecimal integers, named parameters, and multiple
  device path instances.
- Inserts End Instance nodes between top-level comma-separated instances and an End Entire node after the final
  instance.
- Requires no runtime parser, allocation, or Patina device-path feature in the consuming crate.
- Reports malformed syntax, invalid fields, unknown nodes, and unrepresentable values as compile-time errors.
- Supports syntax for specifying vendor-defined hardware, messaging and media device path nodes.

```rust
use patina::devpath;

const PCI_DEVICE_PATH: [u8; 22] = devpath!("PciRoot(0)/Pci(0x11,0)");
const MULTI_INSTANCE_PATH: [u8; 20] = devpath!("Pci(1,0),USB(2,1)");
```

The macro accepts one device path string, optionally preceded by a `vendor-defined` registry. Within the string,
separate nodes with `/` and device path instances with a top-level comma. Backslashes are preserved in file path nodes.

The registry defines invocation-local shortcuts for vendor hardware, messaging, and media nodes:

```rust
const VENDOR_PATH: &[u8] = &devpath!(
    vendor-defined {
        AcmeController {
            type: hardware,
            guid: "00112233-4455-6677-8899-aabbccddeeff",
            fields: [port: u8, flags: u16le],
        },
    };
    "PciRoot(0)/AcmeController(port=3,flags=0x1234)"
);
```

The required `type` property selects a UEFI vendor hardware (`VenHw`), vendor messaging (`VenMsg`), or vendor media
(`VenMedia`) node using `hardware`, `messaging`, or `media`, respectively. Fields are required and may be supplied
positionally or by name. Supported field types are `u8`, `u16le`, `u32le`, `u64le`, `guid` (EFI byte order), `uuid`
(RFC byte order), and `bytes` (hexadecimal byte data). Built-in node names cannot be redefined.
