# Platform Integration and Crate Patching

This guide explains how platforms consume Patina libraries and test local changes. It is written for developers familiar
with EDK II source trees and Git submodules who are new to Rust packaging.

Two terms frequently used in this context help distinguish how Rust code is distributed and compiled:

- **Package:** A bundle of source code described by a `Cargo.toml` manifest. It contains one or more crates, with at
  most one library crate and any number of binary crates. Cargo resolves dependencies and publishes releases at the
  package level.
- **Crate:** A unit of Rust code compiled by the Rust compiler. A library crate provides reusable functionality and a
  binary crate produces an executable, such as a platform's `.efi` image.

Rust developers also commonly call published library packages "crates." This guide uses **package** for dependency
versions, registry releases, and Cargo patches, and **crate** for compilation units. See the Rust Book's
[Packages and Crates](https://doc.rust-lang.org/book/ch07-01-packages-and-crates.html) for details.

## How a Platform Consumes Patina

Patina distributes its Rust libraries through [Cargo](https://doc.rust-lang.org/cargo/), Rust's official build tool and
package manager. Rather than checking out Rust library code as a Git submodule, a platform declares the libraries and
corresponding versions it needs. Cargo retrieves those libraries from a
[registry](https://doc.rust-lang.org/cargo/reference/registries.html), such as [crates.io](https://crates.io), where
Patina publishes its releases. crates.io is the default and official Rust crate registry.

The platform keeps its Rust code in a "[workspace](https://doc.rust-lang.org/cargo/reference/workspaces.html)", with
dependencies (Rust libraries) declared in a
[`Cargo.toml` manifest](https://doc.rust-lang.org/cargo/reference/manifest.html). Cargo downloads the dependency source,
and the Rust toolchain compiles it using the official Rust
[UEFI target](https://doc.rust-lang.org/rustc/platform-support/unknown-uefi.html) with the platform's Rust code to
produce a DXE Core `.efi` image. The EDK II build then includes that PE32 image in firmware through the platform's Flash
Description File (FDF).

Registry packages contain source code and metadata, not prebuilt firmware binaries. Patina is completely open source.
When using crates, you retain access to that source and can
[test local changes with Cargo patches](#testing-a-local-patina-change). Using the standard/official Rust workflow
described above **does not** impede your ability to make local modifications or access source code.

For a working example of platform integration with Patina, see the
[patina-dxe-core-qemu Cargo.toml manifest](https://github.com/OpenDevicePartnership/patina-dxe-core-qemu/blob/main/Cargo.toml),
which selects Patina libraries and builds a DXE Core `.efi` image.
[patina-qemu](https://github.com/OpenDevicePartnership/patina-qemu) packages that image into platform firmware image
(the QEMU ROM file).

For the rationale behind why the Rust code repo is separate from the C platform firmware repo, see
[How Does This All Work with EDK II?](./dxe_core.md#how-does-this-all-work-with-edk-ii) and
[Why Isn't Patina Built as Part of the EDK II Build?](./dxe_core.md#why-isnt-patina-built-as-part-of-the-edk-ii-build).

### Where the Library Code Resides

[Patina's primary Git repository](https://github.com/OpenDevicePartnership/patina) manages multiple Rust packages
together in a Cargo [workspace](https://doc.rust-lang.org/cargo/reference/workspaces.html). Each package's manifest
declares its name, version, dependencies, and build options.

In this case, both the repository and _one_ of its packages are both named `patina`. Within the
[patina](https://github.com/OpenDevicePartnership/patina) repository, `sdk/patina/Cargo.toml` defines the `patina`
package, whose library crate is also named `patina`. That particular crate is also commonly referred to as the "Patina
SDK" since it provides a lot of base definitions and utility code used by many other packages. The repository contains
other packages too: `patina_dxe_core/Cargo.toml` defines the `patina_dxe_core` package, component package directories
are under `components/`, and others. All of these paths are relative to the repository root as part of the overall
"workspace" (in the Cargo sense) in the repository.

Patina publishes individual packages, including [patina](https://crates.io/crates/patina) and
[patina_dxe_core](https://crates.io/crates/patina_dxe_core), to crates.io. Keep in mind that a dependency on the
published `patina` package selects that versioned package, not the entire repository.

## Building Patina and Platform Firmware In One Repo/Build Command

A common question is how to build Patina and platform firmware together in one repository and with a single build
command. This section directly takes on that question while explaining where key recommendations sit in the Patina
integration process.

Key Recommendations:

1. The Rust code should be built in a separate source repo tree from C code.
2. The Rust code should use cargo for building and managing dependencies.
3. The Rust code should use crate dependencies with cargo for managing external dependencies.
4. The Rust code should use Rust standard practices and simple methods for building code against Rust targets to produce
   .efi binaries.
5. The platform firmware build should include Rust code in the form of binaries (where the binaries are built from a
   pure Rust repository).

Those points help prevent custom/esoteric build setups that mix Rust and C code in non-standard ways, ensuring a clear
separation of responsibilities, a straightforward integration path, and the greatest compatibility with standard, high
quality, off-the-shelf Rust tooling.

The open-source patina-dxe-core-qemu and patina-qemu repositories are maintained separately. The patina-dxe-core-qemu
repository publishes the .efi binaries built there as a
[GitHub release](https://github.com/OpenDevicePartnership/patina-dxe-core-qemu/releases) that is picked up during the
build in the patina-qemu repository. You can use a similar binary sharing approach for your platform's Rust and firmware
integration repos, however, you can also build the Rust binary at the same time as the platform firmware with source
included in the same repo.

An approach to do so is to keep the platform's Rust repository as a submodule of its firmware repository and invoke both
builds from one script. The build boundary stays the same: Cargo builds the Rust binaries and EDK II packages them into
firmware flash images.

The following layout is just for illustration. `platform_rust_repo` is the platform's Rust integration repository (Rust
code that integrates Patina libraries to build the .efi image). That repo consumes Patina through Cargo crate
dependencies (not submodules).

```text
platform_firmware/                     # Platform Firmware Repository
|-- build script                       # Builds Rust first, then EDK II firmware
|-- PlatformPkg/
|   |-- C source and build metadata
|   `-- Platform.fdf                   # Includes the Patina DXE Core PE32 image
`-- platform_rust_repo/                # Platform Rust Repository (Git submodule)
    |-- Cargo.toml                     # Versioned Patina dependencies
    |-- Cargo.lock
    |-- src/                           # Platform Rust code and configuration
    `-- target/<uefi-target>/release/
        `-- platform_dxe_core.efi      # Rust build output consumed by the FDF
```

1. Create a separate, pure-Rust repository that builds the platform's `.efi` binaries using Cargo, standard Rust UEFI
   targets, and versioned package dependencies.
   - This is equivalent to the role the
     [patina-dxe-core-qemu](https://github.com/OpenDevicePartnership/patina-dxe-core-qemu) repository serves.
2. Add that repository as a submodule of the platform firmware repository.
   - This is equivalent to the role the [patina-qemu](https://github.com/OpenDevicePartnership/patina-qemu) repository
     serves.
3. Have the platform build script invoke the Rust repository's Cargo build command before starting the EDK II build.
4. Point the FDF's DXE Core PE32 file path to the generated `.efi` image in the submodule's target directory. Use the
   actual output path for your target, profile, and binary name.

This arrangement keeps Rust compilation and dependency management in Cargo while exposing both builds through the
platform's usual build command. Note that the entire process is supported with existing tooling available in Git
(submodules), Rust/Cargo (ability to build .efi binaries), and EDK II (ability to include PE32 images directly).

## Why Use Registry Packages Instead of Submodules?

Git submodules pin repository commits. Registry dependencies instead declare acceptable package versions, allowing Cargo
to resolve requirements across both direct and **transitive dependencies** (libraries required by other libraries).
**This is significant** and not doing so will almost certainly lead to dependency conflicts over time. Cargo also
combines requested package features.

For example, a platform manifest might contain:

```toml
[dependencies]
patina = "24"
patina_dxe_core = "24"
```

These are illustrative requirements, not a recommendation to use a particular release. `"24"` accepts versions from
`24.0.0` up to, but not including, `25.0.0`. It does not mean exactly `24.0.0`. For more background, see the selected
release's integration requirements before upgrading. Cargo documents the details in
[Version Requirement Syntax](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#version-requirement-syntax).

`Cargo.toml` declares acceptable versions. `Cargo.lock` records the resolved package versions, sources, and registry
package checksums. It is recommended to commit the platform binary workspace's `Cargo.lock` file so developers and CI
share that resolution for reproducible builds. See
[Cargo.toml vs Cargo.lock](https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html).

It is recommended to allow compatible minor and patch updates in Cargo.toml dependencies and prevent automatic
major-version upgrades as they need to be reviewed for breaking changes. Registry releases let Cargo select compatible
versions (using semantic versioning) without requiring consumers to coordinate repository commits or checkout layouts.
The dependency graph also supports supply-chain checks, including [human vetting](https://mozilla.github.io/cargo-vet/),
[Rust security advisories](https://rustsec.org/), and [license checks](https://embarkstudios.github.io/cargo-deny/).

When choosing what dependency versions to allow, keep in mind that many Rust crates treat their minor version as the
indicator of breaking changes until they reach a v1.0.0 release, so check the crate's release notes and versioning
policy when choosing compatible versions. Patina crates use the major version to indicate breaking changes.

Git and path dependencies in Cargo.toml (where Cargo would access the submodule using Git, not you) is also supported
(but highly discouraged). When doing this, Cargo.lock records resolved commits. However, unlike a registry's collection
of crate releases, these approaches select source checkouts whose revisions and local layouts you must coordinate. This
has been proven to be very error prone in the past.

## Testing a Local Patina Change

You can use Cargo's [`[patch]` mechanism](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html) to
test changes without replacing the platform's normal versioned dependency declarations. This is an officially supported
mechanism used for testing local changes supported by Cargo, not unique to Patina. A Cargo patch substitutes a package
source.

### Set Up the Checkouts

Clone a `patina` repository where the local Patina changes are being made, for example:

```text
work/
|-- patina/
|   |-- Cargo.toml
|   |-- sdk/patina/Cargo.toml
|   `-- patina_dxe_core/Cargo.toml
`-- platform_rust_repo/
    |-- Cargo.toml
    `-- Cargo.lock
```

Make local changes based on a compatible Patina release version (same major version) with the platform's dependency
requirements.

Add overrides to the platform workspace root `Cargo.toml` in a `[patch.crates-io]` section:

```toml
[dependencies]
patina = "24"
patina_dxe_core = "24"

[patch.crates-io]
patina = { path = "../patina/sdk/patina" }
patina_dxe_core = { path = "../patina/patina_dxe_core" }
```

Note that if the platform uses `[workspace.dependencies]`, this patch table might need to be applied in the workspace
root Cargo.toml file (if workspace members inherit dependencies from the workspace Cargo.toml file).

**Each crate being patched must have its own entry in the `[patch.crates-io]` section.** For simplicity, it is common to
add an entry for all the Patina crates being used by the platform. The rule of thumb for patching Patina crates is that
if you patch one, you should patch all related Patina crates to ensure consistency and avoid conflicts.

Each path is relative to the manifest containing the patch and must point to the directory containing that package's
`Cargo.toml`. Keep in mind that the [Patina repository](https://github.com/OpenDevicePartnership/patina) root is a
[workspace](https://doc.rust-lang.org/cargo/reference/workspaces.html), not the directory for the `patina` package and
the directory for each patch entry will be to the package's own directory containing its `Cargo.toml`.

### Check the Whole Dependency Graph

A patch applies to matching dependencies throughout the consuming workspace's graph, including transitive uses of that
package.

You can run `cargo tree` in the platform Rust workspace to check which sources Cargo selected. A local path beside a
package identifies a local dependency, for example:

```text
qemu_dxe_core v4.0.2 (C:/src/patina-dxe-core-qemu)
├── log v0.4.34
├── patina v24.0.0 (C:/src/patina/sdk/patina)
```

### Troubleshooting When a Patch Is Not Used

An entry in `[patch]` adds a candidate to dependency resolution, but it does not bypass version constraints. These are
common checks when a patch fails:

| Check                 | What to Do                                                                                                                                               |
| :-------------------- | :------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Version requirements  | The version of the crate patch must match the crate version in Cargo.lock.                                                                               |
| Lockfile              | If the lockfile retains a registry version, use `cargo update -p <package>` to reevaluate that package, then inspect the lockfile diff and `cargo tree`. |
| Patch location        | Put the patch table in the consuming workspace's root manifest, not in a dependency or member manifest.                                                  |
| Original source       | `[patch.crates-io]` patches crates.io. For another registry or a Git dependency, use its registry name or source URL as the patch key.                   |
| Package path and name | Point to the package's manifest directory and verify its declared name.                                                                                  |

For a developer-only override, Cargo also supports patch tables through local configuration. See the
[`[patch]` reference](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html#the-patch-section) and
[configuration path rules](https://doc.rust-lang.org/cargo/reference/config.html#config-relative-paths). Do not commit
developer system-specific paths as a shared release configuration. After adopting an upstream release containing the
change, remove the local overrides, update the affected lockfile entries, and confirm the graph uses that release.

### Example: Patching Multiple Packages

For a checkout at `C:/src/patina`, the following entries cover common Patina packages. Include only those needed for
your changes and dependency graph.

```toml
[patch.crates-io]
patina_acpi={path="C:/src/patina/components/patina_acpi"}
patina={path="C:/src/patina/sdk/patina"}
patina_macro={path="C:/src/patina/sdk/patina_macro"}
patina_test={path="C:/src/patina/components/patina_test"}
patina_adv_logger={path="C:/src/patina/components/patina_adv_logger"}
patina_dxe_core={path="C:/src/patina/patina_dxe_core"}
patina_debugger={path="C:/src/patina/core/patina_debugger"}
patina_internal_cpu={path="C:/src/patina/core/patina_internal_cpu"}
patina_stacktrace={path="C:/src/patina/core/patina_stacktrace"}
patina_ffs={path="C:/src/patina/sdk/patina_ffs"}
patina_internal_core={path="C:/src/patina/core/patina_internal_core"}
patina_ffs_extractors={path="C:/src/patina/sdk/patina_ffs_extractors"}
patina_samples={path="C:/src/patina/components/patina_samples"}
patina_smbios={path="C:/src/patina/components/patina_smbios"}
patina_mm={path="C:/src/patina/components/patina_mm"}
patina_performance={path="C:/src/patina/components/patina_performance"}
```
