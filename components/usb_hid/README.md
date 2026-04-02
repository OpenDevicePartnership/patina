# usb_hid

USB HID driver that manages USB HID devices and produces the HidIo protocol as
a Patina component.

## Overview

This component is a port of the
[UsbHidDxe](https://github.com/microsoft/mu_plus/tree/release/202511/HidPkg/UsbHidDxe)
C driver to Rust. It implements a UEFI Driver Binding that:

- **Consumes** the `EFI_USB_IO_PROTOCOL` on USB HID device controllers
- **Produces** the `HID_IO_PROTOCOL` for each managed device

The HidIo protocol is then consumed by downstream components (e.g. `uefi_hid`)
to provide keyboard, pointer, and other HID input support.

## Architecture

The driver follows the standard UEFI Driver Model:

1. **Supported** — checks if a controller has USB IO with HID interface class
2. **Start** — reads USB descriptors, configures report protocol mode for boot
   devices, installs the HidIo protocol
3. **Stop** — shuts down async transfers, uninstalls protocol, frees resources

Asynchronous input reports are delivered via USB interrupt-in transfers. A
timer-based delayed recovery mechanism handles USB transfer errors.
