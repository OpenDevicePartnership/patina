# Standalone Compilation

This section will walk you through setting up a new repository such that you
can build a standalone binary using only the rust ecosystem and later provide
it to the EDKII build system and your platform.

## Create the repository

Start off by creating a new rust crate, in the following example, we will call the crate
`PlatformDxeCore`, however please substitute it with a name meeting your needs.

From the terminal, in the directory you wish to generate a crate, execute the following command:

``` txt
> cargo new PlatformDxeCore
> cd PlatformDxeCore
```

You should see the following folder structure:

``` txt
├── src
|    └── main.rs
├── .gitignore
└── Cargo.toml
```

## Makefile.toml

As mentioned in the [Introduction](../introduction.md), `cargo-make` is used to help simplify the
user experience by automatically providing the command line arguments necessary to build for a
UEFI target. From the root of your platform repository, run the following command:

`> touch Makefile.toml`

Finally, Add the following contents to the file:

``` toml
{{#include ../files/Makefile.toml}}
```

Okay! That is a lot! So lets talk about it. This is a bare-bones makefile whose only command is
`cargo make build`, which will build `main.rs` as an efi binary. The default architecture target is
`X64`, as specified by the `--target x86_64-unknown-uefi` flag in the `NO_STD_FLAGS` variable.
You can change that to meet your platform needs

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
├── src
|    └── main.rs
├── .gitignore
├── Cargo.toml
├── Makefile.toml
└── rust-toolchain.toml
```
