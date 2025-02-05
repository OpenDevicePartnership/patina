# Setting up the DXE Core

Now that we have the scaffolding up for our crate, lets actually set up the crate to compile a pure
rust DXE Core binary. For this section, we will be working out of the crate directory, and all
steps will be the same, regardless of if you are compiling local or external to the platform.

## Add the dxe_core dependency

Inside your crate's Cargo.toml file, add the following, where `$(VERSION)`: is replaced with the
version of the dxe_core you wish to use.

``` toml
[dependencies]
dxe_core = "$(VERSION)"
```

````admonish note
If you want the latest and greatest, you can use the `main` branch from our github repository:

``` toml
dxe_core = { git = "https://github.com/OpenDevicePartnership/uefi-dxe-core", branch = "main" }
```
````

## main.rs Boilerplate

The next step is to add the the following boilerplate to the `main.rs` file. This sets up the
necessary scaffolding such that the only step the platform needs to do, is select its dependencies
and configurations. As stated in the [Introduction](../introduction.md), dependency and
configuration is done in code, rather than through configuration files like the INF, DSC, FDF, and
DEC.

``` rust
#![cfg(all(target_os = "uefi"))]
#![no_std]
#![no_main]

use core::{ffi::c_void, panic::PanicInfo};

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("{}", info);
    loop {}
}

#[cfg_attr(target_os = "uefi", export_name = "efi_main")]
pub extern "efiapi" fn _start(physical_hob_list: *const c_void) -> ! {
    loop {}
}
```

At this point, you can execute `cargo make build` and successfully build an efi image for the DXE
Core. You could even skip the rest of this section, going straight to adding it to your platform.
It would build and execute. But by execute, I mean it would just dead-loop!

Lets talk about each of these sections real quick. Since we are in a `no_std` environment (as
specified by `#![no_std]`), we must define our own panic handler. For now, we will just log the
error and dead-loop. If you wish, you could write something more sophisticated.

Next, we have to tell `rustc` to use the `efiapi` ABI, and to export this function as `efi_main`,
which is exactly what the line `#[cfg_attr(target_os = "uefi", export_name = "efi_main")]` is
doing. We then describe the function interface as consuming a pointer to the `physical_hob_list`,
which is the definition for the DXE Core. If we were making a DXE Driver, it would be different.

## DXE Core Boilerplate

Now that we are to the point where you can compile a binary that the `PEI` phase can locate and
execute, lets actually add the DXE Core logic. In this section, you will also need to make some
decisions on trait implementations, which are used as abstraction points for the platform to add
architecture or platform specific logic. At the point of writing this example, `DxeCore` has two
points of abstraction, `SectionExtractor` and `EfiCpuInit`.

