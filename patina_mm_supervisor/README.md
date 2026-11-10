# Patina MM Supervisor Core

A Rust implementation of the MM (Management Mode) Supervisor Core for standalone MM on x64. It expects:

- Page tables already set up by the pre-MM phase
- All images loaded and ready to execute
- The BSP (Bootstrap Processor) to serve incoming requests
- The APs (Application Processors) to wait in a holding pen, checking a mailbox for work

## Memory Model

The supervisor does not use heap allocation. Its data structures are fixed-size arrays sized by a const generic:

- `MAX_CPUS` - Maximum number of CPUs supported, supplied as a const generic argument to `MmSupervisorCore`

This allows the entire supervisor to be instantiated as a `static` with no runtime allocation.

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
   - Returns once every registered core has finished per-core initialization

2. APs (Application Processors):
   - All other CPUs
   - Wait for BSP initialization to complete, then run per-core initialization and return

On later entries the BSP serves the pending request while the APs wait in the holding pen and poll their mailboxes for
commands. Each core returns once the BSP releases the APs.

During initialization the BSP validates the MM Init image allocation from the HOB list (inside MMRAM, page-aligned, and
not overlapping the Core) and records it. On the first SMI after initialization, before serving the request, it checks
again that the image lies inside MMRAM and that its executable pages are supervisor-only and read-only, then frees its
pages. The HOB list is not read again after initialization.

### Mailbox System

The mailbox system provides inter-processor communication:

- Each CPU index, the BSP's included, has a dedicated mailbox (cache-line aligned to avoid false sharing)
- BSP sends commands to APs via mailboxes
- APs respond with results through the same mailbox
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
// The const generic argument is the maximum CPU count used to size internal arrays.
static SUPERVISOR: MmSupervisorCore<MyPlatform, 8> = MmSupervisorCore::new();

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

Platforms register additional supervisor MMI handlers by implementing
`PlatformInfo::mmi_handlers`, which returns a static slice of handlers. The core dispatches
its built-in handlers first, then the platform handlers.

```rust
use patina_mm_supervisor::*;
use patina::standard::efi;

fn my_handler(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    // Handle the request
    efi::Status::SUCCESS
}

static MY_HANDLERS: &[SupervisorMmiHandler] = &[SupervisorMmiHandler {
    name: "MyHandler",
    handler_guid: patina::BinaryGuid::from_string("12345678-1234-5678-1234-567800000000").into_inner(),
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

By default the supervisor validates only V2 resource descriptor HOBs and ignores V1 ones, matching `patina_dxe_core`.
A platform that still depends on V1 resource descriptors can enable the `v1_resource_descriptor_support` Cargo feature
so V1 descriptors are also checked for overlaps. This feature is temporary and will be removed once platforms produce
V2 descriptors only.

### Integration with MM IPL

The MM IPL (from EDK2/MmSupervisorPkg) loads this binary and calls the entry point. The HOB list passed contains:

- `gEfiMmPeiMmramMemoryReserveGuid` - MMRAM ranges
- `gMmCommBufferHobGuid` - User communication buffer
- `gMmCommonRegionHobGuid` - Supervisor communication buffer
- `gMpInformationHobGuid` - Processor count and APIC IDs
- The MM Supervisor `PassDown` HOB - Ring 3 stacks, SMBASE array, firmware policy, and per-core state
- Memory allocation module HOBs for the MM Supervisor Core, User, and Init images
- FV HOBs for MM driver firmware volumes

## License

Copyright (c) Microsoft Corporation.

SPDX-License-Identifier: Apache-2.0
