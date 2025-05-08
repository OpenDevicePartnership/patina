# Compiling through EDK II

This section will walk you through adding a rust crate to your existing platform repository so
that it can be compiled via the existing EDK II build system.

## Create the crate

Start off by determining where you want to place the crate representing your platform's pure rust
DXE core. Every platform is different, so yours may be too, but a common place for Platform
specific components is inside the platform package:

``` txt
> cd MyPlatformPkg
> touch PlatformDxeCore
> cargo new PlatformDxeCore
> cd PlatformDxeCore
> rm .gitignore
```

You should see the following folder structure:

``` txt
├── PlatformPkg
     └── PlatformDxeCore
          ├── src
          |    └── main.rs
          └── Cargo.toml
```

## Cargo.toml

With the crate not also being the root of the repository, we need to add a Cargo.toml file at the
root of the repository. This lets cargo know where one or multiple crates are located, since they
are not the repository themselves. From the root of your platform repository, run the following
command:

`> touch Cargo.toml`

Finally, Add the following contents to the file:

``` toml
{{#include ../files/Cargo.toml}}
```

**Note:** There are many other things that you can do with the root level `Cargo.toml` file. This
is just the bare minimum to get you building.

## Makefile.toml

As mentioned in the [Introduction](../introduction.md), `cargo-make` is used to help simplify the
user experience by automatically providing the command line arguments necessary to build for a UEFI
target. From the root of your platform repository, run the following command:

`> touch Makefile.toml`

Finally, Add the following contents to the file:

``` toml
{{#include ../files/Makefile.toml}}
```

Okay! That is a lot! So lets talk about it. This is a bare-bones makefile whose only command is
`cargo make build`, which will build `main.rs` as an efi binary. The default architecture target is
`X64`, as specified by the `--target x86_64-unknown-uefi` flag in the `NO_STD_FLAGS` variable. You
can change that to meet your platform needs

Rust supports building for the following UEFI target triples:

1. `x86_64-unknown-uefi`
1. `i686-unknown-uefi`
1. `aarch64-unknown-uefi`

## rust-toolchain.toml

While this file is not strictly necessary, it is highly recommended. It is used to control the tool
chain version that your binary is compiled with, helping to ensure reproducible builds across
developers. From the root of your platform repository, run the following command:

`> touch rust-toolchain.toml`

Add the following contents to the file, substituting the version with your expected version.

``` toml
{{#include ../files/rust-toolchain.toml}}
```

## Final Workspace

While there are additional files you could add, just as a rustfmt.toml, this is the bare minimum
workspace you need to build an EFI binary for a singular architecture:

``` txt
{{#include ../files/local_file_directory.txt}}
```
