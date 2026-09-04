# Patina PCD (Platform Configuration Database) Component Crate

Patina PCD lets other Patina components get and set Dynamic and `DynamicEx` PCDs through the PI `PCD_PROTOCOL`
produced by a platform's C PCD DXE driver. It does not implement PCD storage itself, it only locates the protocol and
exposes it as a `PcdServices` component service. As a general stance, Patina itself does not support PCDs and will
never produce the PCD protocols itself or implement PCD storage directly.

This component is available to provide standardized access to Dynamic PCDS within Patina components where those
components must participate within a larger software stack that depends on PCDs being dynamically set and read.

It is not intended for use in a component that does not have these pre-existing dependencies.

Also note that only Dynamic PCDs are supported. Static PCDs (and similar types like `FeaturePcds`) are not supported,
as they are build-time constructs in the C build process that do not exist at runtime.

## Capabilities

- Produces the `PcdServices` service (defined in `patina::component::service::pcd`), giving typed get/set access to
  8/16/32/64-bit, boolean, and variable-length Dynamic and `DynamicEx` PCDs.
- Dispatches after the `PCD_PROTOCOL` interface is installed (using the `Protocol<PcdProtocol>` component parameter).
- Only PCD get/set is in scope. SKU selection, set-callbacks, and token/token-space enumeration are not exposed as
  they are considered platform build-time or advanced C-driver concerns outside typical component use.

## Platform Managed Components and Services

- **`PcdProvider` component**: Locates `PCD_PROTOCOL` and registers the `PcdServices` service for other components
  to consume.
- **`PcdServices` service**: Consumed the same way as any other Patina component service. Only components with a
  direct dependency on a specific PCD's value should take this dependency.

## Platform Integration Guidance

A platform that wants Patina components to be able to read or write Dynamic PCDs needs a C PCD DXE driver (for example,
EDK II's `MdeModulePkg/Universal/PCD/Dxe`) already dispatched to produce `PCD_PROTOCOL`. In the Patina DXE Core binary
file, register `PcdProvider` so the `PcdServices` service becomes available to other Patina components.

```rust,ignore
use patina_dxe_core::*;
use patina_pcd::component::provider::PcdProvider;

let core = Core::default()
    // ... other configuration ...
    .with_component(PcdProvider::new());
```

## Safety

A PCD token number is an opaque value assigned by the platform's (C/non-Patina) build tooling. `PCD_PROTOCOL` has no
way to validate a token ahead of time, so every `PcdServices` accessor is considered `unsafe`. The caller must ensure
the token refers to a PCD that is registered in the platform's PCD database and holds a value of the type being read or
written. See the `PcdServices` documentation for the full safety contract.

## License

Copyright (c) Microsoft Corporation.

SPDX-License-Identifier: Apache-2.0
