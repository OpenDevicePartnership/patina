# Patina MM Supervisor Core

A Rust implementation of the MM (Management Mode) Supervisor Core for standalone MM on x64. It expects:

- Page tables already set up by the pre-MM phase
- All images loaded and ready to execute
- The BSP (Bootstrap Processor) to serve incoming requests
- The APs (Application Processors) to wait in a holding pen, checking a mailbox for work

## Documentation

The Patina book describes how the supervisor and the user core work, starting at
[Theory and Operation](https://opendevicepartnership.github.io/patina/mm/operation.html). The
[MM Supervisor Requirements](https://opendevicepartnership.github.io/patina/integrate/patina_mm_supervisor_requirements.html)
list what a platform must provide.

## Memory Model

The supervisor does not use heap allocation. `MmSupervisorCore` is built by a `const fn`, so the whole supervisor can be
instantiated as a `static`.

The per-core arrays (CPU slots and AP mailboxes) are sized from the core count the MP Information HOB reports. The BSP
allocates them from supervisor-only MMRAM pages during initialization, so one supervisor binary supports any core count.

## Building a PE/COFF Binary

### Prerequisites

1. Install the Rust UEFI target:

   ```bash
   rustup target add x86_64-unknown-uefi
   ```

### Build

This crate is a library. A platform supervisor binary crate links it in, supplies the `PlatformInfo` implementation
and the entry point, and is built for `x86_64-unknown-uefi` to produce the PE/COFF image. Enable this crate's
`save_state_amd` feature from that binary crate when the platform uses the AMD64 save state layout (see
[Selecting the Save State Layout](#selecting-the-save-state-layout)).

### Entry Point

The image entry point is `efi_main`, a shim in `src/entry_point.asm` that jumps to `rust_main`. The platform
binary provides `rust_main`, which forwards to the supervisor core:

```rust
#[unsafe(export_name = "rust_main")]
pub extern "efiapi" fn mm_supervisor_main(cpu_index: usize, hob_list: *const c_void) {
    // SAFETY: invoked once per core by the MM environment with a valid HOB list.
    unsafe { SUPERVISOR.entry_point(cpu_index, hob_list) };
}
```

The MM IPL (Initial Program Loader) calls this entry point on every processor after:

1. Loading the supervisor image into MMRAM
2. Setting up page tables
3. Constructing the HOB list with MMRAM ranges

## Architecture

### Entry Point Model

Every core runs the entry point on each MMI.

On the first entry:

1. BSP (Bootstrap Processor):
   - Identified by the BSP flag in the `IA32_APIC_BASE` MSR
   - Validates the HOB list and performs one-time initialization
   - Starts the user core in Ring 3
   - Runs its own per-core initialization and returns

2. APs (Application Processors):
   - All other CPUs
   - Wait for BSP initialization to complete, then run per-core initialization and return

On later entries the BSP serves the pending request while the APs wait in the holding pen and poll their mailboxes for
commands. Each core returns once the BSP releases the APs.

During initialization the BSP validates the MM Init image allocation from the HOB list (inside MMRAM, page-aligned, and
not overlapping the Core) and records it. On the first SMI after initialization, before serving the request, it checks
again that the image lies inside MMRAM and frees its pages. The HOB list is not read again after initialization.

### Mailbox System

- Each CPU index, the BSP's included, has a cache-line aligned mailbox
- The BSP posts commands to an AP's mailbox, and the AP posts its response to the same mailbox
- At the end of each MMI the BSP releases the APs through per-CPU semaphores and waits for each to acknowledge

## Usage

### Basic Platform Implementation

```rust
#![no_std]
#![no_main]

use core::{ffi::c_void, panic::PanicInfo};
use patina_mm_supervisor::*;

struct MyPlatform;

impl PlatformInfo for MyPlatform {}

// Static instance - no heap allocation required.
static SUPERVISOR: MmSupervisorCore<MyPlatform> = MmSupervisorCore::new();

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop { core::hint::spin_loop(); }
}

#[unsafe(export_name = "rust_main")]
pub extern "efiapi" fn mm_supervisor_main(cpu_index: usize, hob_list: *const c_void) {
    // SAFETY: invoked once per core by the MM environment with a valid HOB list.
    unsafe { SUPERVISOR.entry_point(cpu_index, hob_list) };
}
```

### Registering Request Handlers

Platforms add supervisor MMI handlers by implementing `PlatformInfo::mmi_handlers`. The core searches its built-in
handlers first, then the platform handlers, and runs the first one whose GUID matches.

```rust
use patina_mm_supervisor::*;
use patina::standard::efi;

fn my_handler(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    // Handle the request
    efi::Status::SUCCESS
}

static MY_HANDLERS: &[SupervisorMmiHandler] = &[SupervisorMmiHandler {
    name: "MyHandler",
    handler_guid: patina::BinaryGuid::from_string("12345678-1234-5678-1234-567800000000"),
    handle: my_handler,
}];

impl PlatformInfo for MyPlatform {
    fn mmi_handlers() -> &'static [SupervisorMmiHandler] {
        MY_HANDLERS
    }
}
```

### Selecting the Save State Layout

Intel and AMD lay out the SMRAM save state differently, and `EFI_MM_CPU_PROTOCOL.ReadSaveState()` has to read the
right one. The layout is chosen at build time, and only the chosen one is compiled into the binary:

- By default the supervisor reads the Intel x64 layout (`SMRAM_SAVE_STATE_MAP64`).
- With the `save_state_amd` Cargo feature it reads the AMD64 layout (`AMD_SMRAM_SAVE_STATE_MAP64`).

Pick the layout the platform's save state actually uses, which is not always the CPUID vendor. For example, an
emulator may always present an AMD64-style save state. Enable the feature from the platform binary crate:

```toml
[dependencies]
patina_mm_supervisor = { version = "*", features = ["save_state_amd"] }
```

### V1 Resource Descriptor HOBs

By default the supervisor checks only V2 resource descriptor HOBs for overlaps and ignores V1 ones, matching
`patina_dxe_core`. A platform that still describes memory with V1 descriptors can enable the
`v1_resource_descriptor_support` Cargo feature to check V1 descriptors instead. This feature is temporary.

### Integration with MM IPL

The MM IPL loads this binary and calls the entry point. The HOB list it passes contains:

- The MMRAM reserve HOB - MMRAM ranges
- The MM Communication Buffer HOB - user communication buffer
- The MM Common Region HOB - supervisor communication buffer
- The MP Information HOB - processor count and APIC IDs
- The MM Supervisor `PassDown` HOB - Ring 3 stacks, SMBASE array, firmware policy, and per-core state
- Memory allocation module HOBs for the MM Supervisor Core, User, and Init images and the MM drivers
- Resource descriptor HOBs, and optionally the CPU, MSEG SMRAM, unblock region and protected region HOBs
- FV HOBs for MM driver firmware volumes

## License

Copyright (c) Microsoft Corporation.

SPDX-License-Identifier: Apache-2.0
