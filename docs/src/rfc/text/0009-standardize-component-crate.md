# RFC: Standardize Component Crate

The purpose of this RFC is to solicit feedback on setting a standardized layout for crates that produce components,
such that when a platform wishes to consume a component, there is a well-defined layout for where any Component,
Service, Config, Hob, etc. definitions are located and accessed from.

## Change Log

- 2025-05-20: Initial RFC created.
- 2025-05-22: Update requirements
- 2025-05-22: Add Crate Name requirements.
- 2025-05-22: Add Test Name requirements.
- 2025-05-22: Add Documentation requirements.

## Motivation

With more components being individually developed and packaged into separate crates, it becomes important to
standardize the layout of these crates to make consumption of their functionality easy for a platform. Requiring a
platform to understand how each component is laid out is burdensome and time consuming.

The overall intent is to have consistency, predictability, and cleanliness for any crate that produces a component.

## Technology Background

N/A

## Goals

Define a standard layout for a crate that produces a component(s).

## Requirements

- Predictable Data Exposure in a consistent format to help with introspection of the crate when consuming it with the
  core.
- Minimize namespae pollution by only allowing types necessary to consuming the component or service to be public.
- Predictable Crate naming to easily find and identify available components for the core.
- Documented Public API
- Standardized on-platform test naming, for easy filtering of tests via the Test Component

## Unresolved Questions

- Do we want to consider support for a prelude module defined in the top level lib.rs (or equivalent) file?
- Should we enforce that public custom types as a part of a Service interface be publically accessible in the `service`
  module or elsewhere.

## Prior Art (Existing PI C Implementation)

As it stands, all crates that produce a component may lay out their crate as they wish.

## Alternatives

N/A

## Rust Code Design

The current design requirement is a suggested starting place; It is looking for feedback and improvements. Once this
RFC is accepted, documentation will be added to the Patina mdbook that lays out the requirements defined here.

The intent is for this RFC is to define certain requirements for a public crate that produces a component as described
below.

### Crate Naming

All crates that produce a component that are published to crates.io must begin with the prefix `patina_`, allowing them
to be easily identifiable.

### On-System Test Naming

Test naming should be prefixed depending on what is being tested, to allow for easy filtering of tests on the platform.
If testing a component, the test name should be prefixed with `test_<component_name>_`. If testing a service interface, the
test name should be prefixed with `test_<service_name>_`. In both cases, CamelCase should be converted to snake_case.

```rust
#[derive(IntoComponent)]
struct MyComponent(u32);

trait MyService {
   fn do_something(&self) -> u32
}

#[patina_test]
fn test_my_component_name_for_test(...) -> Result<()> {
   Ok(())
}

#[patina_test]
fn test_my_service_name_for_test(...) -> Result<()> {
   Ok(())
}
```

### Documentation

All public modules, types, traits, etc. must be documented as specified in the existing documentation requirements.

### Standard Crate Layout

The below is a list of requirements for the crate, but it does not prevent additional modules from existing

1. Type re-exports are allowed, and can be re-exported in the same locations as would a public new type for your crate.
2. No public definitions are accessible via the top level lib.rs (or equivalent) module, only public modules.
3. `component` module: This module must always exist, and contain the publicly importable component(s) for the crate.
4. `config` module: This module may optionally exist if the component consumes configuration data that is registered
   with the platform via `.with_config` and this config is not publically accessible via `patina_sdk` or elsewhere.
5. `error` module: This module may optionally exist if a `service` module is present and the public Service's interface
   contains custom errors.
6. `hob` module: This module may optionally exist if a new guided hob type has been created for this component. The
   hob module and associated guided HOB(s) should be made public such that it can be consumed by others if the need
   arise. Any common or spec defined HOBs should be added to the associated crates (such as `mu_rust_pi`, `patina_sdk`,
   etc.) rather than this crate. HOBs may become a common interface and should thus be moved to the appropriate crate.
   If the HOB type already exists elsewhere, the crate should consume that definition instead of making their own.
7. `service` module: This module may optionally exist if the component produces a service that is not publically
   accessible via `patina_sdk` or another crate.

Below is an example repository that contains all modules defined above, and also contain submodules for each module.

``` cmd
repository
├── component/*
├── config/*
├── hob/*
├── service/*
├── component.rs
├── config.rs
├── error.rs
├── hob.rs
├── service.rs
```

## Guide-Level Explanation

N/A
