
# Rust DXE Core

This repository contains a Pure Rust DXE Core.

## Background

There have been various [instances of advocacy](https://msrc-blog.microsoft.com/2019/11/07/using-rust-in-windows/) for
building system level software in [Rust](https://www.rust-lang.org/).

This repository contains a Rust DXE Core [UEFI](https://uefi.org/) firmware implementation. We plan to enable an
incremental migration of today's firmware components largely written in C to Rust starting with the core. The primary
objective for this effort is to improve the security and stability of system firmware by leveraging the memory safety
offered by Rust while retaining similar boot performance.

## Important Notes

This repository is still considered to be in a "beta" stage and not recommended for production platforms at this time.

Platform testing and integration feedback is very welcome.

Before making pull requests at a minimum, run:

- `cargo clippy -- -D warnings`
- `cargo fmt --all`

## Documentation

We have "Getting Started" documentation located in this repository at `docs/*`. The latest documentation can be found
at <https://OpenDevicePartnership.github.io/uefi-dxe-core/>, however this documentation can also be self-hosted via
([mdbook](https://github.com/rust-lang/mdBook)). Once you all dependencies installed as specified below, you can run
`mdbook serve docs` to self host the getting started book.

You can also generate API documentation for the project using `cargo make doc`. This will eventually be hosted on
docs.rs once we begin uploading to crates.io.

## First-Time Tool Setup Instructions

The following instructions install Rust.

1. Download and install rust/cargo from [Getting Started - Rust Programming Language (rust-lang.org)](https://www.rust-lang.org/learn/get-started).
   > `rustup-init` installs the toolchain and utilities.

2. Make sure it's working - restart a shell after install and make sure the tools are in your path:

   \>`cargo --version`

3. Install toolchain specified in `rust-toolchain.toml`

   \>`rustup toolchain install`

4. While the specific toolchains and components specified in `[toolchain]` section of the `rust-toolchain.toml` are
automatically installed with `rustup toolchain install`, the tools in the `[tools]` section, such as `cargo-make`
are not. At a minimum, you should download `cargo-make` and `cargo-tarpaulin`, however it is suggested that you
download all tools in the `[tools]` section of the `rust-toolchain.toml`.

### DXE Core Goals

1. Construction of a bare-metal "kernel (DXE core)" to dispatch from `DxeIpl`.
   1. Log output over a basic subsystem such as serial I/O.
   2. Integrable into a UEFI build as a replacement for `DxeMain` with observable debug output.
   3. Greater than 80% unit test coverage across all code compiled into the DXE Core.
   4. A "monolithic" DXE environment that encapsulates functionality distributed across separate EFI modules today.
      This is accomplished with an internal dispatcher to the binary that executes individual components linked during
      platform integration and given to the common Rust DXE Core interface when the platform builds its version of
      Rust DXE Core.
   5. In addition to internal Rust component dispatch, UEFI driver dispatch - FVs and FFS files in the firmware ROM.
   6. No direct dependencies on PEI except PI abstracted structures.

2. Support for CPU interrupts/exception handlers.

3. Support for paging and heap allocation.

4. UEFI memory protections that implement best known practices and drive memory protections in UEFI firmware forward.

## Build

**The order of arguments is important in these commands.**

### Building uefi-dxe-core Crates

The following commands build all crates with one of our three supported targets: `x86_64-unknown-uefi`,
`aarch64-unknown-uefi`, and your host system target triple. The default compilation mode is `development`, but you can
easily switch modes with the `-p` flag.

- Development Compilation (aarch64-unknown-uefi): `cargo make build-aarch64`
- Development Compilation (x86_64-unknown-uefi): `cargo make build-x64`
- Development Compilation (host system): `cargo make build`
- Release Compilation (aarch64-unknown-uefi): `cargo make -p release build-aarch64`
- Release Compilation (x86_64-unknown-uefi): `cargo make -p release build-x64`
- Release Compilation (host system): `cargo make -p release build`

### Building a Host Executable DXE Core

DXE Core can also run directly on the host using the standard library in place of some firmware services in a pure
firmware environment.

- Host (Standard Library) DXE Core Build: `cargo make build-bin`

#### Running the Host Executable

While the executable can be run directly out of the `/target/<debug/release>` directory, it can easily be run with the
following command:

- `cargo make run-bin`

> Note: This will currently launch the DXE Core, but some additional changes are needed for it to fully operate in
> `std` mode.

## Test

- `cargo make test`

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

## Contributing

- Review Rust Documentation in the `docs` directory.
- Run unit tests and ensure all pass.
