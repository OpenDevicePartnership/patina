# Patina Internal CPU Crate

The `patina_internal_cpu` crate hosts core CPU functionality that Patina core code depends on for operations such as
cache control, interrupt dispatch, and paging management. It is compiled as part of the monolithic Patina image and
runs in `no_std` UEFI environments.

As an "internal" Patina crate, it is not intended for direct use by code outside of Patina core environments.

## Overview

- Provide a `Cpu` trait and architecture-specific `EfiCpu*` services that provide functionality used to help produce
  the UEFI CPU Architecture Protocol.
- Expose an `InterruptManager` abstraction with default fault handlers, exception context translation, and utilities
  (for example the `log_registers!` macro) that higher layers can reuse.
- Bridge Patina memory management code to the `patina_paging` and `patina_mtrr` crates so that page tables and cache
  attributes can be programmed on supported architectures.

## Key Modules

### `cpu`

`cpu::Cpu` defines the cache flush, INIT broadcast, and timer query hooks required by the UEFI CPU Architecture
Protocol.

- `EfiCpuX64` performs tasks like initializing the floating-point unit, installing a GDT, and routing cache maintenance
  operations to instructions such as `wbinvd`/`invd`.
- `EfiCpuAarch64` performs cache maintenance operations for Arm.
- `EfiCpuNull` is available for documentation and host-based unit tests that do not require actual CPU services.

All implementations derive `IntoService`, allowing the Patina storage layer to register them as `Service<dyn Cpu>`
instances during core bring-up.

### `interrupts`

`interrupts` defines the `InterruptManager` trait, handler registration (`HandlerType`). The module selects a
platform-specific backend, for x86_64, AArch64, or a null stub (which is useful in places like docs and host-based
unit tests).

Exception contexts implement `EfiSystemContextFactory` so Patina callers can forward architecture-native frames to the
UEFI-compatible `EfiSystemContext`. `InterruptManager::register_exception_handler` ultimately feeds a static `RwLock`
array, enabling late binding of either firmware callbacks or trait-based handlers.

### `paging`

`paging` contains a `create_cpu_paging` helper that wraps the `patina_paging` crate with any additional policy
required by a given architecture.

- On x86_64, `EfiCpuPagingX64` includes a `PageTable` implementation with MTRR-aware cache attribute management by
  delegating to `patina_mtrr`. Memory attribute queries merge paging attributes with current MTRR state so callers get
  a consistent view of cacheability.
- On AArch64, `EfiCpuPagingAArch64` is a thin wrapper over `AArch64PageTable`. Cache attributes are exclusively
  controlled by the page table.
- The null variant always returns `UNSUPPORTED` and is used outside UEFI execution.

## Architecture support matrix

| Target                | CPU service       | Interrupts backend | Paging adapter        |
|-----------------------|-------------------|--------------------|-----------------------|
| `x86_64`              | `EfiCpuX64`       | `InterruptsX64`    | `EfiCpuPagingX64`     |
| `aarch64`             | `EfiCpuAarch64`   | `InterruptsAArch64`| `EfiCpuPagingAArch64` |
| tests / documentation | `EfiCpuNull`      | `InterruptsNull`   | `EfiCpuPagingNull`    |

## Related documentation

- `[Memory Management](https://opendevicepartnership.github.io/patina/dxe_core/memory_management.html)` — Describes how
  some of the concepts in this crate are used by the Patina DXE Core.
