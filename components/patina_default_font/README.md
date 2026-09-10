# Patina Default Font Component

A Patina (Rust) port of the default "simple font" package. This is the same font the EDK II `GraphicsConsoleDxe` driver
registers with the HII database. This crate only produces font data and registers it.

Names such as `EFI_HII_FONT_PROTOCOL` mentioned in this file refer to UEFI Specification type names.

## Background

`EFI_HII_FONT_PROTOCOL`'s system default font (the font every `StringToImage()`/`OutputString()` call renders with
when no specific font is requested) resolves glyphs from `EFI_HII_PACKAGE_SIMPLE_FONTS` package(s) registered in the
HII database. `HiiDatabaseDxe` implements the protocol but doesn't include glyph data.

The C `GraphicsConsoleDxe` driver worked around this by registering its own default font package the first time
`EFI_HII_DATABASE_PROTOCOL` became available. Rather than coupling that same data and registration to a single
console/rendering component, this crate provides it independently, so it can be added and removed easily as needed.

Any platform that needs default glyphs available can register this component, and platforms that supply their own font
package(s) can leave it out.

## Components

`DefaultFontProvider` is the only component. It depends on `EFI_HII_DATABASE_PROTOCOL` being installed, expressed as
a `Protocol<hii_database::Protocol>` component parameter, so it is only dispatched once the HII Database driver has
started.

It's registered like any other component:

```rust,ignore
commands.add_component(DefaultFontProvider::new());
```

## License

Copyright (c) Microsoft Corporation.

The glyph data is unmodified from `LaffStd.c` in EDK II:
Copyright (c) 2006 - 2008, Intel Corporation. All rights reserved.<br>
SPDX-License-Identifier: BSD-2-Clause-Patent

SPDX-License-Identifier: Apache-2.0
