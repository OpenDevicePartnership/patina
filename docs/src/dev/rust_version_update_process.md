# Patina Rust and Toolchain Version Update Process

```admonish tip title="TL;DR"
- **Cadence:** Update at least once per quarter. Do not jump to a new stable release the day it ships.
- **Toolchain are specified in:** `rust-toolchain.toml` (nightly channel) and the `rust-version` field in each crate's `Cargo.toml`.
- **Nightly selection:** Use the "Branched from master" date for the target stable release from [releases.rs](https://releases.rs/).
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

## Choosing a Nightly Version

Patina currently builds against nightly Rust since the project depends on a small number of unstable features. Unless
there is a compelling reason to update the nightly version, it is recommended to continue using the same nightly version
until the next stable release. The project typically takes the nightly version listed as the "Branched from master" date
for the release on [releases.rs](https://releases.rs/). For example, the [1.97.0](https://releases.rs/docs/1.97.0/)
release has a "branched from master on" date of "22 May, 2026".

````admonish tip
If you need to find the commit that the stable release was branched from, you can use `git` to find the commit hash
working within the <https://github.com/rust-lang/rust> repository. For example, `git merge-base main 1.95.0` returns
`67aec36df76a7b67b71a8ee47684467e16f1847e`. `git show 67aec36df76a7b67b71a8ee47684467e16f1847e` returns:

```text
commit 67aec36df76a7b67b71a8ee47684467e16f1847e
Merge: 3a70d0349fa 70da8044517
Author: bors <bors@rust-lang.org>
Date:   Fri Feb 27 22:04:20 2026 +0000
```

[releases.rs - 1.95.0](https://releases.rs/docs/1.95.0/) also lists the "Branched from master on" date as
"27 Feb, 2026".
````
