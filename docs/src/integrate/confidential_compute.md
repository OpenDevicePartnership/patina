# Confidential Compute

Patina supports being run as a confidential compute guest. It is expected to broadly work as an unenlightened guest
(e.g. with a paravisor) across different virtualization technologies. Work is ongoing to add support as an enlightened
guest. For the latest updates, view [the tracking item](https://github.com/OpenDevicePartnership/patina/issues/1783).

## Platform Enablement

To enable the features that are specifically for a confidential compute guest in Patina, build patina_dxe_core with
the `confidential_compute` flag set:

```text
# Cargo.toml
patina_dxe_core = { version = "x.x.x", features = ["confidential_compute"] }
```

### Shared Memory Regions

In order to map shared regions with the VMM, a C driver can call the `AliasedMemoryMappingProtocol` which has a C
binding in patina-edk2. This protocol allows arbitrary VA to PA mappings.

For example, in a TDX VM where a page is being shared, the driver would call:

```c
Status =  gBS->LocateProtocol(&gAliasedMemoryMappingProtocolGuid, NULL, (VOID**)&mAliasedMappingProtocol);
if (EFI_ERROR(Status)) {
  return Status;
}

// We are using AllocatePages to get a VA assigned to us, which we are going to access as if the shared bit was set,
// which will canonicalize our VA
Pa = AllocatePages(1);
if (Pa == NULL) {
  return EFI_OUT_OF_RESOURCES;
}

Va = Pa + TDX_SHARED_BIT;
Va |= CANONICAL_MASK;

// Set the shared bit in the page table
mAliasedMappingProtocol->CreateAliasedMapping(
            mAliasedMappingProtocol,
            Va,
            Pa,
            Length,
            Attributes
        );

if (EFI_ERROR(Status)) {
  return Status;
}

// Now we are good to directly access the Va
ZeroMem(Va, EFI_PAGE_SIZE);
```

If the shared page is being destroyed, the C driver can call:

```c
mAliasedMappingProtocol->UnmapAliasedMapping(
            mAliasedMappingProtocol,
            Va,
            Length
);
```

This allows a platform to directly specify shared pages to Patina. Patina requires this because it does not inherit
prior phase page tables and requires platforms to be deterministic about mappings.

### MTRRs

For x64 virtualization technologies, MTRR access may or may not be allowed. Patina will check if MTRRs are supported
and if not will continue to gracefully map pages.

### Virtualization Exceptions

Patina, like EDK II, does not have generic virtualization exception handling because this is platform/hypervisor
specific. Platforms must provide virtualization exception handlers during Patina initialization that Patina will
install.

```rust
# extern crate patina_dxe_core;
use patina_dxe_core::{CpuInfo, ExceptionContext, ExceptionType, InterruptHandler};

const EXCEPTION_VECTOR_VE: ExceptionType = 20;

struct MyPlatform;

impl InterruptHandler for MyPlatform {
    fn handle_interrupt(&'static self, exception_type: ExceptionType, context: &mut ExceptionContext) {
      // Handle...
    }
}

impl CpuInfo for MyPlatform {
    fn exception_handlers() -> &'static [(ExceptionType, &'static dyn InterruptHandler)] {
        static EXCEPTION_HANDLERS: [(ExceptionType, &'static dyn InterruptHandler); 1] =
            [(EXCEPTION_VECTOR_VE, &MyPlatform)];

        &EXCEPTION_HANDLERS
    }
}
```

Patina will install these to the new IDT/VBAR immediately upon creation to ensure the exceptions can be serviced.
