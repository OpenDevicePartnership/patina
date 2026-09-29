# RFC: Consolidate common core code into a single internal library crate

This RFC proposes creating an internal crate (`core/patina_internal_core`) for
code shared by core implementations. This single crate would remove overhead and
simplify changing internal code and interfaces. This will become increasingly
important with the introduction of the Standalone MM core.

This RFC builds on the crate organization and naming conventions established by
[RFC: Categorizing and Renaming Patina Crates for Consistency](./0007-patina-crates-naming-and-categorization.md).

## Change Log

- 2026-06-26: Initial RFC created.
- 2026-06-29: Renames proposed crate to `patina_internal_core`, and opted to leave CPU
              crate independent for now.
- 2026-07-13: Fixed incorrect RFC number

## Motivation

The Patina cores will share a meaningful amount of internal-only infrastructure
(collections, CPU/interrupt/paging support, dependency expression parsing,
performance, etc.). Today that infrastructure is split across several
small `patina_internal_*` crates that were each carved out individually.

The use of these specific-purpose internal crates creates overhead and limits
development in a few ways.

1. **Purpose-defined internal crates force unnecessary external rigidity**.
Internal crates, by definition, are not intended to host any externally consumable
interfaces. As such, they are free to be refactored, updated, and redesigned. However,
publishing crates with named purposes, such as `patina_internal_cpu`, creates an
external contract around the organization of this code.

2. **Some internal crates are very small**.
The main benefit of splitting these crates up is to reduce build and test times, but
the internal crates are all currently small enough that this benefit is not meaningful.
   - `patina_internal_depex` is a single `lib.rs` file.
   - `patina_internal_collections` is only 5 files.
   - `patina_internal_cpu` is the largest and is still only 17 files.

3. **Adding common code is burdensome**.
As other cores are developed, Patina should strive to make as much code common as
possible. The burden of having to create a new published crate to do this for a
new category of code is an unnecessary engineering cost.

## Technology Background

The internal crates in question have evolved and been refactored a few times. The original
intention for some of the implementations was to be common and consumable in the other
environments where their organization originated. However, once these crates were made
officially internal—explicitly not for external consumption and not stable—the rationale
for their specific organization was lost. The structure of these crates at the time of this
RFC is as follows.

```text
CORE\PATINA_INTERNAL_COLLECTIONS
│   Cargo.toml
│   README.md
├───benches
│       bench_add.rs
│       bench_delete.rs
│       bench_search.rs
└───src
        bst.rs
        lib.rs
        node.rs
        rbt.rs
        sorted_slice.rs

CORE\PATINA_INTERNAL_CPU
│   Cargo.toml
│   README.md
└───src
    │   cpu.rs
    │   interrupts.rs
    │   lib.rs
    │   paging.rs
    ├───cpu
    │   │   aarch64.rs
    │   │   stub.rs
    │   │   x64.rs
    │   ├───aarch64
    │   │       cache.rs
    │   │       cpu.rs
    │   └───x64
    │           cpu.rs
    │           gdt.rs
    ├───interrupts
    │   │   aarch64.rs
    │   │   exception_handling.rs
    │   │   stub.rs
    │   │   x64.rs
    │   ├───aarch64
    │   │       exception_handler.asm
    │   │       gic_manager.rs
    │   │       interrupt_manager.rs
    │   └───x64
    │           idt.rs
    │           interrupt_handler.asm
    │           interrupt_manager.rs
    └───paging
            aarch64.rs
            null.rs
            x64.rs

CORE\PATINA_INTERNAL_DEPEX
│   Cargo.toml
│   README.md
└───src
        lib.rs
```

## Goals

1. Reduce the overhead of having to add new crates for internal-only use.
2. Improve development ergonomics for leveraging shared code.
3. Prevent unnecessary boundaries for internal-only code.
4. Maintain the existing "internal" semantics for shared code.

## Requirements

1. A common internal library crate should be created to host most internal
   shared code.
2. This common crate must only be consumed in other official patina core crates,
   as required by naming convention.
3. This design must not preclude the use of purpose-built patina_internal_* crates
   in the future, if a purpose arises.
4. The common crate must allow gating on the `alloc` feature to support all patina
   core environments.

## Unresolved Questions

- **MM Supervisor coupling.** The exact subset of `patina_internal_core` consumed
  by the MM Supervisor is out of scope here and will be driven by the MM
  Supervisor work.
- **CPU Crate Independence.** The CPU crate is used in more limited environments
  such as the supervisor.This RFC leaves this as a future decision.

## Prior Art

[RFC 0007](0007-patina-crates-naming-and-categorization.md) established the
`patina_internal_` convention for internal-only crates and is the basis for the
naming used here. [RFC 0008](0008-consolidate-sdk-crates-into-patina-sdk-modules.md)
set a precedent within Patina for consolidating multiple small crates into a
single crate organized by module hierarchy; this RFC applies the same reasoning
to the internal core crates.

## Alternatives

1. **Keep the status quo (multiple `patina_internal_*` crates).** Rejected
   because sharing internal code with a new core requires publishing a crate, and
   the existing granularity does not provide enough cohesion/reuse benefit to
   justify that ongoing cost.
2. **Per-core duplication of shared code.** Rejected because it increases the
   maintenance burden and risks divergence of behavior across cores.
3. **Move the shared code into the SDK.** Rejected because the SDK is for
   *public* interfaces. This code is internal implementation detail that should
   not be part of the public API surface.

## Rust Code Design

Each existing `patina_internal_*` crate becomes a top-level module in
`patina_internal_core`, named after the crate's purpose with the
`patina_internal_` prefix dropped. The existing crates then stop being
published and are deprecated.

```rust
// core/patina_internal_core/src/lib.rs
#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod collections; // was patina_internal_collections
pub mod depex;       // was patina_internal_depex
```

Each former crate root (`lib.rs`) becomes the module root for its area, keeping
its existing submodule layout unchanged underneath it:

```text
core/patina_internal_core/src/
    lib.rs
    collections.rs
    collections/
        bst.rs
        node.rs
        rbt.rs
        sorted_slice.rs
    depex.rs
    ... future expansion ...
```

For now, `patina_internal_cpu` will be left independent because of it's unique compilation
requirements, use in low level environments, and increased scope compared to the other crates.

In the future, new `patina_internal_` crates may still be introduced if the need arises
for either large or logically contained code.

## Guide-Level Explanation

All internal shared code must live in a `patina_internal` crate. For smaller
modules, `patina_internal_core` serves as the common library. If a given internal
interface meets the following criteria:

1. Logically contained
2. Sufficiently large
3. Requires independent compilation

then a separate `patina_internal` crate may be introduced for that purpose, with the
approval of the Patina maintainers.
