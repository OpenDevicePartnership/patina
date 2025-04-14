# RFC: Memory Management Interface

To support scaling both within the core and throughout Components, stable APIs
must be created for all functionality provided by the core. This RFC proposes the
creation of a `MemoryManager` trait within uefi-sdk to act as the common interface
for all externally available memory APIs.

## Change Log

- 2024-04-06: Initial RFC created.
- 2024-04-07: Changes AllocationOptions to not use implementation defined defaults.
- 2024-04-07: Further clarification on the `HeapAllocator` type.
- 2024-04-09: Removed HeapAllocator, added more details on other types.
- 2024-04-14: Removed panic wrappers for PageAllocation, added slice wrappers.

## Motivation

As Patina scales to include extensibility, a scalable and stable API will be needed for
all critical services between cores. Memory management is a foundational service and
can used as an initial testbed for these services.

## Technology Background

This design assumes the presence and use of the Component and Service model within the
dependency injection-based dispatch of rust-based extensions to the core binary. More
details on this model can be found in the [Component Model](https://github.com/OpenDevicePartnership/uefi-dxe-core/blob/main/docs/src/dxe_core/component_model.md)
document.

## Goals

1. Create a stable a "rusty" memory management API
    - Page Allocations
    - Typed heap allocation
    - Page attribute manipulation
    - Future Additions
2. Create API usable both by components and within the core.

## Requirements

1. The API should be scalable for current and future use cases.
2. The API should use proper Rust safety abstractions, safe by default.
3. The API should be suitable for UEFI use cases.

## Unresolved Questions

- What are the correct layers of abstractions for Core APIs? These are subjective with
different trade-offs. One question of this RFC is to establish a pattern that can be
re-used for future core APIs.
- Where to use UEFI definitions, versus unique definitions in Core APIs

## Prior Art (Existing PI C Implementation)

This design takes inspiration from the [UEFI Memory Allocation APIs](https://uefi.org/specs/UEFI/2.10/07_Services_Boot_Services.html#memory-allocation-services)
and the UEFI Memory attributes protocol. These are used to establish existing use
cases but should be improved upon by taking advantage of rust to create a safer and
more extensible API.

For allocation management, the [Allocator trait](https://doc.rust-lang.org/std/alloc/trait.Allocator.html)
can be referenced for prior art on abstracting arbitrary allocations.

## Alternatives

There are a large number of possible ways the specific interface could be implemented,
but the most significant alternative evaluated is related to the services paradigm
for core services such as memory management. The alternative to this would be using
static routines.

### Static Routines

Alternatives to the Service model would be to instead expose functions to the components
and internally in the core. Generally, one downside of this is that relying on static
routines can be significantly more difficult to unit test because this requires some
global locking during the tests and makes creating mocks more difficult depending
on the implementation. These static routines could be implemented in the core or
in the SDK wrapping a global dyn reference.

Exposing this from the core would additionally mean that the components must take
dependency on a specific implementation that would only further complicate the testing
story and makes it difficult for the core to refactor this logic in the future
without breaking components.

Exposing static routines within the SDK that wrap a dyn reference effectively takes
the Service model approach of having a dyn reference, but still limit testability and
consistency.

## Rust Code Design

### Memory Manager Service

The design proposal is to create a new trait definition in the uefi-sdk to implement
core memory functionality called the `MemoryManager` trait. This RFC will not go into
the details of the Service model, but information can be found in the [Component Model](https://github.com/OpenDevicePartnership/uefi-dxe-core/blob/main/docs/src/dxe_core/component_model.md)
document. This trait will be accessed through a `Service<dyn MemoryManager>` from external
components and directly against the implementor within dxe-core itself. This trait will
define functions for

- Allocating/Freeing memory pages.
- Acquiring an allocator for _typed_ heap allocations.
- Getting and changing memory attributes.

More functions are likely to be added in the future for other memory related operations
such as adding memory or accessing the memory map. The currently proposed definitions
is as follows:

```rust
pub trait MemoryManager {
    fn allocate_pages(
        &self,
        page_count: usize,
        options: AllocationOptions,
    ) -> Result<PageAllocation, MemoryError>;

    fn allocate_zero_pages(
        &self,
        page_count: usize,
        options: AllocationOptions,
    ) -> Result<PageAllocation, MemoryError>;

    unsafe fn free_pages(
        &self,
        address: usize,
        page_count: usize
    ) -> Result<(), MemoryError>;

    fn get_allocator(
        &self,
        memory_type: EfiMemoryType
    ) -> Result<&'static dyn Allocator, MemoryError>;

    unsafe fn set_page_attributes(
        &self,
        address: usize,
        page_count: usize,
        access: AccessType,
        caching: Option<CachingType>,
    ) -> Result<(), MemoryError>;

    fn get_page_attributes(
        &self,
        address: usize,
        page_count: usize
    ) -> Result<(AccessType, CachingType), MemoryError>;
}
```

Consumer components may acquire this implementation though the service dependency injection.

```rust
pub fn component (memory_manager: Service<dyn MememoryManager>) -> Result<()> {
    // component logic
}
```

This API uses a few other wrapper types that intend to add convenience for the caller,
introduce more safe management of memory, and to allow for easier extension in the future.
These are detailed below.

### Page Allocation Type

All page allocations will return a `PageAllocation` type. This structure wraps the raw
memory allocation and allows the caller to then convert the allocation into different
usable types such as a smart pointer, a raw pointer, or an initialized static reference.
This allows pages to have automatic freeing semantics until requested otherwise by either
converting to a manually managed raw pointer or explicitly leaking the memory as a static
reference.

```rust
pub struct PageAllocation {
    address: usize,
    page_count: usize,
    memory_manager: &'static dyn MemoryManager,
}

impl PageAllocation {
    pub fn into_raw_ptr<T>(self) -> *mut T { ... }
    pub fn into_raw_slice<T>(self) -> *mut [T] { ... }
    pub fn byte_length(&self) -> usize { ... }
    pub fn page_count(&self) -> usize { ... }
    pub fn try_into_box<T>(self, value: T) -> Option<Box<T, PageFree>> { ... }
    pub fn into_boxed_slice<T: Default>(self) -> Box<[T], PageFree> { ... }
    pub fn try_leak_as<T>(self, value: T) -> Option<&'static T> { ... }
    pub fn leak_as_slice<T: Default>(self) -> &'static [T] { ... }
}

```

### Allocation Options

The page allocation options such at memory type, alignment, and allocation strategy
are being wrapped in a separate type `AllocationsOptions`. This is to allow easy
but explicit use of default options while overwriting new values as needed with
`.with_` routines.

```rust
pub struct AllocationOptions {
    allocation_strategy: PageAllocationStrategy,
    alignment: usize,
    memory_type: EfiMemoryType,
}

impl AllocationOptions {
    // accessors and init
    pub const fn with_strategy(mut self, allocation_strategy: PageAllocationStrategy) -> Self { ... }
    pub const fn with_alignment(mut self, alignment: usize) -> Self { ... }
    pub const fn with_memory_type(mut self, memory_type: EfiMemoryType) -> Self { ... }
}
```

### Memory Attribute Types

The following enums were created to represent the Access and Caching attributes
of memory exposed to the caller.

```rust
pub enum AccessType {
    NoAccess,
    ReadOnly,
    ReadWrite,
    ReadExecute,
    ReadWriteExecute,
}

pub enum CachingType {
    Uncached,
    WriteCombining,
    WriteBack,
    WriteThrough,
}
```

These definitions were chosen to more accurate reflect hardware then the definitions
currently used in the UEFI specification. Notably, there is no current notion of
what memory is "capable" of, but only what is actually configured. Additionally,
There is no concept of "read-protect" as this does not accurate reflect what hardware
supports. Instead `NoAccess` was chosen as the implementation will treat this as
unmapped and so it does not make sense to allow this in conjunction with other
protections.

## Guide-Level Explanation

### Memory Manager

Memory Management is done through the `MemoryManager` trait. For Components, this trait
should be used through a service wrapper to acquire the memory manager implementation from the core.

```rust
pub fn component (memory_manager: Service<dyn MememoryManager>) ->Result<()> {
    .
}
```

### Page Allocations

With the memory manager, a component can allocate typed memory, pages, and otherwise
alter/inspect memory state. A common use case of this would be allocate memory pages.
Allocations can be initialized as several different types: smart pointers, static types,
or raw pointers.

```rust
// Create a boxed type, this will be freed automatically.
let allocation = memory_manager.allocate_pages(1, AllocationOptions::default())?;
let boxed_u32 = allocation.into_box(42_u32);

// Create a raw pointer, this must be freed manually
let allocation = memory_manager.allocate_pages(1, AllocationOptions::default())?;
let ptr = allocation.into_raw_ptr::<u8>();
ptr.write(42);
unsafe{ memory_manager.free_pages(ptr as usize, 1) };

// Create a leaked static reference, this will not be freed and so is safe to share.
let allocation = memory_manager.allocate_pages(1, AllocationOptions::default())?;
let static_u32 = allocation.leak_as(42);
```

All previous use cases used the `AllocationOptions::default()` to use the API defined
default options. However. some page allocation requires additional constraints such
as memory type or alignment. Additional constraints may be specified through the
`AllocationOptions` parameter using the `.with_` routine to override default values.

```rust
let options = AllocationOptions::default()
    .with_memory_type(EfiMemoryType::BootServicesData)
    .with_alignment(0x2000);

let allocation = memory_manager.allocate_pages(1, options)?;
```

### Heap Allocations

Another use case of the memory manager is to make heap allocations within a specific memory
type. For allocation occuring with the bootServicesData, the global allocator can be used
through the default allocations like `Box::new(T)` This is done by first acquiring the `Allocator`
then using the heap allocator to create object.

```rust
let allocator = memory_manager.get_allocator(EfiMemoryType::EfiBootServicesData)?;
```

The `Allocator` can be used to initiate standard and custom types using the allocator API.

```rust
let boxed_u32 = Box::new_in(42_u32, allocator);
```

### Memory Attributes

Some operations require changing the attributes of memory, such as access permissions and caching.
For these operations, the memory manager exposes the `set_page_attributes` and `get_page_attributes`
routines.

```rust
// Change a page to be inaccessible, leaving the caching unchanged.
memory_manager.set_page_attributes(address, 1, AccessType::NoAccess, None)?;

// Check that the access type was changed
let (access, caching) = memory_manager.get_page_attributes(address, 1)?;
assert!(access = AccessType::NoAccess);
```
