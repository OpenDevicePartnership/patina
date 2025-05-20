# RFC: Standardize Component Crate Layout

The purpose of this RFC is to solicit feedback on setting a standardized layout for crates to produce components, so
that when a platform wishes to consume a component, there is a well-defined layout for where any Component, Service,
Config, Hob, etc. definitions are located and accesses from.

## Change Log

- YYYY-MM-DD: Initial RFC created.

## Motivation

With more components being individually developed and packaged into separate crates, it becomes important to
standardize the layout of these crates to make consumption of their functionality easy for the platform. Forcing a
platform to understand how each component is laid out is burdomsome and time consuming.

## Technology Background

N/A

## Goals

Define a standard layout for a crate that produces a component(s)

## Requirements

A standard layout for a crate that produces a component(s) is defined

## Unresolved Questions

- Do we want to consider support for a prelude module defined in the top level lib.rs (or equivalent) file?
- Should we enforce that public custom types as a part of a Service interface be publically accessible in the `service`
  module or elsewhere.

## Prior Art (Existing PI C Implementation)

As it stands, all crates that produce a component may lay out their crate as they wish.

## Alternatives

N/A

## Rust Code Design

The current design requirement is suggested, and looking for feedback and improvements. Once this RFC is accepted,
documentation will be added to the patina mdbook that lays out the requirements defined here.

The intent is for this RFC is to define certain modules that must exist and be accessable via the root of the library
crate. It does not restrict the possibility of other modules existing, only that certain modules must exist at the root
of they crate given certain circumstances as defined below:

1. No public definitions are accessible via the top level lib.rs (or equivalent) module, only public modules.
2. `component` module: This module must always exist, and contain the publicly importable component(s) for the crate.
3. `config` module: This module may optionally exist if the component consumes configuration data that is registered
   with the platform via `.with_config` and this config is not publically accessible via `patina_sdk` or elsewhere.
4. `service` module: This module may optionally exist if the component produces a service that is not publically
   via `patina_sdk` or another crate.
5. `hob` module: This module may optionally exist only if the hob is expected to be consumable by other public
   components and is not publically accessible via `patina_sdk` or elsewhere. If a Hob is only intended for this
   specific component(s) defined in the crate, then these may be kept private.
6. `error` module: This module may optionally exist if a `service` module is present and the public Service's interface
   contains custom errors.

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
├── service.rs
```

## Guide-Level Explanation

N/A
