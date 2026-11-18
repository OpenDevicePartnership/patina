# Patina MM User Core

A Rust implementation of the MM User Core for Standalone MM.

## Overview

This crate provides the user-level core of a Standalone MM environment on x64 systems where:

- Code runs in user mode (Ring 3) after the MM Supervisor has initialized.
- The supervisor has already loaded every MM driver image.
- User code requests supervisor services through syscalls.

## Documentation

The Patina book describes the user core in [User Core](https://opendevicepartnership.github.io/patina/mm/user_core.html)
and how it works with the supervisor, starting at
[Theory and Operation](https://opendevicepartnership.github.io/patina/mm/operation.html).

## Memory Model

The supervisor owns the page tables, memory attributes, and page allocation. The user core's global allocator is a
pool allocator that takes pages from the supervisor through syscalls, and `MmAllocatePages` requests pages directly.
All user-level allocations use the runtime services data memory type.

## Building a PE/COFF Binary

This crate is a library. A platform binary links it and is built for the `x86_64-unknown-uefi` target (install it
with `rustup target add x86_64-unknown-uefi`) using the toolchain pinned in `rust-toolchain.toml`.

The platform binary exports `user_core_main`:

```rust
#[cfg_attr(target_os = "uefi", unsafe(export_name = "user_core_main"))]
pub extern "efiapi" fn mm_user_main(op_code: u64, arg1: u64, arg2: u64) -> u64 {
    USER_CORE.entry_point_worker::<MyMmPlatform>(op_code, arg1, arg2)
}
```

The image entry point is the `efi_main` shim in `src/entry_point.asm`. The supervisor enters it in Ring 3; the shim
calls `user_core_main` and returns the status to the supervisor through a call gate.

## Entry Point Model

The entry point takes one of three operation codes:

1. Start User Core (0), once on the BSP: initializes the core, discovers the MM drivers from the HOB list, builds the
   MM System Table, and registers the core MMI handlers. Components and drivers are not run yet.
2. User Request (1), on the BSP: dispatches the MMI to the registered handlers. The first
   `MM_DISPATCH_EVENT_GROUP_GUID` MMI, sent after the supervisor has rendezvoused the APs, registers and runs the
   platform's Patina components (with configs unlocked, then locked) and then dispatches the MM drivers. Its handler
   retires before running any of them, so this happens once.
3. User AP Procedure (2), on the APs in parallel: runs the given procedure. It is not synchronized with the BSP or
   other APs, so it should not touch global state or shared data structures.

## Usage

### Basic Platform Implementation

```rust
#![no_std]
#![no_main]

use core::panic::PanicInfo;
use patina_mm_user_core::{
    MmUserCore,
    component_dispatcher::{Add, Component, MmComponentInfo},
};

// Static instance; no heap allocation required.
static USER_CORE: MmUserCore = MmUserCore::new();

// The platform provides its components via an `MmComponentInfo` type.
struct MyMmPlatform;

impl MmComponentInfo for MyMmPlatform {
    fn components(mut add: Add<Component>) {
        // add.component(...);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop { core::hint::spin_loop(); }
}

#[unsafe(export_name = "user_core_main")]
pub extern "efiapi" fn mm_user_main(op_code: u64, arg1: u64, arg2: u64) -> u64 {
    USER_CORE.entry_point_worker::<MyMmPlatform>(op_code, arg1, arg2)
}
```

## License

Copyright (c) Microsoft Corporation.

SPDX-License-Identifier: Apache-2.0
