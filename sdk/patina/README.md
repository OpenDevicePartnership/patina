# Patina

The Patina crate provides shared primitives used throughout the Patina project and serves as a "Software Development
Kit (SDK)" for other Patina code.

The crate implements foundational elements used throughout Patina such as the dependency-injected component model,
typed access to interfaces like the UEFI Boot and Runtime Services, Platform Initialization (PI) Specification
content, protocol helpers, logging, performance measurement, and the Patina on-platform testing infrastructure. The
crate builds in `no_std` environments by default, can be paired with either firmware or host tooling, and configured
with optional features.

## Getting started

Add the crate to your manifest and enable the features needed by your firmware or host tooling target.

```toml
[dependencies]
patina = { version = "13.1.0", default-features = false, features = ["enable_patina_tests"] }
```

The crate is `no_std` unless `std` is selected. Tests or host utilities can enable `std` or `mockall` as needed.

## Feature Overview

| Feature | Purpose |
|---------|---------|
| `core` | Expose dispatcher-facing types such as `Storage` (enables `alloc`). |
| `alloc` | Allow allocation APIs when targeting `no_std` firmware environments with a custom allocator. |
| `std` | Link the standard library. For example, when building host utilities. |
| `doc` | Pull in items needed to build documentation. |
| `mockall` | Provide mock implementations for Boot Services and other traits (implies `std`). |
| `global_allocator` | Install the global allocator support used by Patina firmware images. |
| `enable_patina_tests` | Enable the `#[patina_test]` attribute and link-time test registration (requires at least one test). |
| `serde` | Enable serialization support for configuration and PI data structures. |
| `unstable` | Opt into experimental APIs gated behind `unstable-*` flags, including device path helpers. |
| `unstable-device-path` | Activate the current device-path parsing and construction prototypes. |

## Additional resources

- [Patina background](https://opendevicepartnership.github.io/patina/patina.html) for project context and design goals.
- [Component getting started guide](https://opendevicepartnership.github.io/patina/component/getting_started.html) for a
    walkthrough that builds on the `component` module.
- The `examples` directory contains host-run samples such as
    [`basic_hob_usage.rs`](https://github.com/OpenDevicePartnership/patina/blob/main/sdk/patina/examples/basic_hob_usage.rs)
    that demonstrate HOB parsing.
