# Dependency Management

The Rust DXE Core is designed to be a monolithic binary, meaning that diverse sets of Rust functionality and the core
are compiled together. This allows for more checks to directly be performed against the overall dependency graph that
composes that Rust DXE Core but also leads to a relatively larger number of dependencies in that graph. This document
describes some of the best practices in place for managing these dependencies.

## Dependency Linting

[cargo-deny](https://embarkstudios.github.io/cargo-deny/) ([repo](https://github.com/EmbarkStudios/cargo-deny)) is a
cargo plugin that lints the dependencies of a Rust project. It can be used to enforce policies on dependencies, such as
banning certain crates or versions, or ensuring that all dependencies are up-to-date. The Rust DXE Core uses
`cargo-deny` to enforce the following policies:

- **Allowed Licenses**: Only certain licenses are allowed to be used in the Rust DXE Core and its dependencies. This is
  done to ensure that the project remains free of dependencies that have been deemed unsuitable.
- **Allowed Sources**: Only crates from expected sources are allowed to be used in the Rust DXE Core.
- **Banned crates**: Certain crates are banned from being used in the Rust DXE Core. This is done to ensure that the
  project remains free of dependencies that have been deemed unsuitable. Crates may be banned only for certain versions
  or for all versions.
- **Security Advisories**: All crates and their respective versions must not have any security advisories. This is
  currently cheked against the [RustSec advisory database](https://rustsec.org/).

`cargo-deny` is run in CI and can also be run locally with the `cargo make deny` command. This command will encapsulate
any flags that are required to run `cargo-deny` with the correct configuration for the Rust DXE Core.

The configuration for `cargo-deny` is stored in the `deny.toml` file in the root of the repository.
