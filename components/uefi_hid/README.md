<!-- Copyright (c) Microsoft Corporation. All rights reserved. -->
# UEFI HID

## Overview

This Patina component provides Human Interface Device (HID) support for UEFI. It consumes the
[HidIo](https://github.com/microsoft/mu_plus/blob/release/202502/HidPkg/Include/Protocol/HidIo.h) protocol and
produces standard UEFI input protocols for keyboard and pointer HID devices:

- **SimpleTextInput** (`EFI_SIMPLE_TEXT_INPUT_PROTOCOL`)
- **SimpleTextInputEx** (`EFI_SIMPLE_TEXT_INPUT_EX_PROTOCOL`)
- **AbsolutePointer** (`EFI_ABSOLUTE_POINTER_PROTOCOL`)

## Architecture

The component installs a UEFI Driver Binding that manages HID device instances. When the driver is
started on a controller that exposes the HidIo protocol, it:

1. Opens the HidIo protocol on the controller.
2. Parses the HID report descriptor to identify keyboard and pointer usages.
3. Creates the appropriate input protocol handlers (keyboard and/or pointer).
4. Installs the corresponding UEFI input protocols on the controller handle:
   - **SimpleTextInput** and **SimpleTextInputEx** for keyboard devices.
   - **AbsolutePointer** for pointer and touch devices.
5. Registers a report callback to receive asynchronous HID input reports.

### Report Processing

Incoming HID reports are buffered through a `ReportQueue` rather than being processed inline from the
HidIo producer's callback. This ensures all report processing occurs at a consistent `TPL_CALLBACK`
regardless of the producer's calling TPL:

1. **Report callback** (any TPL): pushes raw HID report bytes onto a queue and signals a `TPL_CALLBACK` event.
2. **Event handler** (`TPL_CALLBACK`): dequeues all pending reports and dispatches them to receivers.

## Modules

| Module | Description |
|---|---|
| `hid` | Driver binding implementation that manages HID instances on controllers. |
| `hid_io` | HidIo protocol FFI bindings, report queue, and receiver traits (`HidIo`, `HidReportReceiver`). |
| `keyboard` | Keyboard HID handler — translates HID key reports into UEFI keystrokes using HII keyboard layouts, and produces SimpleTextInput / SimpleTextInputEx protocol interfaces. |
| `pointer` | Pointer HID handler — translates HID pointer/touch reports into absolute pointer state and produces the AbsolutePointer protocol interface. |

## Features

| Feature | Default | Description |
|---|---|---|
| `ctrl-alt-del` | ✅ | Enables Ctrl+Alt+Delete to trigger a system reset via UEFI Runtime Services. |

## Dependencies

Key crate dependencies (see `Cargo.toml` for the full list):

- [`hidparser`](https://crates.io/crates/hidparser) — HID report descriptor parsing.
- [`patina`](https://crates.io/crates/patina) — Patina component SDK (boot services, driver binding, protocol interfaces).
- [`r-efi`](https://crates.io/crates/r-efi) — Rust UEFI type definitions.

## Platform Integration

To include `uefi_hid` in a Patina binary, add the crate as a dependency and register the component
in the platform's `ComponentInfo` implementation.

1. Add the dependency to the binary crate's `Cargo.toml`:

   ```toml
   [dependencies]
   uefi_hid = { path = "../components/uefi_hid" }
   ```

2. Register the component in the `components` function:

   ```rust
   impl ComponentInfo for MyPlatform {
       fn components(mut add: Add<Component>) {
           // ...other components...
           add.component(uefi_hid::UefiHidComponent);
       }
   }
   ```

The driver binding will automatically attach to any controller that exposes the HidIo protocol. A
HidIo producer (e.g. a USB HID driver) must be present in the platform firmware for this component
to be functional.

The `ctrl-alt-del` feature is enabled by default. To disable it:

```toml
uefi_hid = { path = "../components/uefi_hid", default-features = false }
```

## Testing

Unit tests use `mockall` and `patina`'s mock boot services:

```sh
cargo test -p uefi_hid
```
