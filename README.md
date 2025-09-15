
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

```shell
cargo make all
```

## Performing a Release

Below is the information required to perform a release that publishes to the registry feed:

1. Review the current draft release on the github repo: [Releases](https://github.com/OpenDevicePartnership/patina/releases)
   1. If something is incorrect, update it in the draft release
   2. If you need to manually change the version, make sure you update the associated git tag value in the draft release
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
docs.rs once we begin uploading to crates.io. You can have the documentation opened in your browser by running
`cargo make doc-open`.

## First-Time Tool Setup Instructions

1. Follow the steps outlined by [Getting Started - Rust Programming Language (rust-lang.org)](https://www.rust-lang.org/learn/get-started)
to install, update (if needed), and test cargo/rust.

2. The `[toolchain]` section of the [rust-toolchain.toml](https://github.com/OpenDevicePartnership/patina/blob/HEAD/rust-toolchain.toml)
file contains the tools necessary to compile and can be installed through rustup.

   ```shell
   rustup toolchain install
   ```

3. The `[tools]` section of the [rust-toolchain.toml](https://github.com/OpenDevicePartnership/patina/blob/HEAD/rust-toolchain.toml)
file contains tools to support commands such as `cargo make coverage` and must be installed manually.  A local build
does not need them all, but at a minimum, cargo-make and cargo-llvm-cov should be installed.

   ```shell
   cargo install cargo-make
   cargo install cargo-llvm-cov
   ```

4. Another optional tool that has proven useful for speeding up the build process is 'cargo-binstall', located on
[GitHub](https://github.com/cargo-bins/cargo-binstall).  See the readme.md file in that repository for installation and
usage instructions.

## Build

All of the Patina crates can be compiled in one of 3 supported targets; aarch64, x64, or native.

```shell
cargo make build-aarch64
   - or -
cargo make build-x64
   - or -
cargo make build
```

By default, the make compiles a developer build, but development or release can be indicated by using the "-p" flag

```shell
cargo make -p development build-aarch64
   - or -
cargo make -p release build-aarch64
```

## Test

Use the test command to invoke a test build and execute all unit tests.

```shell
cargo make test
```

## Coverage

The coverage command will generate test coverage data for all crates in the project.  To target a single crate, the
name can be added to the command line.

```shell
cargo make coverage
   - or -
cargo make coverage dxe_core
```

## Benchmarks

Various crates have benchmarks setup that can be executed using the `cargo make bench` command. Any arguments provided
will be passed along to the bench command:

```cmd
cargo make bench -p patina_sdk
cargo make bench -p patina_sdk --bench bench_component
cargo make bench -p patina_sdk --bench bench_component -- with_component
```

Benchmarks utilize the [criterion](https://crates.io/crates/criterion) benchmarking library, so if new benchmarks are
to be added, they should follow that documentation. Benchmarks can be added to any crate to test performance by
following the same layout as existing benchmarks, and adding the benchmark to the appropriate crate's Cargo.toml file.

## Notes

- This project uses a makefile that sets the "RUSTC_BOOTSTRAP=1" environment variable due to internal requirements which
puts us in parity with the nightly features that exist on the toolchain targeted.  The "nightly" toolchain may be used
in place of this.

## High-Level Patina Roadmap

Patina's upcoming work falls into three main categories:

1. **Stabilization** - Bug fixes, performance improvements, and feature completion for existing functionality. This
   work is focused on ensuring that everything in Patina's main branch is stable and ready for production use. This is
   the primary focus for the next several months.
2. **Expansion** - Expansion of Patina's capabilities and this falls into two sub-categories:
   1. **Component Growth** - Adding new components to Patina to replace equivalent C-based UEFI components. As new
      components are made available, it is expected that platforms adopting Patina will incrementally remove their
      C-based UEFI components as the equivalent Rust-based functionality is now available in their Patina build.
   2. **MM Core Support** - Similar to the DXE Core support in Patina today, adding a Patina Standalone MM Core such
      that the Management Mode (MM) core environment is written in Rust and a larger portion of MM drivers can be
      ported to Patina MM components.
3. **Ecosystem Integration** - This is broken down into two sub-categories:
    1. **Firmware Ecosystem** - Patina adopters working together to ensure Patina works in their platforms and use
        cases. This includes things like platform bring-up, integration with existing firmware components, and
        validation against various hardware configurations.
    2. **Rust Ecosystem** - Engaging with the broader Rust community to ensure Patina aligns with Rust's best
        practices and leverages the latest Rust features. This includes things like contributing to Rust libraries,
        participating in Rust forums, and collaborating with other Rust projects.

### Status

1. **Stabilization** - In Progress
2. **Expansion**
    1. **Component Growth** - In Progress
    2. **MM Core Support** - Planned
3. **Ecosystem Integration**
    1. **Firmware Ecosystem** - In Progress
    2. **Rust Ecosystem** - In Progress

Patina welcomes and encourages community contributions to help accelerate progress in these focus areas. We also value
your ideas and feedback on additional priorities that matter to the community.

## Contributing

- Review Rust Documentation in the [/docs](https://github.com/OpenDevicePartnership/patina/blob/HEAD/docs/src/introduction.md)
directory.
- Run unit tests and ensure all pass.
