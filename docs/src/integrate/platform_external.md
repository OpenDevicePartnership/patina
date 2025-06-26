# External to the Platform

This section assumes you are attempting to provide the pure Patina DXE Core as a pre-compiled binary
to the EDK II build system and that you have your crate setup as specified in [Workspace Setup](./compile_external.md).

There are a few common ways to consume an EFI binary in the EDK II build system, each of which will have its pros and
cons. The two common ways are to create a Module Information (INF) file with a `[Binaries]` section while the second
way common way is to use the `FILE` keyword in your platform's Flash Description (FDF) file.

In addition to this, you also have to decide how you wish to consume this binary in general. Do you copy the binary
directly into the repository? Do you consume it via a versioned submodule? Do you use [external dependency](https://www.tianocore.org/edk2-pytool-extensions/features/extdep/)
functionality provided via [stuart](https://www.tianocore.org/edk2-pytool-extensions/)?

For simplicity, this guide assumes you're copying the binary directly into your repository. However, **this approach is
not recommended for production use**. At minimum, you should implement version control specifically for the binary
artifact. To support active development workflows, we'll also demonstrate how to override the binary path at compile
time.

## Module Information (INF) file

This method is almost exactly the same as updating [Local to the Platform](./platform_local.md) and
you should follow those exact steps. The **only** difference is what you put in the the Module
Information file:

``` toml
{{#include ../files/PlatformDxeCoreBinary.inf}}
```

Note the addition of `DEFINE DXE_CORE_BASE_PATH =`; This is what allows you to provide a path change to the target
binary while building. By adding this define on the command line of the build command, you can easily switch the
targeted binary. The third and final change is that we added a `Binaries` section, with paths to the Patina DXE Core
depending on the build target (`DEBUG`, `RELEASE`, `NOOPT`).

As mentioned above, other than changing the INF file, you can follow the [Local to the Platform](./platform_local.md)
documentation for all additional steps.
