# RFC: Build on Stable Rust with `RUSTC_BOOTSTRAP`

This RFC proposes building Patina with a pinned stable Rust toolchain and `RUSTC_BOOTSTRAP=1` instead of the nightly
toolchain. The goal is to lock onto a chosen stable release, move off of it only on an intentional schedule, and make it
clearer which toolchain downstream projects should build against.

## Change Log

- 2026-06-25: Initial draft of RFC.
- 2026-07-02: RFC is accepted and numbered as RFC 0030.

## Motivation

Patina depends on a small number of unstable Rust features ([`c_variadic`](https://github.com/OpenDevicePartnership/patina/issues/806),
[`allocator_api`](https://github.com/OpenDevicePartnership/patina/issues/805), and [`coverage_attribute`](https://github.com/OpenDevicePartnership/patina/issues/804)
at the time of writing). To compile that code, Patina currently builds against a nightly toolchain pinned in
`rust-toolchain.toml`.

Building on nightly has caused recurring friction:

- Toolchain instability.
  - A given nightly can be broken or incompatible with tooling Patina relies on.
  - For example, a recent nightly was incompatible with the rust-analyzer plugin, which required searching for a
    different nightly that was both compatible with the plugin and still supported the unstable features Patina uses.
- Unintended feature "drift".
  - Nightly continuously incorporates new language and library functionality.
  - It is possible to unintentionally start depending on a capability that is not present in the stable release Patina
    intends to target, and that dependency may go unnoticed until later.
- Downstream confusion.
  - Downstream projects build and release production firmware on stable toolchains. When the upstream toolchain moves
  between nightlies, it is harder for those projects to determine which stable toolchain they should align to.

`RUSTC_BOOTSTRAP=1` makes a stable `rustc` accept `#![feature(...)]` gates and other unstable flags. Setting it lets
Patina pin a specific stable release, build the same unstable features it uses today, and stay on that release until an
intentional update. This is effectively a return to an earlier approach that Patina moved away from. The
[Prior Decision](#prior-decision) section covers why that move happened and why it is being revisited now.

## Technology Background

### `RUSTC_BOOTSTRAP`

[`RUSTC_BOOTSTRAP`](https://doc.rust-lang.org/beta/unstable-book/compiler-environment-variables/RUSTC_BOOTSTRAP.html) is
an environment variable used by the Rust project's own bootstrap process to build the compiler and standard library,
which themselves rely on unstable features, using a stable or beta `rustc`. When set to `1`, it instructs `rustc` to
permit unstable feature gates and flags that are otherwise only available on nightly.

Because it exists primarily for the compiler's internal bootstrap, it carries no stability guarantee for external use.
Its observable effect (enabling unstable gates on a non-nightly toolchain) has been stable in practice for a long time
and is widely used in the ecosystem, but Patina should treat it as a deliberate, temporary measure rather than a
supported configuration.

### Nightly versus stable selection

Patina's [Rust and toolchain version update process](../../dev/rust_version_update_process.md) describes
selecting a nightly using the "Branched from master" date for a target stable release. That indirection exists only
because Patina builds on nightly while wanting behavior close to a specific stable release. Pinning a stable
release directly removes that indirection. The toolchain Patina builds against is the same release downstream
consumers ship on.

### Current configuration

Two files, both synchronized from the
[patina-devops](https://github.com/OpenDevicePartnership/patina-devops) repository, are relevant:

- `rust-toolchain.toml` pins the toolchain channel. It currently selects a nightly (for example,
  `channel = "nightly-2026-04-11"`).
- `.cargo/config.toml` sets environment variables for the build through its `[env]` table.

The change this RFC proposes has two parts: switch the channel in `rust-toolchain.toml` from a nightly to a pinned
stable release, and set `RUSTC_BOOTSTRAP = "1"` in the `[env]` table of `.cargo/config.toml` so the stable toolchain
accepts the unstable feature gates Patina uses.

## Goals

1. Build Patina against a pinned stable Rust release rather than nightly.
2. Move to a new stable release only as an intentional, reviewed change, following the existing toolchain update process.
3. Keep building the unstable features Patina depends on today, without changing the long-term plan to remove them.
4. Make it clear to downstream consumers which stable toolchain to align to.
5. Keep the configuration change small and contained to the existing toolchain configuration files.

## Requirements

1. **Stable channel.** `rust-toolchain.toml` must pin an explicit stable release (initially `channel = "1.95.0"`)
   rather than a nightly or a floating `stable` channel. An explicit version keeps builds reproducible and makes the
   update an intentional, reviewable change. We also want consumers to have time to comment on the toolchain update
   process.
2. **Bootstrap enablement.** The build must set `RUSTC_BOOTSTRAP=1` so the pinned stable toolchain accepts the unstable
   feature gates Patina uses. This is set in the `[env]` table of `.cargo/config.toml`.
3. **Update process.** Changing the currently specified stable release must follow the existing
   [Rust and toolchain version update process](../../dev/rust_version_update_process.md), including the review window and
   reviewers described there.
4. **No change to feature policy.** This RFC does not add or remove any unstable feature. Unstable feature
   usage continues to be governed by the [unstable feature process](../../dev/unstable.md) and tracked under the
   [`rustc-feature-gate`](https://github.com/OpenDevicePartnership/patina/issues?q=is%3Aissue+state%3Aopen+label%3Arustc-feature-gate)
   label.

## Implementation Notes

The change is to set `rust-toolchain.toml`'s `channel` to a given stable release (initially `1.95.0` at time of writing)
and set `RUSTC_BOOTSTRAP=1` in the `[env]` table of `.cargo/config.toml`. Subsequent stable versions are chosen per the
existing update process.

Two follow-on details for the update process documentation:

- The "Choosing a Nightly Version" guidance used now essentially becomes "choose a stable release" and no longer needs
  the "Branched from master" date lookup.
- The MSRV check (`.github/workflows/msrv-check.yml` and the `[msrv]` table in `rust-toolchain.toml`) remains, but its
  `channel` is set to a stable version tag rather than the nightly that corresponds to a stable `rust-version`.

## Unresolved Questions

- None.

## Prior Decision

Patina originally used `RUSTC_BOOTSTRAP` to enable unstable features. Patina later moved to nightly out of a concern
that `RUSTC_BOOTSTRAP` is intended for the Rust compiler's internal bootstrap process and is not a supported external
configuration.

Revisiting that decision is being done now because the move to nightly traded one concern for several recurring operational
problems (see [Motivation](#motivation)) without a corresponding benefit. Nightly is not what downstream firmware ships
on, and Patina does not need any nightly-only capability beyond the ability to compile the specific unstable feature
gates it has already accepted through the unstable feature process. Pinning a stable release plus `RUSTC_BOOTSTRAP=1`
keeps those feature gates working while giving Patina control over when the toolchain moves.

This proposal does not change the long-term direction, which remains to depend on no unstable features. That effort is
tracked through the open issues under the
[`rustc-feature-gate`](https://github.com/OpenDevicePartnership/patina/issues?q=is%3Aissue+state%3Aopen+label%3Arustc-feature-gate)
label and the parent issue [#826](https://github.com/OpenDevicePartnership/patina/issues/826). Once those features are
removed, Patina can drop `RUSTC_BOOTSTRAP` and build on stable using the same configuration that pins the toolchain
while the variable is in place.

## Alternatives

- **Stay on nightly.**
  - Retains the toolchain instability, feature drift, and downstream alignment problems described in the motivation.
    Not recommended.
- **Pin a floating `stable` channel.**
  - Picks up each new stable release automatically.
  - This reintroduces a moving target and weakens reproducibility, so an explicit pinned version is preferred.
- **Remove all unstable features first, then move to stable without `RUSTC_BOOTSTRAP`.**
  - This is the eventual end state, but it is gated on stabilizing or replacing every feature currently in use.
  - Moving to stable plus `RUSTC_BOOTSTRAP` now captures the toolchain-control benefits without blocking on that
    longer effort, and converges to the same place once the features are removed.
