
# Rust DXE Core

This repository contains a Pure Rust DXE Core.

## Background

There have been various [instances of advocacy](https://msrc-blog.microsoft.com/2019/11/07/using-rust-in-windows/) for
building system level software in [Rust](https://www.rust-lang.org/).

This repository contains a Rust DXE Core [UEFI](https://uefi.org/) firmware implementation. We plan to enable an
incremental migration of today's firmware components largely written in C to Rust. The primary objective for this
effort is to improve the security and stability of system firmware by leveraging the memory safety offered by Rust
while retaining similar boot performance.

## Important Notes

This repository is still considered to be in a "beta" stage and not recommended for production platforms at this time.

Platform testing to provide feedback is very welcome.

Before making pull requests at a minimum, run:

- `cargo clippy -- -D warnings`
- `cargo fmt --all`

## Documentation

We have "Getting Started" documentation located in this repository at `docs/*`. This documentation is actually a self
hosted book ([mdbook](https://github.com/rust-lang/mdBook)). Once you have rust downloaded as specified below, you can
download the tool with `cargo install mdbook`. Once installed, you can run `mdbook serve docs` to self host the getting
started book. The goal is to eventually host this somewhere, but for now, this works great!

## First-Time Tool Setup Instructions

The following instructions install Rust.

1. Download and install rust/cargo from [Getting Started - Rust Programming Language (rust-lang.org)](https://www.rust-lang.org/learn/get-started).
   > rustup-init installs the toolchain and utilities.

2. Make sure it's working - restart a shell after install and make sure the tools are in your path:

   \>`cargo --version`

3. Install the toolchain version specified in `rust-toolchain.toml`. The following examples assume `1.68.2` is there.
   Note that `1.68.2` is just an example, not the version that is currently used.

   Windows:

   \>`rustup toolchain install 1.68.2-x86_64-pc-windows-msvc`

   \>`rustup component add rust-src --toolchain 1.68.2-x86_64-pc-windows-msvc`

   Linux:

   \>`rustup toolchain install 1.68.2-x86_64-unknown-linux-gnu`

   \>`rustup component add rust-src --toolchain 1.68.2-x86_64-unknown-linux-gnu`

4. Install Cargo Make.

   \>`cargo install --force cargo-make`

### DXE Core Goals

1. Construction of a bare-metal "kernel" to dispatch from `DxeIpl`.
   1. Built in a basic build environment for no-std.
   2. Uses a basic output subsystem (likely legacy UART, but maybe VGA if it works in QEMU before GOP starts it).
   3. Integrable into a UEFI build as a replacement for `DxeMain` with observable debug output.
   4. No direct dependencies on PEI except PI abstracted structures.

2. Integration of Rust component builds into UEFI build system - i.e. not building in two separate enlistments and
   copying around outputs.

3. Support for CPU interrupts/exception handlers.

4. Support for rudimentary paging and heap allocation.
   1. Investigate `DxeIpl` handoff implementation.
   2. Explore how to handle dynamic allocation of different memory types (e.g. RuntimeCode/Data vs.
      BootServicesCode/Data).

## Build

**The order of arguments is important in these commands.**

### Building an Architecture-Specific DXE Core EFI Binary

The following commands build a X64 DXE core .efi binary along with the PDB file. Either a `development` or `release`
mode binary can be built. The default is the `development` mode binary.

- Development Binary (implicit): `cargo make -e ARCH=X64 build-arch`
- Development Binary (explicit): `cargo make -p development -e ARCH=X64 build-arch`
- Release Binary (explicit): `cargo make -p release -e ARCH=X64 build-arch`

The default `ARCH` value is `X64` so this command also builds the `development` binary:

- Development Binary (implicit arch and implicit target): `cargo make build-arch`

The `cargo make build-arch` command can support future architectures such as `AARCH64` by simply swapping out the
`ARCH` parameter:

- Future: `cargo make -e ARCH=AARCH64 build-arch`
  - Because `AARCH64` is currently not fully supported, this command currently returns: `error: none of the selected
    packages contains these features: aarch64`

At this time, these commands are intended to build a function DXE Core binary that can be used on an architecture
compatible platform. The `std` binary build is always meant to stay in this DXE Core repo. However, `dxe_core` is
ultimately a Rust library crate and it can be used to build a binary in another repo in the future if that allows
better integration of dependencies or integration workflows.

### Building a Host Executable DXE Core

DXE Core can also run directly on the host using the standard library in place of some firmware services in a pure
firmware environment.

- Host (Standard Library) DXE Core Build: `cargo make build-std`
  - Simpler Alias Command: `cargo make build`

#### Running the Host Executable

While the executable can be run directly out of the `/target/<debug/release>` directory, it can easily be run with the
following command:

- `cargo make run-std`

> Note: This will currently launch the DXE Core, but some additional changes are needed for it to fully operate in
> `std` mode.

## Test

- `cargo make test`

> Note: Unit tests are still being enabled in this repo. This should be very brief as they accommodate the DXE Core
> refactor.

## Coverage

A developer can easily generate coverage data with the below commands. A developer can specify a single package
to generate coverage for by adding the package name after the command.

- `cargo make coverage`
- `cargo make coverage dxe_core`

Another set of commands are available that can  generate coverage data, but is generally only used for CI.
This command runs coverage on each package individually, filtering out any results outside of the package,
and will fail if the code coverage percentage is less than 75%.

- `cargo make coverage-fail`
- `cargo make coverage-fail dxe_core`

## Notes

1. This project uses `RUSTC_BOOSTRAP=1` environment variable due to internal requirements
   1. This puts us in parity with the nightly features that exist on the toolchain targeted
   2. The `nightly` toolchain may be used in place of this

## Troubleshooting

Installing the toolchain via the rust-toolchain.toml on windows may have the following error:

```bash
INFO - error: the 'cargo.exe' binary, normally provided by the 'cargo' component, is not applicable to the '1.68.2-x86_64-pc-windows-msvc' toolchain
```

To fix this:

```bash
# Reinstall the toolchain
rustup toolchain uninstall 1.68.2-x86_64-pc-windows-msvc
rustup toolchain install 1.68.2-x86_64-pc-windows-msvc

# Add the rust-src back for the toolchain
rustup component add rust-src --toolchain 1.68.2-x86_64-pc-windows-msvc
```

## Contributing

- Review Rust Documentation Conventions at `docs/RustdocConventions.md`.
- Run unit tests and ensure all pass.
