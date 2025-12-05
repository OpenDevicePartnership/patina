# RFC: Code Coverage Using llvm-cov

This RFC proposes using the [cargo-llvm-cov crate](https://crates.io/crates/cargo-llvm-cov) for code coverage instead
of [cargo-tarpaulin](https://crates.io/crates/cargo-tarpaulin), the current solution.

## Change Log

- 2025-07-25: Initial draft of RFC

## Motivation

Currently, Patina uses `cargo-tarpaulin` to calculate code coverage from unit and integration tests. `cargo-tarpaulin`
has been observed to miscalculate line coverage, even when using the `llvm` backend, which is purported to be the
same as `cargo-llvm-cov`. When running the same configuration and code under test with `cargo-llvm-cov`, line coverage
has been observed to be much more accurate.

We desire to have accurate code coverage results as such, this RFC proposes moving to `cargo-llvm-cov`.

## Technology Background

[Code Coverage](https://en.wikipedia.org/wiki/Code_coverage) is a standard concept in software engineering.

## Goals

1. Provide accurate code coverage of our unit and integration tests.

## Requirements

1. **Line Coverage Results** - We must show a line coverage percentage to track how much of Patina has been tested.
2. **Maintain Test Framework** - We must maintain our existing build and test frameworks.

## Unresolved Questions

- What is the best configuration for getting accurate coverage results? Small scale testing and research has given this
  profile:

  ```rust
  [profile.test]
  opt-level = 0
  debug = true
  debug-assertions = true
  overflow-checks = true
  lto = false
  incremental = false
  codegen-units = 1
  ```

- Does `cargo-llvm-cov` bring in any nightly features that we are not okay with? The
  [documentation](https://crates.io/crates/cargo-llvm-cov) notes that the following require nightly/are unstable:

  - `llvm-tools-preview` is required to execute `cargo llvm-cov`, but it is not required to run the tests under a
    nightly toolchain
  - [#[coverage(off)] attribute](https://github.com/rust-lang/rust/issues/84605), required to disable code coverage on
    code, including test code
  - branch coverage (optional)
  - doc test coverage (optional)

- `cargo-llvm-cov` is not as widely used as `cargo-tarpaulin`. Does it meet our standards? Recent versions of
  `cargo-llvm-cov` have ~80k downloads, `cargo-tarpaulin` has ~1.7m. Both appear to have a single maintainer who is the
  primary contributors.

- Are there performance differences? Research indicates that `cargo-llvm-cov` is faster, but has not been verified.

## Prior Art

As noted, the current code coverage solution used `cargo-tarpaulin`. `cargo-tarpaulin` does not support branch, region,
or function coverage; `cargo-llvm-cov` does.

## Alternatives

- **Stay with `cargo-tarpaulin`** - This is not recommended due to the issues seen with it calculating code coverage.
- **Move to `grcov`** - grcov is a coverage tool developed by Mozilla, however it is not Rust specific and
  experimentation has shown that it also has issues with calculating code coverage in Patina.
- **Use other coverage tools** - There may be better options, either other Rust crates or non-Rust tools, but research
  would need to be done to verify.

## Rust Code Design

This is mostly a CI change. The only Rust code changes would be:

- Adding a `profile.test` in `Cargo.toml` as noted above to disable optimizations for tests
- Adding the `#[coverage(off)]` attribute to test modules and non-testable functions
- Updating documentation, pipelines, makefiles, etc. to run `cargo-llvm-cov` instead of `cargo-tarpaulin`

## Guide-Level Explanation

This proposal does not require a change from most consumers of Patina except to download `cargo-llvm-cov`. Most of this
should be abstracted behind `cargo make coverage`.

However, by default, `cargo-llvm-cov` calculates code coverage over unit tests. In order to only calculate coverage
over actual production firmware code, test authors must do the following:

```rust
#![cfg_attr(coverage, feature(coverage_attribute))] // if using stable
#![cfg_attr(coverage_nightly, feature(coverage_attribute))] // if using nightly

#[cfg_attr(coverage, coverage(off))] // if using stable
#[cfg_attr(coverage_nightly, coverage(off))] // if using nightly
mod tests {
  ...
}
```
