<!-- Copyright (c) Microsoft Corporation. All rights reserved. -->
# USB HID

## Overview

This Patina component provides USB Human Interface Device support for UEFI by consuming the
`EFI_USB_IO_PROTOCOL` on USB HID device controllers and producing the
[HidIo](../../../sdk/patina/src/vendor_protocols/hid_io.rs) protocol for
each managed device.

The HidIo protocol is then consumed by downstream components (e.g. `uefi_hid`) to provide keyboard, pointer, and
other HID input support.

## Architecture

The component installs a UEFI Driver Binding that manages USB HID device instances. The driver follows the standard
UEFI Driver Model:

1. **Supported** — checks if a controller has USB IO with HID interface class.
2. **Start** — reads USB descriptors, configures report protocol mode for boot devices, and installs the HidIo
   protocol on the controller handle.
3. **Stop** — shuts down async transfers, uninstalls the protocol, and frees resources.

Asynchronous input reports are delivered via USB interrupt-in transfers. A timer-based delayed recovery mechanism
handles USB transfer errors.

## Modules

| Module | Description |
| --- | --- |
| `control_transfers` | USB control transfer helpers for HID devices (set protocol, set/get report). |
| `descriptors` | USB descriptor reading for HID devices (HID descriptor, report descriptor). |
| `device` | Per-device state for USB HID devices. |
| `driver` | Driver binding implementation that manages USB HID device instances on controllers. |
| `hid_io_impl` | HidIoProtocol function pointer implementations — delegates to USB IO operations. |
| `interrupt_transfers` | Async interrupt transfer management and error recovery. |

## Dependencies

Key crate dependencies (see `Cargo.toml` for the full list):

- [`patina`](https://crates.io/crates/patina) — Patina component SDK (boot services, driver binding, protocol interfaces).
- [`r-efi`](https://crates.io/crates/r-efi) — Rust UEFI type definitions.

## Platform Integration

To include `usb_hid` in a Patina binary, add the crate as a dependency and register the component
in the platform's `ComponentInfo` implementation.

1. Add the dependency to the binary crate's `Cargo.toml`:

   ```toml
   [dependencies]
   usb_hid = { version = "20" }
   ```

2. Register the component in the `components` function:

   ```rust
   impl ComponentInfo for MyPlatform {
       fn components(mut add: Add<Component>) {
           // ...other components...
           add.component(usb_hid::UsbHidComponent);
       }
   }
   ```

The driver binding will automatically attach to any controller that exposes the USB IO protocol with a HID interface
class. A HidIo consumer (e.g. [`uefi_hid`](../uefi_hid)) must be present in the platform firmware for the produced
HidIo protocol to be functional.

## Testing

Unit tests use `mockall` and `patina`'s mock boot services:

```sh
cargo test -p usb_hid
```
