# RFC: Consolidate SDK Crates into `patina_sdk` and `patina_sdk_macro` as Modules

This RFC proposes consolidating the existing crates inside the Patina `sdk`
directory into the `patina_sdk` and `patina_sdk_macro` crates, organized by
modules. No functional changes to the modules are intended.

## Change Log

- 2025-05-20: Initial draft of the RFC.
- 2025-05-22: Final updates during FCP
  - `patina_test_macro` will be merged into `patina_sdk_macro`, but
  `patina_sdk_macro` will remain a separate crate from `patina_sdk` because
  procedural macros must be defined in a separate crate.
  - Rename `0000-consolidate-sdk-crates-into-patina-sdk-modules` to
    `0008-consolidate-sdk-crates-into-patina-sdk-modules`

## Motivation

Following the creation of the Patina monorepo, and in line with the [Crates
Renaming RFC](0007-patina-crates-naming-and-categorization.md), there is now a
scope to consolidate the crates located in the `sdk` directory. There was
[consensus](https://github.com/OpenDevicePartnership/patina/pull/408#discussion_r2089478969)
on merging some of these crates (e.g., runtime services, UEFI protocol) into
`patina_sdk`, organized via a module hierarchy. This RFC formalizes and
continues that discussion.

## Technology Background

This RFC does not affect any specific technologies or alter the functionality of
the modules.

## Goals

1. Reduce the number of standalone crates present in the `sdk` directory.
2. Reduce the number of standalone crates published to [crates.io](https://crates.io).
3. Preserve the current crate or module boundaries.
4. Consolidate the crates into `patina_sdk` and `patina_sdk_macro`.
5. Leverage Rust's default [module visibility
   rules](https://doc.rust-lang.org/reference/visibility-and-privacy.html),
   using `pub(crate)` and similar mechanisms to control visibility both inside
   and outside the `patina_sdk` crate.
6. Consolidate the following crates, which are already being re-exported in
   `patina_sdk` via `pub use`, as modules within the crate:
   - `patina_boot_services`
   - `patina_driver_binding`
   - `patina_runtime_services`
   - `patina_tpl_mutex`
   - `patina_uefi_protocol`
7. Consolidate the following crates into `patina_sdk_macro`, as modules
   within the crate:
   - `patina_test_macro`

## Requirements

1. Preserve the current crate or module boundaries.
2. Do not introduce any changes to the logic or functionality of the modules,
   other than updating import paths.
3. Update all references in the codebase to reflect the new module paths after
   reorganization.

## Unresolved Questions

## Prior Art: Crate/Module Hierarchy Before and After Consolidation

Below is a comparison of the current and proposed crate/module hierarchies. Some
of the crates being consolidated were previously re-exported in `patina_sdk` and
are now integrated directly as modules.

```text
Before                             After
C:\r\patina\sdk>                    C:\r\patina\sdk>
├── patina_boot_services            │
│   ├── allocation.rs               │
│   ├── boxed.rs                    │
│   ├── c_ptr.rs                    │
│   ├── event.rs                    │
│   ├── global_allocator.rs         │
│   ├── lib.rs                      │
│   ├── protocol_handler.rs         │
│   └── tpl.rs                      │
├── patina_driver_binding           │
│   └── lib.rs                      │
├── patina_runtime_services         │
│   ├── lib.rs                      │
│   └── variable_services.rs        │
├── patina_sdk                      ├── patina_sdk
│   ├── base.rs                     │   ├── base.rs
│   │                               │   ├── boot_services/*
│   │                               │   ├── boot_services.rs    (was `patina_boot_services/lib.rs`)
│   ├── component/*                 │   ├── component/*
│   ├── component.rs                │   ├── component.rs
│   │                               │   ├── driver_binding/*
│   │                               │   ├── driver_binding.rs   (was `patina_driver_binding/lib.rs`)
│   ├── efi_types.rs                │   ├── efi_types.rs
│   ├── error.rs                    │   ├── error.rs
│   ├── guid.rs                     │   ├── guid.rs
│   ├── lib.rs                      │   ├── lib.rs              (`pub use` becomes `pub mod`)
│   ├── log/*                       │   ├── log/*
│   ├── log.rs                      │   ├── log.rs              (was reexporting `boot_services/driver_binding/runtime_services/uefi_protocol`)
│   ├── macros.rs                   │   ├── macros.rs
│   │                               │   ├── runtime_services/*
│   │                               │   ├── runtime_services.rs (was `patina_runtime_services/lib.rs`)
│   ├── serial/*                    │   ├── serial/*
│   └── serial.rs                   │   ├── serial.rs
│                                   │   ├── tpl_mutex/*
│                                   │   ├── tpl_mutex.rs        (was `patina_tpl_mutex/lib.rs`)
│                                   │   ├── test/*
│                                   │   ├── test.rs             (was `patina_test/lib.rs`)
│                                   │   ├── uefi_protocol/*
│                                   │   └── uefi_protocol.rs    (was `patina_uefi_protocol/lib.rs`)
├── patina_sdk_macro                └── patina_sdk_macro
│   ├── component_macro.rs              ├── component_macro.rs
│   ├── hob_macro.rs                    ├── hob_macro.rs
│   ├── lib.rs                          ├── lib.rs
│   └── service_macro.rs                ├── service_macro.rs
│                                       ├── test_macro/*
│                                       └── test_macro.rs       (was `patina_test_macro/lib.rs`)
├── patina_test
│   ├── __private_api.rs
│   └── lib.rs
├── patina_test_macro
│   └── lib.rs
├── patina_tpl_mutex
│   └── lib.rs
└── patina_uefi_protocol
    ├── lib.rs
    └── status_code.rs
```

## Alternatives

- Keep the current crate organization
  - This option is not recommended, as it does not achieve the goals outlined in
    this RFC.
