# Local to the Platform

This section assumes you are attempting to compile the DXE Core binary through the EDKII build
system, that you already have your crate setup as specified in [Workspace Setup](./compile_local.md),
and that `cargo make build` successfully builds the binary.

## Add a Module Information (INF) File

The first step will be to add a INF to the workspace, which is used by the build system to provide
information regarding the module. If you are still in the directory of your crate, you can run the
following command:

`> touch PlatformDxeCore.inf`

Your workspace should now look like this:

``` txt
{{#include ../files/local_file_directory.txt}}
```

Filling out the module INF file is no different than any other module INF file except that the only
source that needs to be provided is the Cargo.toml file. The two important exceptions is that in
the `[DEFINES]` section, you must specify `RUST_MODULE=TRUE` and in the `[Sources]` section, the
only necessary source file is `Cargo.toml`. Here is an example of a filled out INF file, but you
may need to make changes depending on your platform:

``` toml
{{#include ../files/PlatformDxeCore.inf}}
```

## Updating the Platform Description (DSC) File

The next step is to add the INF to your platform's DSC file, which tells the build tools not only
that it *should* be built, but *how* it should be built. This is fairly simple; in the
`[Components]` section, or the `[Components.$(ARCH)]` section depending on your platform; comment
out the current DxeMain, and add the following line:

``` txt
# MdeModulePkg/Core/Dxe/DxeMain.inf
PlatformPkg/PlatformDxeCore/PlatformDxeCore.inf
```

## Updating the Platform Flash Description (FDF) File

The final step is to add the INF to your platform's FDF file, which tells the build tools *where*
to place the compiled file in the final image.

Wherever your current platform defines the current DxeMain, comment it out and replace it with the
pure rust version:

``` txt
# INF MdeModulePkg/Core/Dxe/DxeMain.inf
INF PlatformPkg/PlatformDxeCore/PlatformDxeCore.inf
```

## Finale

With these changes made, you should be able to run your platform's existing build script, whether
that be `build.py` directly, `stuart_build`, or a custom build script, and everything should work!
