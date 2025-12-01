# Patina Macro Crate

`patina_macro` hosts the procedural macros used in Patina. This includes those that support the Patina component
system, service registration, guided HOB parsing, on-target test discovery, and more. The
[`patina`](https://crates.io/crates/patina) crate re-export these macros, so most cases only need a dependency on
`patina`.

## Notable Macros

### `#[derive(IntoComponent)]`

- Generates the boilerplate required for a struct or enum to satisfy `patina::component::IntoComponent`.
- Expects an owning `entry_point` method that consumes `self` and takes dependency-injected parameters implementing
  `ComponentParam`.
  - Note: The method name can be customized with `#[entry_point(path = <func>)]`.

```rust
use patina::component::{IntoComponent, params::Config};

#[derive(IntoComponent)]
struct BoardInit;

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
use patina::test::{patina_test, Result};

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