`SectionExtractor` is an abstraction point that allows a platform specify the specific section
extraction methods it supports. As an example, a platform may only compress it's sections with
brotli, so it only needs to support brotli extractions. A platform may create their own extractor,
it only needs implement the [SectionExtractor](https://github.com/microsoft/mu_rust_pi/blob/c8dd7f990d87746cfae9a5e821ad69501c46f346/src/fw_fs.rs#L77)
trait. However multiple implementations are provided via [section_extractor](https://github.com/OpenDevicePartnership/uefi-core/tree/main/section_extractor),
such as brotli, crc32, uefi_decompress, etc.

`EfiCpuInit` is an abstraction point for architecture specific initialization steps.
Implementations are provided via [uefi_cpu](https://github.com/OpenDevicePartnership/uefi-core/tree/main/uefi_cpu),
however if necessary, a platform can create their own implementation via the [EfiCpuInit](https://github.com/OpenDevicePartnership/uefi-core/blob/main/uefi_core/src/interface.rs)
trait.

```admonish note
If there are any new traits added, please submit a PR to update this documentation.
```

With all of that said, you can add the following code to `main.rs`, replacing the implementations
in this example with your platform specific implementations:

```rust
use dxe_core::Core;
use uefi_cpu::X64EfiCpuInit;
use section_extractor::BrotliSectionExtractor;

#[cfg_attr(target_os = "uefi", export_name = "efi_main")]
pub extern "efiapi" fn _start(physical_hob_list: *const c_void) -> ! {
    Core::default()
        .with_section_extractor(BrotliSectionExtractor::default())
        .with_cpu_init(X64EfiCpuInit::default())
        .init_memory(physical_hob_list)
        .start()
        .unwrap()
    loop {}
}
```

``` admonish note
If you copy + paste this directly, the compiler will not know what `uefi_cpu` or
`section_extractor` is. You will have to add that to your platform's `Cargo.toml` file.
Additionally, where the `Default::default()` option is, this is where you would provide and
configurations to the DXE Core, similar to a PCD value.
```

At this point, you could skip the rest of this section and move on to compiling it into the
platform firmware, and it would run! However you would not get any logs! so lets set up a logger.

### Setting up a logger

We will start off simple by configuring and initializing a logger that is used throughout the
execution of the DXE Core. If you add any monolithic-ally compiled drivers (we will get to that
soon), then this same logger will also be used by that too!

The DXE Core uses the same logger interface as [log](https://crates.io/crates/log), so if you wish
to create your own logger, follow those steps. We currently provide two loggers, an [adv_logger](https://dev.azure.com/microsoft/MsUEFI/_git/DxeRust?path=/adv_logger)
implementation, which is great for this tutorial as it will also show you how to add a monolithic
compiled driver to the dxe_core.

First, add `adv_logger to your Cargo.toml file in the crate:

``` toml
adv_logger = "$(VERSION)
```

Next, update main.rs with the following:

``` rust
use adv_logger::{AdvancedLogger, init_advanced_logger};

static LOGGER: AdvancedLogger<uefi_sdk::serial::Uart16550> = AdvancedLogger::new(
    uefi_sdk::log::Format::Standard,
    &[
        ("goblin", log::LevelFilter::Off),
        ("uefi_depex_lib", log::LevelFilter::Off),
        ("gcd_measure", log::LevelFilter::Off),
    ],
    log::LevelFilter::Trace,
    uefi_sdk::serial::Uart16550::new(uefi_sdk::serial::Interface::Io(0x402)),
);

#[cfg_attr(target_os = "uefi", export_name = "efi_main")]
pub extern "efiapi" fn _start(physical_hob_list: *const c_void) -> ! {
    log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Trace)).unwrap();
    let adv_logger = AdvancedLoggerComponent::<Uart16550>::new(&LOGGER);
    adv_logger.init_advanced_logger(physical_hob_list).unwrap();

    Core::default()
        .with_section_extractor(BrotliSectionExtractor::default())
        .with_cpu_init(X64EfiCpuInit::default())
        .init_memory(physical_hob_list)
        .with_component(adv_logger)
        .start()
        .unwrap()
    loop {}
}
```

This does a few things. The first is it creates our actual logger, with some configuration
settings. Specifically it sets the log message format, disables logging for a few modules,
sets the minimum log type allowed, then specifies the Writer that we want to write to. In this
case we are writing to port `0x402` via `Uart16550`. Again, this is just our advanced logger
implementation, This could be different if you create your own.

The next static that we generate is the component that gets executed during runtime, which
initializes and publishes the advanced logger so that regular EDKII built components also have
access to the advanced logger.

Next, is we set the global logger to our static logger. That is just a `log` crate thing. Finally,
we initialize the component, and then add it to the list of components that the `dxe_core` will
execute.

## Final main.rs

``` rust
#![no_std]
#![no_main]
extern crate alloc;

use adv_logger::{AdvancedLogger, init_advanced_logger};
use core::{ffi::c_void, panic::PanicInfo};
use dxe_core::Core;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("{}", info);
    loop {}
}

static LOGGER: AdvancedLogger<uefi_sdk::serial::Uart16550> = AdvancedLogger::new(
    uefi_sdk::log::Format::Standard,
    &[
        ("goblin", log::LevelFilter::Off),
        ("uefi_depex_lib", log::LevelFilter::Off),
        ("gcd_measure", log::LevelFilter::Off),
    ],
    log::LevelFilter::Trace,
    uefi_sdk::serial::Uart16550::new(uefi_sdk::serial::Interface::Io(0x402)),
);

#[cfg_attr(target_os = "uefi", export_name = "efi_main")]
pub extern "efiapi" fn _start(physical_hob_list: *const c_void) -> ! {
    log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Trace)).unwrap();
    let adv_logger = AdvancedLoggerComponent::<Uart16550>::new(&LOGGER);
    adv_logger.init_advanced_logger(physical_hob_list).unwrap();

    Core::default()
        .with_cpu_init(uefi_cpu::EfiCpuInitX64::default())
        .with_section_extractor(section_extractor::CompositeSectionExtractor::default())
        .init_memory(physical_hob_list)
        .with_driver(adv_logger)
        .start()
        .unwrap();

    log::info!("Dead Loop Time");
    loop {}
}
```
