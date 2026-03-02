# RFC: Downstream Build Options

This RFC intends to define the process for ensuring Patina crates are built with a standard set of build options,
from RUSTFLAGS to compiler and linker flags.

## Change Log

- 2026-02-06: Initial RFC created.

## Motivation

Patina is heavily dependent on its build options, from stack cookies, image section size, debug asserts enabled in
debug builds, etc. However, because Patina crates are not binary crates, we cannot control any build options
for a platform. This can lead to a lot of noise and problems with having to document everything that Patina needs,
platforms having issues when they don't use exactly that, and time spent debugging issues that should never have
occurred in the first place.

To avoid all of this and put platforms in the best position, Patina should do its best to ensure the correct set of
toolchain options are being used.

## Technology Background

The edk2 [tools_def.template](https://github.com/tianocore/edk2/blob/HEAD/BaseTools/Conf/tools_def.template) is the
inspiration for the idea of having platforms use a canonical set of build options. edk2 achieves this through a
custom maintained set of tools that platforms must use to build with.

Information about Rust options can be found [in the Rust docs](https://doc.rust-lang.org/cargo/reference/config.html),
amongst other places.

## Goals

1. Create an easy to consume toolchain configuration for platforms
2. Ensure the correct toolchain configuration is used when building Patina

## Requirements

1. Do not maintain custom build tools
2. Do not allow Patina to be built with incorrect build options
3. Do not put burden on the platform to figure out the right build options

## Unresolved Questions

- Are any of the alternatives preferable to the proposed approach? Many of these could work, but have cons that have
  led to a different recommendation.
- Are there any alternatives not listed that are more attractive?
- If this approach is taken, should Patina just advertise that `config.toml` should be copied or should it create
  a template repo for platforms to fork?
- Should Patina check specific toolchain flags are set/unset in `build.rs` or should it only check the version?

## Prior Art (Existing PI C Implementation)

As noted above, edk2's tools_def.template, as used by its BaseTools, is the prior art. It gives edk2 full control
(by default, it is overrideable per platform and per module) to what it is built with. It requires extensive custom
build tools to maintain and is not a separateable concept from edk2's BaseTools.

## Rust Code Design

The proposed solution consists of two parts:

### Configuration

Patina will have a `config.toml` file with standard [profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)
defined for different Patina uses cases, i.e. `dev`, `release`, and `test`. Custom profiles may be defined in the
future; this is outside the scope of this RFC. Each profile will define the set of `RUSTFLAGS`, compiler flags,
linker flags, and any other relevant toolchain options. This will be done in conjunction with `target.\<triple\>`
sections that are global to all profiles.

Patina will document that platforms must copy this `config.toml` to their platform bin repo to build Patina. This
puts some work on the platform to do the copy and place it correctly. It also is not guaranteed that platforms will
follow this step. But this can be easily documented and validated (see below).

### Validation

Patina will have a `build.rs` file for each core (e.g. one for patina_dxe_core, one for a future patina_mm_core, etc.)
that validates the expected profile version used by the platform. This will not check all toolchain options, but only
enforce that the version matches what Patina expects. This is intended to be a guide rail, not an absolute requirement;
platforms may choose to implement non-recommended toolchain options.

`build.rs` files are automatically built when a crate is being built, including as a dependency. This allows us to
automatically enforce our config version expectation. See the
[cargo build scripts documentation](https://doc.rust-lang.org/cargo/reference/build-scripts.html) for more details.

An example to ensure that Patina is using config version 5:

```rust
// config.toml
[env]
PATINA_CONFIG_VERSION = "5"

[profile.dev]
// whatever flags this version sets
```

```rust
// build.rs
use std::{env, process};

fn main() {
    let version = env::var_os("PATINA_CONFIG_VERSION")
        .unwrap_or_default();

    if version != "5" {
        eprintln!("error: Incorrect PATINA_CONFIG_VERSION, expected version "5", got version {version}");
        eprintln!("Use Patina's latest config.toml.");
        process::exit(1);
    }

    // Only rerun this when the rustflags or the config version changes
    println!("cargo:rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");
    println!("cargo:rerun-if-env-changed=PATINA_CONFIG_VERSION");
}
```

This allows us to fail to build if we don't have the proper config version. It is not much overhead because Cargo will
only rerun this when the `RUSTFLAGS` change or the `PATINA_CONFIG_VERSION` changes.

> **Note:** build.rs is very powerful and flexible. It can be used to set `RUSTFLAGS` as well as check them, but that
> is not proposed here because it only applies to the crate being built, not dependencies. This is merely for
> validation, not application.

## Guide-Level Explanation

Patina uses a `config.toml` at `patina/.config/config.toml`. Platforms must copy this file to the same location in
their platform binary repo, e.g. `cp patina/.config/config.toml plat_bin/.config/config.toml`. This will ensure the
platform is built with the correct toolchain configuration. Patina enforces a set of toolchain configurations it
must be built with in `patina/build.rs`. The full list is documented in the
[Patina docs](https://opendevicepartnership.github.io/patina/).

## Alternatives

### Maintain a Cargo Subcommand

In order to enforce platforms use the correct options, we could maintain our own `Cargo subcommand` or other custom
build tooling.

Upsides:

- Complete control of what platform sets, as long as they use our tool

Downsides:

- Heavy maintenance burden in owning our own tools
- Higher cost to entry to learn custom tools

### Maintain Custom Target Triples

The relevant toolchain configuration can all be set in custom targets.

Upsides:

- If Patina only can be built with the custom targets, we completely control toolchain configuration.

Downsides:

- Maintenance burden of maintaining our own targets
- Not compatible with existing UEFI crates

### Have Submodule or External Dependency

A repo could be set up with a `.config/config.toml` in it and a custom tool could copy that to the platform's `.config`
directory.

Upsides:

- Could guarantee a platform does the copy of the config file.

Downsides:

- Have to maintain custom script.
- Introduce submodules/external dependencies to Patina which it currently avoids
- Complicated, easy to envision it failing under different development scenarios

### Various Non-Working Ideas

Various other ideas were explored that seemed promising but are not able to work in the Cargo environment. Briefly,
these are:

- Creating Patina profiles that can be enforced
  - Rejected because profiles cannot be shared except in the `config.toml` copy described in the proposed solution
- Having a `config.toml` that lives in Patina and can apply to consumers without copying
  - Rejected because Cargo only reads `config.toml` from very specific locations
