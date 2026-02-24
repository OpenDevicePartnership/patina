# Toolchain Configuration

Patina relies on platform bin repos copying its `.cargo/config.toml` to properly set toolchain configuration. This
document focuses on the process to update and maintain Patina's toolchain configuration.

## Background

Patina must recommend a set of toolchain configuration options for platforms to use to have the best experience. With
different options, various features may not work as intended (e.g. if `-C force-unwind-tables` is not used, the
stack walk in a debugger will be truncated), performance or binary size may be worse, or failures could occur.

EDK II achieves this same goal by maintaining custom build tools. One of Patina's philosophies is to use existing
tools wherever possible. In pursuit of this goal, Patina has a `.cargo/config.toml` file that is used when building
Patina in CI. It [is recommended](../integrate/patina_dxe_core_requirements.md#35-configtoml-usage) that platforms
copy this `.cargo/config.toml` to match Patina's toolchain configuration exactly. Patina enforces the same version
is being used in a `build.rs` file.

>**Note:** Platforms still have the flexibility to change toolchain configuration as needed in this setup, they just
> need to ensure the config version is the same as what Patina is expecting. Platforms choosing to diverge from the
> known good Patina config accept the risk of unexpected behavior stemming from these modifications.

## Updating Configuration

There are some simple rules that must be followed when updating Patina's configuration.

1. Consider the change. Is is appropriate for all platforms? All architectures?
2. Add (or remove) the configuration option to `patina/.cargo/config.toml`. See [the rules](#configuration-adding-rules).
3. Add a comment (multiline okay) above the option describing what this option does and why.
4. Increase `PATINA_CONFIG_VERSION` by one. All config updates, no matter how trivial, must update the version.
5. Update the `PATINA_CONFIG_VERSION` defined in `patina/patina_dxe_core/build.rs` by one. This must match the
   `PATINA_CONFIG_VERSION` updated in step 4.

### Configuration Adding Rules

The following is a set of rules to follow when adding new toolchain configuration options.

#### Prefer Cargo Options

Per [Rust documentation](https://doc.rust-lang.org/cargo/reference/config.html#buildrustflags) all `RUSTFLAGS` that
`Cargo` itself manages in `profile` settings (e.g. `lto`, `debug`, etc.) should be set in the relevant `profile`
section, not directly in `RUSTFLAGS`. See [the profile docs](https://doc.rust-lang.org/cargo/reference/profiles.html)
for which settings are managed there.

#### Put RUSTFLAGS in target.\<triple\>.rustflags Section

Per [Rust documentation](https://doc.rust-lang.org/cargo/reference/config.html#buildrustflags), there are four mutually
exclusive ways to pass RUSTFLAGS:

```text
1. CARGO_ENCODED_RUSTFLAGS environment variable.
2. RUSTFLAGS environment variable.
3. All matching target.<triple>.rustflags and target.<cfg>.rustflags config entries joined together.
4. build.rustflags config value.
```

Patina only uses the third option. We do not directly set environment variables as this is brittle and can conflict
with many different scenarios. We prefer using the target triple sections because they have precedence over general
`[Build]` sections. Do not put RUSTFLAGS in a `[Build]` section, it will be unused.
