# Memory Management

This portion of the core is responsible for producing the capabilities described in [Section 7.2](https://uefi.org/specs/UEFI/2.10_A/07_Services_Boot_Services.html#memory-allocation-services)
of the UEFI specification to support memory allocation and tracking within the UEFI environment and to track and report
the system memory map. In addition to UEFI spec APIs, the memory management module also implements the
["Global Coherency Domain(GCD)"](https://uefi.org/specs/PI/1.9/V2_Services_DXE_Services.html#global-coherency-domain-services)
APIs from the Platform Initialization (PI) spec. The memory management subsystem also produces a Global System Allocator
implementation for Rust Heap allocations that is used throughout the rest of the core.

## General Architecture

The memory management architecture of the Patina DXE Core is split into two main layers - an upper [`UefiAllocator`](memory_management.md#uefiallocator)
layer consisting of discrete allocators for each EFI memory type that are designed to service general heap allocations
in a performant manner, and a lower layer consisting of a single large (and relatively slower) allocator that tracks the
global system memory map at page-level granularity and enforces memory attributes (such as `Execute Protect`) on memory
ranges. This lower layer is called the [`GCD`](memory_management.md#global-coherency-domain-gcd) since it deals with
memory at the level of the overall global system memory map.

```mermaid
---
Allocator Architecture
---
block-beta
  columns 1
  block
    columns 3
    top(["Top Level 'UefiAllocators':"])
    EfiBootServicesData
    EfiBootServicesCode
    EfiRuntimeServicesData
    EfiRuntimeServicesCode
    EfiAcpiReclaimMemory
    EfiLoaderData
    EfiLoaderCode
    Etc...
  end
  GCD("Global Coherency Domain (GCD)")
```

UEFI Spec APIs that track specific memory types such as [AllocatePool](https://uefi.org/specs/UEFI/2.10_A/07_Services_Boot_Services.html#efi-boot-services-allocatepool)
and [AllocatePages](https://uefi.org/specs/UEFI/2.10_A/07_Services_Boot_Services.html#efi-boot-services-allocatepages)
are typically implemented by the `UefiAllocator` layer (sometimes as just a passthru for tracking the memory type). APIs
that are more general such as [GetMemoryMap](https://uefi.org/specs/UEFI/2.10_A/07_Services_Boot_Services.html#efi-boot-services-getmemorymap)
as well as the [GCD APIs](https://uefi.org/specs/PI/1.9/V2_Services_DXE_Services.html#global-coherency-domain-services)
from the PI spec interact directly with the lower-layer GCD allocator.

## UefiAllocator

### UefiAllocator - General Architecture and Performance

The `UefiAllocator` impelements a general purpose [slab allocator](https://en.wikipedia.org/wiki/Slab_allocation) based
on the [Fixed-Size Block Allocator](https://os.phil-opp.com/allocator-designs/#fixed-size-block-allocator) that is
presented as part of the [Writing an OS in Rust](https://os.phil-opp.com/) blog series.

Each allocator tracks "free" blocks of fixed sizes that are used to satisfy allocation requests. These lists are backed
up by a linked list allocator to satisfy allocations in the event that a fixed-sized block list doesn't exist (in the
case of very large allcoations) or does not have a free block.

This allows for a very efficient allocation procedure:

1. Round up requested allocation size to next block size.
2. Remove the first block from the corresponding "free-list" and return it.

If there are free blocks of the required size available this operation is constant-time. This should be the expected
normal case after the allocators have run for a while and have built up a set of free blocks.

Freeing a block (except for very large blocks) is also constant-time, since the procedure is the reverse of the above:

1. Round up the freed allocation size to the next block size.
2. Push the block on the front of the corresponding "free-list."

If the fixed-block size list corresponding to the requested block size is empty or if the requested size is larger than
any fixed-block size, then the allocation falls back to a linked-list based allocator. This is also typically constant-
time, since the first block in the linked-list backing allocator is larger than all the free-list block sizes (because
blocks are only freed back to the fallback allocator if they are larger than the all free-list block sizes). This means
that allocation typically consists of simply splitting the required block off the front of the first free block in the
list.

If the fallback linked-list allocator is _also_ not able to satisfy the request, then a new large block is fetched from
the GCD and inserted into the fallback linked-list allocator. This is a slower operation and its performance is a
function of how fragmented the GCD has become - on a typical boot this can extend to a search through hundreds of nodes.
This is a relatively rare event and the impact on overall allocator performance is negligible.

### Allocation "Buckets"

In order to ensure that the OS sees a generally stable memory map boot-to-boot, the UefiAllocator implementation can be
seeded with an initial "bucket" of memory that is statically assigned to the allocator at startup in a deterministic
fashion. This allows a platform integrator to specify (via means of a "Memory Type Info" HOB) a set of pre-determined
minimum sizes for each allocator. All memory associated with an `UefiAllocator` instance is reported to the OS as that
memory type (for example, all memory associated with the `EfiRuntimeServicesData` allocator in the GCD will be reported
to the OS as `EfiRuntimeServicesData`, even if it is not actually allocated). If the platform seeds the bucket with a
large enough initial allocation such that all memory requests of that type can be satisfied during boot without a
further call to the GCD for more memory, then all the memory of that type will be reported in a single contiguous block
to the OS that is stable from boot-to-boot. This facility is important for enabling certain use cases (such as
hibernate) where the OS assumes a stable boot-to-boot memory map.

### UefiAllocator Operations

The UefiAllocator supports the following operations:

* Creating a new allocator for arbitrary memory types. A subset of well-known allocators are provided by the core to
support UEFI spec standard memory types, but the spec also allows for arbitrary OEM-defined memory types. If a caller
makes an allocation request to a previously unused OEM-defined memory type, a new allocator instance is dynamically
instantiated to track memory for the new memory type.
* Retrieving the `EfiMemoryType` associated with the allocator. All allocations done with this allocator instance will
be of this type.
* Reserving pages for the allocator. This is used to seed the allocator with an initial [bucket](memory_management.md#allocation-buckets)
of memory.
* APIs for allocate and free operations of arbitrary sizes, including `impl` for [`Allocator`](https://doc.rust-lang.org/std/alloc/trait.Allocator.html)
and [`GloballAlloc`](https://doc.rust-lang.org/std/alloc/trait.GlobalAlloc.html)
traits. See [Rust `Allocator` and `GlobalAlloc` Implementations](memory_management.md#rust-allocator-and-globalalloc-implementations)
below.
* APIs for allocating and freeing pages (as distinct from arbitrary sizes). These are pass-throughs to the
underlying [GCD operations](memory_management.md#gcd-operations) with some logic to handle preserving ownership for
allocation buckets.
* Expanding the allocator by making a call to the GCD to acquire more memory if the allocator does not have enough
memory to satisfy a request.
* Locking the allocator to support exclusive access for allocation (see [Concurrency](memory_management.md#concurrency)).

## Global Coherency Domain (GCD)

### GCD - General Architecture and Performance

The GCD tracks memory allocations at the system level to provide a global view of the memory map. In addition, this is
level at which memory attributes (such as `Execute Protect` or `Read Protect`) are tracked.

The Patina DXE Core implements the GCD using a Red-Black Tree to track the memory regions within the
GCD. This gives the best expected performance when the number of elements in the GCD is expected to
be large. There are alternative storage implementations in the `patina_internal_collections` crate
within the core that implement the same interface that provide different performance characteristics
(which may be desirable if different assumptions are used - for example if the number of map entries
is expected to be small), but the RBT-based implementation is expected to give the best performance
in the general case.

### GCD Data Model

The GCD tracks both memory address space and I/O address space. Each node in the data structure tracks a region of the
address space and includes characteristics such as the memory or I/O type of the region, capabilities and attributes of
the region, as well as ownership information. Regions of the space can be split or merged as appropriate to maintain a
consistent view of the address space.

A sample memory GCD might look like the following:

```text
GCDMemType Range                             Capabilities     Attributes       ImageHandle      DeviceHandle
========== ================================= ================ ================ ================ ================
NonExist   0000000000000000-00000000000fffff 0000000000000000 0000000000000000 0x00000000000000 0x00000000000000
MMIO       0000000000100000-000000004fffffff c700000000027001 0000000000000001 0x00000000000000 0x00000000000000
NonExist   0000000050000000-0000000053ffffff 0000000000000000 0000000000000000 0x00000000000000 0x00000000000000
MMIO       0000000054000000-000000007fffffff c700000000027001 0000000000000001 0x00000000000000 0x00000000000000
SystemMem  0000000080000000-0000000080000fff 800000000002700f 0000000000002008 0x00000000000000 0x00000000000000
SystemMem  0000000080001000-0000000080002fff 800000000002700f 0000000000004008 0x00000000000002 0x00000000000000
SystemMem  0000000080003000-0000000081805fff 800000000002700f 0000000000002008 0x00000000000000 0x00000000000000
```

### GCD Operations

The GCD supports the following operations:

* Adding, Removing, and Allocating and Freeing regions within the address space. The semantics for these operations
largely follow the Platform Initialization spec [APIs](https://uefi.org/specs/PI/1.9/V2_Services_DXE_Services.html#gcd-memory-resources)
for manipulating address spaces.
* Configuring the capabilities and attributes of the memory space. The GCD uses CPU memory management hardware to
enforce these attributes where supported. See [Paging](cpu.md#paging-implementation) for details on how this hardware is
configured.
* Retrieving the current address space map as a list of descriptors containing details about each memory region.
* Locking the memory space to disallow modifications to the GCD. This allows the GCD to be protected in certain
sensitive scenarios (such as during [Exit Boot Services](memory_management.md#exit-boot-services-handlers)) where
modifications to the GCD are not permitted.
* Obtaining a locked instance of the GCD instance to allow for concurrency-safe modification of the memory map. (see
[Concurrency](memory_management.md#concurrency)).

The internal GCD implementation will ensure that a consistent map is maintained as various operations are performed to
transform the memory space. In general, all modifications of the GCD can result in adding, removing, splitting, or
merging GCD data nodes within the GCD data structure. For example, if some characteristic (such as attributes,
capabilities, memory type, etc.) is modified on a region of memory that is within a larger block of memory, that will
result in a split of the larger block into smaller blocks so that the region with new characteristic is carved out of
the larger block:

```mermaid
---
Splitting Blocks
---
block-beta
  columns 3
  single_large_block["Single Large Block (Characteristics = A)"]:3
  space
  blockArrowId4[\"Set Characteristics = B"/]
  space
  new_block_a["Block (Characteristics = A)"]
  new_block_b["Block (Characteristics = B)"]
  new_block_c["Block (Characteristics = A)"]
```

Similarly, if characteristics are modified on adjacent regions of memory such that the blocks are identical except for
the start and end of the address range, they will be merged into a single larger block:

```mermaid
---
Splitting Blocks
---
block-beta
  columns 3
  old_block_a["Block (Characteristics = A)"]
  old_block_b["Block (Characteristics = B)"]
  old_block_c["Block (Characteristics = A)"]
  space
  blockArrowId4[\"Set Characteristics = A"/]
  space
  single_large_block["Single Large Block (Characteristics = A)"]:3
```

## Concurrency

UefiAllocator and GCD operations require taking a lock on the associated data structure to prevent concurrent
modifications to internal allocation tracking structures by operations taking place at different TPL levels (for
example, allocating memory in an event callback that interrupts a lower TPL). This is accomplished by using a
[`TplMutex`](synchronization.md#tplmutex) that switches to the highest TPL level (uninterruptible) before executing the
requested operation. One consequence of this is that care must be taken in the memory subsystem implementation that
no explicit allocations or [implicit Rust heap allocations](memory_management.md#rust-allocator-and-globalalloc-implementations)
occur in the course of servicing an allocation.

In general, the entire memory management subsystem is designed to avoid implicit allocations while servicing allocation
calls to avoid reentrancy. If an attempt is made to re-acquire the lock (indicating an unexpected reentrancy bug has
occurred) then a panic will be generated.

## Rust `Allocator` and `GlobalAlloc` Implementations

In addition to producing the memory allocation APIs required by the UEFI spec, the memory allocation subsystem also
produces implementations of the [`Allocator`](https://doc.rust-lang.org/std/alloc/trait.Allocator.html) and
[`GloballAlloc`](https://doc.rust-lang.org/std/alloc/trait.GlobalAlloc.html) traits.

These implementations are used within the core for two purposes:

1. The `GlobalAlloc` implementation allows one of the `UefiAllocator` instances to be designated as the Rust Global
Allocator. This permits use of the standard Rust [`alloc`](https://doc.rust-lang.org/alloc) smart pointers (e.g. [Box](https://doc.rust-lang.org/alloc/boxed/index.html))
and collections (e.g. [Vec](https://doc.rust-lang.org/std/vec/struct.Vec.html), [BTreeMap](https://doc.rust-lang.org/std/collections/struct.BTreeMap.html)).
The `EfiBootServicesData` UefiAllocator instance is designated as the default global allocator for the Patina DXE Core.
2. UEFI requires being able to manage many different memory regions with different characteristics. As such, it may
require heap allocations that are not in the default `EfiBootServicesData` allocator. For example, the EFI System Tables
need to be allocated in `EfiRuntimeServicesData`. To facilitate this in a natural way, the [`Allocator`](https://doc.rust-lang.org/std/alloc/trait.Allocator.html)
is implemented on all the `UefiAllocators`. This is a [nightly-only experimental API](https://github.com/rust-lang/rust/issues/32838),
but aligning on this implementation and tracking it as it stabilizes provides a natural way to handle multiple
allocators in a manner consistent with the design point that the broader Rust community is working towards.

An example of how the `Allocator` trait can be used in the core to allocate memory in a different region:

```rust
let mut table = EfiRuntimeServicesTable {
  runtime_services: Box::new_in(rt, &EFI_RUNTIME_SERVICES_DATA_ALLOCATOR)
};
```

## Exit Boot Services Handlers

Platforms should ensure that when handling an `EXIT_BOOT_SERVICES` signal (and `PRE_EXIT_BOOT_SERVICES_SIGNAL`),
they do not change the memory map. This means allocating and freeing are disallowed once
`EFI_BOOT_SERVICES.ExitBootServices()` (`exit_boot_services()`) is invoked.

In the Patina DXE Core in release mode, allocating and freeing within the GCD (which changes the memory map and its key)
will return an error that can be handled by the corresponding driver.
In debug builds, any changes to the memory map following `exit_boot_services` will panic due to an assertion.

## Memory Protections

Patina (here called Patina or the core interchangeably) applies strict memory protections while still allowing for PI
and UEFI spec APIs to adjust them. Protections are applied categorically with the only customization currently supported
being [Compatibility Mode](#compatibility-mode).

> **Note:** This section primarily deals with access attributes. Caching attributes are platform and driver driven and
> outside the scope of this document. The core gets the initial platform specified caching attributes via the Resource
> Descriptor HOB v2 and persists whatever the GCD entry has on every other call. After this point, drivers (such as
> the PCI Host Bridge driver) may update memory regions with different caching attributes.

### General Flow

#### Page Table Initialization

When Patina is given control at its entry point, typically all memory is mapped as Read/Write/Execute (RWX). This is up
to the entity that launches Patina and Patina takes no dependencies on the initial memory state (other than the
fundamentals of its code regions being executable, etc.).

As soon as the GCD is initialized and memory allocations are possible, the core sets up a new page table using
the [patina_paging crate](https://github.com/OpenDevicePartnership/patina-paging/blob/main/README.md). Memory
allocations are required to back the page tables themselves. In this initial Patina owned page table, which is not
installed yet, all currently allocated memory is set to be non-executable, as the majority of memory is expected not to
be executable code.

Next, the Patina image location is discovered via the `MemoryAllocationModule` associated with it. The core needs
to ensure its own image code sections are read only (RO) and executable or else we will immediately fault after
installing this page table. See [Image Memory Protections](#image-memory-protections) for the rationale behind RO + X
for image code sections. All of the core's data sections are left as non-executable, as is done for other images.

> **Note:** All Rust components are monolithically compiled into the Patina image, so all of their image protections
> are applied at this point, as well.

Finally, all MMIO and reserved regions (as reported by resource descriptor HOBs) are mapped as non-executable. Patina
must map all of these regions because existing C drivers expect to be able to directly access this memory without going
through an API to allocate it.

After this, the page table is installed; all other pages are unmapped and access to them will generate a page fault.
Early platform integration will involve describing all MMIO and reserved regions in resource descriptor HOBs so they
will be mapped for use.

#### Allocations

All page allocations, regardless of source, will cause Patina to map the page as non-executable. If the allocating
entity requires different attributes, they will need to use PI spec, UEFI spec, or Patina APIs to update them. Most
allocations do not need different attributes.

Pool allocations, as described in the [UefiAllocator](#uefiallocator) section, are simply managed memory on top of
pages, so all pool memory allocated will be non-executable. No entity should attempt to set attributes on pool owned
memory, as many other entities may own memory from the same underlying page and attributes must be set at page
granularity per HW requirements. If an entity requires different attributes, they must instead allocate pages and
update the attributes.

When pages are freed, Patina will unmap the pages in the page table so that any further accesses to them cause page
faults. This helps to catch use-after-free bugs as well as meeting the cleanliness requirements of Patina.

### Image Memory Protections

Patina follows industry standards for image protection: making code sections RO + X and data sections non-executable. It
also ensures the stack for each image is non-executable.

#### Rust Component Memory Protection

As noted in the [page table initialization flow](#page-table-initialization), Rust components get their image memory
protections applied when Patina protects its own image, as they are monolithically compiled into it.

#### PI Dispatched Driver Memory Protection

When PI compliant drivers are [dispatched](./dispatcher.md) by Patina, it will read through the PE/COFF headers and
apply the appropriate memory attributes depending on the section type.

### Compatibility Mode

Compatibility Mode is the state used to describe a deprecated set of memory protections required to boot current
versions of Linux. The most common Linux bootloaders, shim and grub, currently crash with memory protections enabled.
Booting Linux is a critical scenario for platforms, so compatibility mode is implemented to support that case. Old
versions of Windows bootmgr are also susceptible to some of the issues listed below.

The [integration steps](../integrate/dxe_core.md#compatibility-mode) for Patina describe the single platform
configurable knob for compatibility mode, which is simply the ability for a platform to allow compatibility mode to
be entered. By default, this is false and compatibility will not be entered and any EFI_APPLICATION that is not marked
as NX_COMPAT will not be loaded.

The only way Patina will enter Compatibility Mode is by attempting to load an EFI_APPLICATION (chosen because this is
what bootloaders are) that does not have the NX_COMPAT DLL Characteristic set in its PE/COFF header.

Entering Compatibility Mode causes the following to occur:

* Map all memory in the legacy BIOS write back range (0x0 - 0xA000) as RWX if it is part of system memory. This is done
  as Linux both can have null pointer dereferences and attempts to access structures in that range.
* Map all newly allocated pages as RWX. This is done as non-NX_COMPAT bootloaders will attempt to allocate pages and
  execute from them without updating attributes.
* Map the current image that triggered Compatibility Mode as RWX. This is done as Linux will try to execute out of data
  regions.
* Map all memory owned by the `EFI_LOADER_CODE` and `EFI_LOADER_DATA` `UefiAllocators` as RWX. This supports Linux
  allocating pool memory and attempting to execute from it.
* The `EFI_MEMORY_ATTRIBUTE_PROTOCOL` is uninstalled. This is done because versions of Linux bootloaders in the wild
  will use the protocol to map their entire image as non-executable, then attempt to map each section as executable, but
  the subsequent calls aren't page aligned, causing the protocol to return an error, which is unchecked. The bootloader
  then attempts to execute out of non-executable memory and crashes.

Patina implements Compatibility Mode as a one time event that causes actions; it is not a global state that is
checked on each invocation of memory allocation or image loading. This design pattern was deliberately chosen. It
simplifies hot code paths and reduces the complexity of the feature. The only state that is tracked related to
Compatibility Mode is what default attributes are applied to new memory allocations. As stated above, this is
`efi::MEMORY_XP` when Compatibility Mode is not active and `0` (i.e. memory is mapped RWX) when Compatibility Mode is
active. This state is tracked in the GCD itself and does not exist as conditionals in the code.

### Future Work

Patina does not currently support the complete set of protections it will.

#### Heap Guard

C based FW implementations rely on guard pages around pages and pools to ensure buffer overflow is not occurring,
typically only in debug scenarios, at least for pool guard, due to the memory requirements it has. Rust has more built
in buffer safety guarantees than C does, but buffer overflows are still possible when interacting with raw pointers.
C drivers are also still executed by Patina, so there does remain value for them to have Heap Guard enabled.

#### Validation

The [C based UEFI shell apps](https://github.com/microsoft/mu_plus/blob/HEAD/UefiTestingPkg/Readme.md) that are used to
validate memory protections on a C codebase are not all valid to use in Patina; they rely on internal C DXE Core state.
These need to be ported to Rust or new tests designed. The other test apps also need to be run and confirmed that the
final memory state is as expected.
