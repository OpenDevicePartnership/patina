# Patina PCD (Platform Configuration Database) Component Crate

Patina PCD lets other Patina components get and set `Dynamic` and `DynamicEx` PCDs through the PI `PCD_PROTOCOL`
produced by a platform's C PCD DXE driver. It does not implement PCD storage itself, it only locates the protocol and
exposes it as a `Pcd` component service. As a general stance, Patina does not support PCDs and will never
produce the PCD protocols itself or implement PCD storage directly. PCDs were a custom configuration mechanism used in
the TianoCore/EDK II build process dependent on C `AutoGen` tooling and infrastructure incompatible with Patina's goals.

This component is available to provide standardized access to Dynamic PCDs within Patina components where those
components must participate within a larger software stack that depends on PCDs being dynamically set and read.

It is not intended for use in a component that does not have these pre-existing dependencies.

Also note that only Dynamic PCDs are supported. Static PCDs (and similar types like `FeaturePcds`) are not supported, as
they are build-time constructs in the C build process that do not exist at runtime.

## Capabilities

- Produces the `Pcd` service (defined in `patina::component::service::pcd`), giving typed get/set access to
  8/16/32/64-bit, boolean, and variable-length Dynamic and `DynamicEx` PCDs.
- Dispatches after the `PCD_PROTOCOL` interface is installed (using the `Protocol<PcdProtocol>` component parameter).
- Only PCD get/set is in scope. SKU selection, set-callbacks, and token/token-space enumeration are not exposed as they
  are considered platform build-time or advanced C-driver concerns outside typical component use.

## `DynamicEx` vs. `Dynamic` PCDs

The `Pcd` service accepts both `PcdToken::DynamicEx(guid, token)` and `PcdToken::Dynamic(token)`, matching the two
token-addressing forms `PCD_PROTOCOL` itself exposes, but the two are not equally practical to use from a Patina
component.

A `DynamicEx` PCD's token number is the fixed `Token` value declared in the owning package's `.dec` file. EDK II
requires that value to be identical everywhere the PCD is referenced, so it is a stable, documented constant a Patina
component can be written against using the value directly from the `.dec` file and hardcode, independent of any specific
platform's build. This is also why EDK II reserves `DynamicEx` for modules (such as binaries) that are not part of the
platform's own DSC/FDF build. The same property that makes it usable from outside EDK II's `AutoGen` tooling makes it
usable from Patina.

A `Dynamic` PCD does not have a fixed token number. Its runtime token is assigned by `BaseTools`' `AutoGen` step as an
index into a table it builds while processing that specific platform's DSC/FDF, based on every Dynamic PCD included in
that build. A C module only gets the correct value because the same `AutoGen` pass that assigns it also regenerates that
module's `AutoGen.h` with a matching macro. Patina components are built by Cargo, entirely outside that process, so
there is no supported way to obtain the correct token number for a `Dynamic` PCD, and a value captured from one platform
build is not guaranteed to still be correct after the platform's PCD set changes in any way, not just for the PCD in
question.

`PcdToken::Dynamic` is still provided because `PCD_PROTOCOL` defines it, and some platforms may already have an
out-of-band way to communicate the right token number, but `DynamicEx` should be preferred whenever a platform
integrator can choose the PCD's access method, because its token number is stable and independently knowable.

## Platform Managed Components and Services

- **`PcdProvider` component**: Locates `PCD_PROTOCOL` and registers the `Pcd` service for other components to
  consume.
- **`Pcd` service**: Consumed the same way as any other Patina component service. Only components with a direct
  dependency on a specific PCD's value should take this dependency.

## Platform Integration Guidance

