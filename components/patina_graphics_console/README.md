# Patina Graphics Console Component

A Patina (Rust) implementation of `GraphicsConsoleDxe`. The main responsibility of this component is to bind a
EFI Simple Text Output console to every Graphics Output Protocol (GOP) controller in the system, rendering text through
the EFI HII Font Protocol.

Note that some protocol names such as `EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL` refer to the type name used for the protocol
definition in the UEFI Specification. When referring to the generic protocol interface in Patina, that type name may be
used, though the Rust type name for the same protocol interface will differ.

## Capabilities

- Participates in the UEFI Driver Model (`EFI_DRIVER_BINDING_PROTOCOL`)
  - Binds to every GOP controller present, including multiple displays, and responds to
    `ConnectController()`/`DisconnectController()`.
- Installs `EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL` on each controller it starts, implementing the functionality behind the
  protocol's interface functions.
- Publishes `EFI_COMPONENT_NAME_PROTOCOL`/`EFI_COMPONENT_NAME2_PROTOCOL`.
- Computes the standard text-mode list (80x25, 80x50 when supported, a set of common resolutions, and a full-screen
  mode) from the controller's available GOP resolutions.
- Supports reading the Dynamic PCDs used in existing C code to set resolution configuration during boot:
  - `PcdVideoHorizontalResolution`
  - `PcdVideoVerticalResolution`
  - `PcdConOutColumn`
  - `PcdConOutRow`

## Components

`GraphicsConsoleProvider` is the main component. It depends on `ProtocolServices`, `TplServices`, and `PcdServices`
(optional). Driver binding is installed right away and the real work happens later, per controller, in
`Supported()`/`Start()`/`Stop()`.

It's registered like any other component:

```ignore
commands.add_component(GraphicsConsoleProvider::new());
```

## PCD integration

A number of standard UEFI C drivers set the resolution of the graphics console through PCDs.

To allow interoperability with those drivers, this component will read those PCDs if the `PcdServices` service is
available which is only produced when a C PCD driver is present in the system.

`PcdVideoHorizontalResolution`, `PcdVideoVerticalResolution`, `PcdConOutColumn`, and `PcdConOutRow` are read as
`DynamicEx` PCDs using the fixed token numbers declared in `MdePkg`'s `MdeModulePkg.dec`
(`gEfiMdeModulePkgTokenSpaceGuid`, tokens `0x40000009`, `0x4000000a`, `0x40000007`, `0x40000006`). Unlike ordinary
`Dynamic` PCD token numbers (which are assigned by the platform's build tooling and are not portable across platforms),
these `DynamicEx` token numbers are fixed in the `.dec` file itself.

`Service<dyn PcdServices>` is an optional dependency. On a platform with no PCD driver, this component still starts and
defaults to the display's highest available resolution and largest text mode (a value of `0` for any of the four PCDs
means the same thing).

## Default font package

`EFI_HII_FONT_PROTOCOL`'s system default font (the font every `OutputString()` call renders with) resolves glyphs from
`EFI_HII_PACKAGE_SIMPLE_FONTS` package(s) registered in the HII database. `HiiDatabaseDxe` implements the protocol but
doesn't include glyph data of its own.

Since this component replaces `GraphicsConsoleDxe`, `console::font_package` ports the same registration done in the
EDK II C driver where `console::font_data` embeds the identical glyph bitmap (printable ASCII and the box/shape
characters the UEFI specification requires).

It's registered with the HII database the first time a controller starts, so a platform only needs the HII Database
driver in its firmware volume and is not required to carry a separate font package driver for text to render.

## Intentional deviations from `GraphicsConsoleDxe`

- The initial text mode is applied immediately in `Start()` (cleared to the default colors) instead of being left for a
  later `SetMode()` call from a caller such as `ConSplitterDxe`. This avoids exposing a half-initialized console
  allowing the component to be more self-sufficient. `SetMode()` can still be called later to switch modes.
- `OutputString()` only erases & redraws the cursor block once per call (at the start and end) rather than after every
  intermediate cursor move, since the cursor position is tracked locally during processing and only committed to the
  `Mode` struct once. The on-screen result is identical, with fewer `Blt` calls.
- Backspace erasure draws a blank cell directly instead of recursively re-entering `OutputString()` with a synthetic
  string, which would double-acquire this component's internal lock and panic.

## License

Copyright (c) Microsoft Corporation.

SPDX-License-Identifier: Apache-2.0
