# Patina MM User Core

A Rust implementation of the MM User Core for Standalone MM.

## Overview

This crate provides the user-level core of a Standalone MM environment on x64 systems where:

- Code runs in user mode (Ring 3) after the MM Supervisor has initialized.
- The supervisor owns the page tables.
- The supervisor has already loaded every MM driver image.
- User code requests supervisor services through syscalls.

## Memory Model

The supervisor controls the final memory model (page tables and memory attributes) and owns page allocation and
freeing.

The user core runs a pool allocator for small allocations and requests pages from the supervisor for larger ones.
All user-level allocations use the runtime services data memory type.

## Building a PE/COFF Binary

### Prerequisites

1. Install the Rust UEFI target:

   ```bash
   rustup target add x86_64-unknown-uefi
   ```

2. Use the toolchain pinned in the repository's `rust-toolchain.toml`.

### Build

This crate is a library. It is linked into a platform-provided MM User Core binary, which is built for the
`x86_64-unknown-uefi` target to produce the PE/COFF image.

### Entry Point

The platform binary exports `user_core_main`, matching the EDK II convention:

```rust
#[cfg_attr(target_os = "uefi", unsafe(export_name = "user_core_main"))]
pub extern "efiapi" fn mm_user_main(op_code: u64, arg1: u64, arg2: u64) -> u64 {
    USER_CORE.entry_point_worker::<MyMmPlatform>(op_code, arg1, arg2)
}
```

The image entry point is the `efi_main` shim in `src/entry_point.asm`. The supervisor enters it in Ring 3 after it
has set up the MM environment (page tables, memory attributes, and the secure policy gate). The shim calls
`user_core_main` and returns the status to the supervisor through a call gate.

## Architecture

### Entry Point Model

The entry point takes one of three operation codes:

1. Start User Core (0):
   - Runs once, on the BSP.
   - Initializes the user core (pool allocator, protocol database, MMI handler database, and so on).
   - Discovers the MM drivers from the HOB list. They are dispatched later, when the MM dispatch event MMI arrives.
   - Retains the platform's component preparation callback and the persistent user HOB list address without calling
     platform registration callbacks, constructing components, initializing parameters, or parsing component HOBs.

2. User Request (1):
   - Runs on the BSP.
   - Dispatches the MMI to the registered user-level handlers.
   - On `MM_DISPATCH_EVENT_GROUP_GUID`, prepares and runs the platform's Patina components, then dispatches MM
     drivers. The supervisor has rendezvoused registered APs before forwarding this request.
   - Component preparation includes registration, construction, parameter initialization, and HOB parsing.
     Execution runs both configuration phases before drivers evaluate their dependencies.
   - The dispatch handler retires before invoking any component code; the retained preparation is consumed once.

3. User AP Procedure (2):
   - Runs on the APs, in parallel.
   - The procedure should not access global state or shared data structures, since it is not synchronized with the
     BSP or the other APs.

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

// Static instance - no heap allocation required
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
