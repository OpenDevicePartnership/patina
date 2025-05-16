
# Patina

This repository hosts the Patina project - a Rust implementation of UEFI firmware.

The goal of this project is to serve as a replacement for core UEFI firmware components so they are written in Pure
Rust as opposed to Rust wrappers around core implementation still written in C.

## Background

There have been various [instances of advocacy](https://msrc-blog.microsoft.com/2019/11/07/using-rust-in-windows/) for
building system level software in [Rust](https://www.rust-lang.org/).

This repository contains a Rust [UEFI](https://uefi.org/) firmware implementation called Patina. We plan to enable an
incremental migration of today's firmware components largely written in C to Rust starting with the core. The primary
objective for this effort is to improve the security and stability of system firmware by leveraging the memory safety
offered by Rust while retaining similar boot performance.

## Important Notes

This repository is still considered to be in a "beta" stage at this time. Platform testing and integration feedback
is very welcome.

Before making pull requests at a minimum, run:

- `cargo make all`

## Performing a Release

Below is the information required to perform a release that publishes to the registry feed:

1. Review the current draft release on the github repo: [Releases](https://github.com/OpenDevicePartnership/patina/releases)
   a. If something is incorrect, update it in the draft release
   b. If you need to manually change the version, make sure you update the associated git tag value in the draft release
2. Publish the release
3. Monitor the publish release workflow that is automatically triggered on the release being published:
   [Publish Release Workflow](https://github.com/OpenDevicePartnership/patina/actions/workflows/publish-release.yml)
4. Once completed successfully, click on the  "Notify Branch Creation Step" and click the provided link to create the
   PR to update all versions in all Cargo.toml files across the repository.

## Documentation

We have "Getting Started" documentation located in this repository at `docs/*`. The latest documentation can be found
at <https://OpenDevicePartnership.github.io/patina/>, however this documentation can also be self-hosted via
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

## Build

**The order of arguments is important in these commands.**

### Building Crates

The following commands build all crates with one of our three supported targets: `x86_64-unknown-uefi`,
`aarch64-unknown-uefi`, and your host system target triple. The default compilation mode is `development`, but you can
easily switch modes with the `-p` flag.

- Development Compilation (aarch64-unknown-uefi): `cargo make build-aarch64`
- Development Compilation (x86_64-unknown-uefi): `cargo make build-x64`
- Development Compilation (host system): `cargo make build`
- Release Compilation (aarch64-unknown-uefi): `cargo make -p release build-aarch64`
- Release Compilation (x86_64-unknown-uefi): `cargo make -p release build-x64`
- Release Compilation (host system): `cargo make -p release build`

## Test

- `cargo make test`

## Coverage

A developer can easily generate coverage data with the below commands. A developer can specify a single package
to generate coverage for by adding the package name after the command.

- `cargo make coverage`
- `cargo make coverage patina_dxe_core`

Another set of commands are available that can  generate coverage data, but is generally only used for CI.
This command runs coverage on each package individually, filtering out any results outside of the package,
and will fail if the code coverage percentage is less than 75%.

- `cargo make coverage-fail`
- `cargo make coverage-fail patina_dxe_core`

## Notes

1. This project uses `RUSTC_BOOSTRAP=1` environment variable due to internal requirements
   1. This puts us in parity with the nightly features that exist on the toolchain targeted
   2. The `nightly` toolchain may be used in place of this

## Contributing

- Review Rust Documentation in the `docs` directory.
- Run unit tests and ensure all pass.