A platform that wants Patina components to be able to read or write Dynamic PCDs needs a C PCD DXE driver (for example,
EDK II's `MdeModulePkg/Universal/PCD/Dxe`) already dispatched to produce `PCD_PROTOCOL`. In the Patina DXE Core binary
file, register `PcdProvider` so the `Pcd` service becomes available to other Patina components.

```rust,no_run
use patina_dxe_core::*;
use patina_ffs_extractors::NullSectionExtractor;
use patina_pcd::component::provider::PcdProvider;

struct ExamplePlatform;

impl MemoryInfo for ExamplePlatform {}

impl CpuInfo for ExamplePlatform {}

impl ComponentInfo for ExamplePlatform {
    fn components(mut add: Add<Component>) {
        // ... other components ...
        add.component(PcdProvider::new());
    }
}

impl PlatformInfo for ExamplePlatform {
    type MemoryInfo = Self;
    type CpuInfo = Self;
    type ComponentInfo = Self;
    type Extractor = NullSectionExtractor;
    const UEFI_SPEC_VERSION: patina::UefiSpecVersion = patina::UefiSpecVersion::V2_11;
}

static CORE: Core<ExamplePlatform> = Core::new(NullSectionExtractor::new());
```

## Examples

A component that needs a PCD's value takes a `Service<dyn Pcd>` parameter in its entry point, then calls the
accessor matching the PCD's declared type and token form. Every accessor is `unsafe` because `PCD_PROTOCOL` cannot
validate the token ahead of time. See [Safety](#safety) for more details.

### Reading a `DynamicEx` scalar PCD

```rust,no_run
use patina::{
    component::{component, service::Service},
    component::service::pcd::{Pcd, PcdError, PcdToken},
    error::Result,
    BinaryGuid,
};

const MY_TOKEN_SPACE_GUID: BinaryGuid = BinaryGuid::from_string("550e8400-e29b-41d4-a716-446655440000");
const PCD_MY_SETTING_TOKEN: usize = 0x1;

#[derive(Default)]
pub struct MyComponent;

#[component]
impl MyComponent {
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, pcd: Service<dyn Pcd>) -> Result<()> {
        let token = PcdToken::DynamicEx(MY_TOKEN_SPACE_GUID, PCD_MY_SETTING_TOKEN);

        match unsafe { pcd.get_u32(token) } {
            Ok(value) => log::info!("PcdMySetting is {value:#x}"),
            Err(PcdError::NotFound) => log::warn!("PcdMySetting is not registered on this platform"),
            Err(err) => log::error!("Failed to read PcdMySetting: {err}"),
        }

        Ok(())
    }
}
```

### Writing a scalar PCD

```rust,no_run
# use patina::component::service::pcd::{Pcd, PcdToken};
# use patina::BinaryGuid;
# fn example(pcd: &dyn Pcd, token_space: BinaryGuid) -> Result<(), patina::component::service::pcd::PcdError> {
let token = PcdToken::DynamicEx(token_space, 0x2);
unsafe { pcd.set_bool(token, true) }?;
# Ok(())
# }
```

### Reading and writing a variable-length PCD

Variable-length PCDs (for example a string or byte-array setting) are accessed as `Vec<u8>` through `get_bytes` and
`set_bytes`. `set_bytes` returns `PcdError::BufferTooLarge` if `value` exceeds the PCD's maximum declared size.

```rust,no_run
# use patina::component::service::pcd::{Pcd, PcdToken};
# use patina::BinaryGuid;
# fn example(pcd: &dyn Pcd, token_space: BinaryGuid) -> Result<(), patina::component::service::pcd::PcdError> {
let token = PcdToken::DynamicEx(token_space, 0x3);

let mut bytes = unsafe { pcd.get_bytes(token) }?;
bytes.push(0xFF);
unsafe { pcd.set_bytes(token, &bytes) }?;
# Ok(())
# }
```

### Using a `Dynamic` (non-`Ex`) token

`PcdToken::Dynamic` is addressed only by token number, with no token space GUID. As noted above, `DynamicEx`
tokens should be preferred whenever the platform integrator can choose the access method and `Dynamic` might
not be practical at all given its token number is not fixed across `AutoGen` passes.

```rust,no_run
# use patina::component::service::pcd::{Pcd, PcdToken};
# fn example(pcd: &dyn Pcd) -> Result<u16, patina::component::service::pcd::PcdError> {
let token = PcdToken::Dynamic(0x10);
unsafe { pcd.get_u16(token) }
# }
```

## Safety

A PCD token number is an opaque value assigned by the platform's (C/non-Patina) build tooling. `PCD_PROTOCOL` has no way
to validate a token ahead of time, so every accessor on the `Pcd` service is considered `unsafe`. The caller must ensure
the token refers to a PCD that is registered in the platform's PCD database and holds a value of the type being read or
written. See the `Pcd` trait documentation for the full safety contract.

## License

Copyright (c) Microsoft Corporation.

SPDX-License-Identifier: Apache-2.0
