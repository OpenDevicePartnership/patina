# Patina Advanced Logger

The Patina Advanced Logger crate provides a [log::Log](https://crates.io/crates/log) implementation including in-memory
and serial logging capabilities and a component for producing the Advanced Logger protocol. The in-memory logging is
written to the advanced logger memory buffer, whose address is produced via the Advanced logger guided HOB described
below. In addition to the runtime functionality described above, this crate also supplies a command line tool to parse
parse logs pulled off of a physical device, and a public parser object for creating custom tooling.

## Memory Log Behavior

The crate stores records in a shared memory buffer that begins with an `ADVANCED_LOGGER_INFO` header.

- Aligned entries follow in the memory buffer after the header.
- Each entry records the boot phase identifier, EFI debug level mask, timestamp counter, and message bytes.

## Hardware Port Behavior

In addition to the memory log, records may be emitted to a hardware port. The port is abstracted behind the
`patina_adv_logger::hardware_port::AdvancedLoggerHardwarePort` trait. This allows a platform to control the
serial port and any filtering logic required.

- `SerialHardwarePort<S>` is the default implementation and writes every message it receives to the provided
  `SerialIO` instance.
- The logger applies the memory log's `HwPrintLevel` / `HwPortDisabled` filtering (including any per-target
  `hw_filter_override`) before calling the port, so a port implementation only sees records that already passed
  the standard filtering. The record's debug level is still passed to the port so platforms can apply
  additional, platform specific gating.
- Platforms that need to gate output (for example, on a debug jumper or GPIO) or emit to something other than a
  serial port can supply their own implementation of the trait instead of using `SerialHardwarePort`.

## Parser Support

This crate includes a bare-bones log parser executable for parsing the logs produced via the logger, and a public log
parser struct (`patina_adv_logger::parser::Parser`) that can be used to create custom tooling. Both are available via
the `std` feature, which exposes the `parser` module.

The command line executable accesses the buffer, prints header metadata, and emits log lines with optional level and
timestamp context. This parser underpins host utilities and remains version-aligned with the memory layout implemented
in `memory_log.rs`.

## Integration Instructions

### Patina DXE Core Integration instructions

Below are the instructions for setting up and configuring the patina component and logger implementations inside of
your platform's Patina DXE Core binary. Additional setup will be required for your Platform, which is discussed further
below.

1. Instantiate `AdvancedLogger` with the desired format, filters, level, and hardware port implementation.
   Register it with `log::set_logger` as early as possible.
2. Call `AdvancedLogger::init` with the physical HOB list pointer.
   This allows the logger to adopt the buffer and record its address for later protocol publication.
3. Register the advanced logger component (`AdvancedLoggerComponent`) to be dispatched by the Patina DXE Core so it
   can install the Advanced Logger protocol via boot services.

#### Example

```rust
# #[cfg(feature = "component")]
# {
use patina_dxe_core::*;
use patina::{debug::log::Format, peripheral::serial::uart::UartNull};
use patina_adv_logger::{
   component::AdvancedLoggerComponent,
   hardware_port::SerialHardwarePort,
   logger::{AdvancedLogger, TargetFilter},
};

use log::LevelFilter;
use core::ffi::c_void;

static LOGGER: AdvancedLogger<SerialHardwarePort<UartNull>> = AdvancedLogger::new(
   Format::Standard, // How logs are formatted
   &[TargetFilter { target: "allocations", log_level: LevelFilter::Off, hw_filter_override: None }], // set custom log levels per module
   log::LevelFilter::Info, // Default log level
   SerialHardwarePort::new(UartNull { }), // Hardware port instance
);

struct ExamplePlatform;

impl ComponentInfo for ExamplePlatform {
   fn components(mut add: Add<Component>) {
      add.component(AdvancedLoggerComponent::<SerialHardwarePort<UartNull>>::new(&LOGGER));
   }
}

#[cfg_attr(target_os = "uefi", unsafe(export_name = "efi_main"))]
pub extern "efiapi" fn _start(physical_hob_list: *const c_void) -> ! {
   log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Trace)).unwrap();

   // SAFETY: The physical_hob_list pointer is assumed to be valid as it is provided to the entry_point from an
   // external caller.
   if let Err(e) = unsafe { LOGGER.init(physical_hob_list) } {
      log::error!("Failed to find the Advanced Logger HOB. Cannot write to the memory log.");
   }

   # loop { }
}
# }
```

### Platform Integration

The Patina Advanced Logger expects that a log buffer has already been created prior to the Patina DXE Core being
executed. The location of this buffer is provided via a GUID HOB in the HOB list. So long as it is provided, the logger
and component will execute as expected. If your platform is a EDK II style platform, a [PEI Core Library](https://github.com/microsoft/mu_plus/blob/d0d305b620baced42adf16b2387af9412fdc0ef9/AdvLoggerPkg/Library/AdvancedLoggerLib/PeiCore/AdvancedLoggerLib.inf)
is available that will produce the HOB. The other option is to manually produce the Guided HOB with the following
format / guid:

guid: `{ 0x4d60cfb5, 0xf481, 0x4a98, { 0x9c, 0x81, 0xbf, 0xf8, 0x64, 0x60, 0xc4, 0x3e } }`
data: `[u8; 8]` (address (u64) of the log buffer)

As mentioned, the Patina component produces the Advanced Logger Protocol. This protocol can be used directly in
UEFI drivers to write to the buffer, or you can use existing abstractions, such as the [BaseDebugLibAdvancedLogger](https://github.com/microsoft/mu_plus/blob/release/202502/AdvLoggerPkg/Library/BaseDebugLibAdvancedLogger/BaseDebugLibAdvancedLogger.inf)
when compiling your EDK II style firmware.
