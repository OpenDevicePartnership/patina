# Patina Rust and Toolchain Version Update Process

```admonish tip title="TL;DR"
- **Cadence:** Update at least once per quarter. Do not jump to a new stable release the day it ships.
- **Toolchain are specified in:** `rust-toolchain.toml` (stable channel) and the `rust-version` field in each crate's
  `Cargo.toml`.
- **Stable selection:** Pin the explicit stable release version (for example, `channel = "1.95.0"`) in
  `rust-toolchain.toml`.
- **Unstable features:** The pinned stable toolchain builds the unstable feature gates Patina uses via
  `RUSTC_BOOTSTRAP=1`, set in the `[env]` table of `.cargo/config.toml`. See
  [Use of Unstable Rust Features in Patina](unstable.md).
- **MSRV verification:** The `MSRV Check` workflow builds the workspace against the toolchain in the `[msrv]` table of
  `rust-toolchain.toml`. Keep that table in sync with the `rust-version` field.
- **Review:** Open the PR against `main`, add the `OpenDevicePartnership/patina-contributors` team, and leave it open for at least three full business days.
```

Rust is released on a regular six week cadence. The Rust project maintains a site with
[dates for upcoming releases](https://forge.rust-lang.org/). While there is no hard requirement to update the Rust
version used by Patina in a given timeframe, it is recommended to do so at least once per quarter. Updates to the
latest stable version should not happen immediately after the stable version release as Patina consumers may need time
to update their internal toolchains to migrate to the latest stable version.

A pull request that updates the Rust version should always be created against the `main` branch. The pull request should
include the `OpenDevicePartnership/patina-contributors` team as reviewers and remain open for at least three full
business days to ensure that stakeholders have an opportunity to review and provide feedback.

## Updating the Minimum Supported Rust Version

The Rust toolchain used in this repo is specified in `rust-toolchain.toml`. The minimum supported Rust version for the
crates in the workspace is specified in the `rust-version` field of each crate's `Cargo.toml` file. When updating the
Rust toolchain version, the minimum supported Rust version should be evaluated to determine if it also needs to be
updated.

A non-exhaustive list of reasons the minimum version might need to change includes:

1. An unstable feature has been stabilized and the corresponding `#![feature(...)]` has been removed
2. A feature introduced in the release is immediately being used in the repository

```admonish note
If the minimum supported Rust version does need to change, please add a comment explaining why. Note that
formatting and linting changes to satisfy tools like rustfmt or clippy do not alone necessitate a minimum Rust
version change.
```

A quick way to check if the minimum supported Rust version needs to change is to keep the changes made for the new
release in your workspace and then revert the Rust toolchain to the previous version. If the project fails to build,
then the minimum supported Rust version needs to be updated.

### Automated MSRV Verification

The minimum supported Rust version is also verified automatically by the `MSRV Check` workflow
(`.github/workflows/msrv-check.yml`). This workflow builds and tests the workspace against the toolchain declared in the
`[msrv]` table of `rust-toolchain.toml` (rather than the primary `[toolchain]` channel used for day-to-day development).

The `[msrv]` table pins the stable release that corresponds to the `rust-version` declared in the crates' `Cargo.toml`
files:

```toml
[msrv]
channel = "1.89.0"   # matches rust-version = "1.89"
```

The workflow runs on a daily schedule and on any pull request that modifies `rust-toolchain.toml`. This catches cases
where a change unintentionally relies on functionality newer than the declared minimum. When you raise the minimum
supported Rust version, update the `[msrv]` table's `channel` to the stable release that matches the new `rust-version`.

## Choosing a Stable Version

Patina builds against a pinned stable Rust release. The small number of unstable features the project still depends on
are compiled by setting `RUSTC_BOOTSTRAP=1` in the `[env]` table of `.cargo/config.toml`, which lets the stable
toolchain accept `#![feature(...)]` gates. See [Use of Unstable Rust Features in Patina](unstable.md) for the
feature policy and the `-Z allow-features` list that restricts which gates are permitted.

Pin the explicit stable release in `rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.95.0"
```

Use an explicit version rather than the floating `stable` channel so builds stay reproducible and each toolchain move is
an intentional, reviewable change. Available stable releases are listed on [releases.rs](https://releases.rs/)
and [the Rust release schedule](https://forge.rust-lang.org/).
