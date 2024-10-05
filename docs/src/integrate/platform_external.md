# External to the Platform

This section assumes you are attempting to provide the pure rust DXE core as a pre-compiled binary
to the EDKII build system and that you have your crate setup as specified in [Workspace Setup](./compile_external.md).

There are a few common ways to consume an efi binary in the EDKII build system, each of which will
have it's pros and cons. The two common ways is to create a Module Information (INF) file with a
`[Binaries]` section while the second way common way is to use the `FILE` keyword in your
platform's FlashDescription (FDF) file.

In addition to this, you also have to decide how you wish to consume this binary in general. Do you
copy the binary directly into the repository? Do you consume it via a versioned submodule? Do you
use [external dependency](https://www.tianocore.org/edk2-pytool-extensions/features/extdep/)
functionality provided via [stuart](https://www.tianocore.org/edk2-pytool-extensions/)?

For simplicities sake, we will assume you are just copying the binary directly into the repository.
Please note that **This is not the recommended way**, and at a bare minimum, you should use some
type of versioning control specifically on the binary. We will also provide a path to change the
target binary at during compilation, to make active development simple.

## Module Information (INF) file

This method is almost exactly the same as updating [Local to the Platform](./platform_local.md) and
you should follow those exact steps. The **only** difference is what you put in the the Module
Information file:

``` toml
{{#include ../files/PlatformDxeCoreBinary.inf}}
```

From looking at the above, you should be able to see a few notable differences. The first was that
we removed `RUST_MODULE = TRUE` in the Defines section; this is because we are no longer compiling
it, so the build system does not need to know that this is a rust module. The second is that we
added `DEFINE DXE_CORE_BASE_PATH =`; This is what allows you to provide a path change to the target
binary while building. By adding this define on the command line of the build command, you can
easily switch the targeted binary. The third and final change is that we added a `Binaries`
section, with paths to the dxe_core depending on the build target (DEBUG, RELEASE, NOOPT).

As mentioned above, other than changing the INF file, you can follow the [Local to the Platform](./platform_local.md)
documentation for all additional steps.
