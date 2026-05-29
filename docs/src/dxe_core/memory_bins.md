# Memory Bins

Memory bins are pre-allocated regions of memory for specific EFI memory types that stabilize the UEFI runtime
memory footprint across boots. This stability is required for S4 (hibernate) resume, where the OS must restore
system memory to the same layout as the previous boot. Without bins, small variations in runtime memory allocation
patterns between boots can shift memory map entries and break S4 resume.

For background on the S4 problem and the overall bin design, refer to the
[edk2 Memory Bin Feature](https://github.com/tianocore/edk2/blob/HEAD/MdeModulePkg/Core/MemoryBins.md) document. This
page focuses on the Patina implementation of the DXE side of that design.

## How Bins Work

A platform declares the desired bin sizes by producing a **Memory Type Information GUID HOB**
(`MEMORY_TYPE_INFO_HOB_GUID`). Each entry in this HOB specifies a memory type and a page count representing
the bin size for that type. The memory map must not change during S4 resume because the OS will restore system memory
from disk. The memory bins keep memory ranges consistent for ranges of memory types that need to be consistent across
hibernation.

- `EfiReservedMemoryType`
  - Memory that is reserved for firmware use and may not be used by the OS.
- `EfiRuntimeServicesCode`
  - Used for UEFI runtime services code sections.
- `EfiRuntimeServicesData`
  - Used for UEFI runtime services data buffers.
- `EfiACPIMemoryNVS`
  - Memory used or reserved by the system (e.g. ACPI FACS) and must not be used by the operating system. This memory is
    required to be saved and restored across across an NVS sleep and saved in S4.
- `EfiACPIReclaimMemory`
  - Used for most ACPI tables. Memory that is preserved by the loader and OS until ACPI is enabled and the ACPI tables
    are read. This memory is required to be saved and restored across an NVS sleep cycle and saved in S4.

When the Patina DXE Core initializes memory services, it:

1. Finds the Memory Type Information HOB and extracts the bin configuration.
2. Establishes a contiguous address range for the bins.
3. Steers page allocations for bin types into their designated range.
4. Tracks allocation statistics so BDS can recommend next-boot bin sizes.
5. Adjusts `GetMemoryMap()` output so each bin appears as a single descriptor of its type, absorbing any free
   space within the bin range.
6. Publishes the memory type information config table so BDS can recommend next-boot bin sizes.

The bin size is an intentional over-allocation. Runtime allocations that fluctuate between boots are absorbed by
the bin, so the overall memory map reported to the OS remains stable.

## Patina Implementation

The implementation primarily resides in two files:

- [`allocator.rs`](https://github.com/OpenDevicePartnership/patina/blob/HEAD/patina_dxe_core/src/allocator.rs) -
  Integration points in the DXE Core memory allocator.
- [`memory_bin.rs`](https://github.com/OpenDevicePartnership/patina/blob/HEAD/patina_dxe_core/src/memory_bin.rs) -
  The standalone `MemoryBinManager` module.

### Input HOBs

The bin feature consumes up to three types of HOBs from the PEI phase. The first HOB enables basic memory bin support
while the additional HOBs provide extra control over bin placement from the HOB producer phase.

#### 1. Memory Type Information GUID HOB (required)

A GUID Extension HOB (`EFI_HOB_TYPE_GUID_EXTENSION`) whose `Name` matches `MEMORY_TYPE_INFO_HOB_GUID`
(`4C19049F-4137-4DD3-9C10-8B97A83FFDFA`). The HOB data is an array of `EFI_MEMORY_TYPE_INFORMATION`
entries terminated by a sentinel entry with `Type = EfiMaxMemoryType`:

```text
+--------+--------+--------+--------+--------+--------+---
| Type₀  | Pages₀ | Type₁  | Pages₁ | ...    | 0x10   | 0
+--------+--------+--------+--------+--------+--------+---
  u32      u32      u32      u32               sentinel
```

Each entry specifies a memory type and the number of 4 KB pages to reserve for that type's bin.

**Fields used:**

| Field           | Size  | Description                                                                                                                                     |
|-----------------|-------|-------------------------------------------------------------------------------------------------------------------------------------------------|
| `Type`          | `u32` | `efi::MemoryType` value (e.g. `EfiRuntimeServicesData = 6`).                                                                                    |
| `NumberOfPages` | `u32` | Bin size in 4 KB pages. The actual GCD allocation is aligned up to the type's granularity (64 KB for runtime types on AArch64, 4 KB otherwise). |

If this HOB is absent, no bins are initialized and the feature is disabled.

#### 2. Resource Descriptor HOB with Owner GUID (optional, recommended)

A Resource Descriptor HOB (`EFI_HOB_TYPE_RESOURCE_DESCRIPTOR`) whose `Owner` field matches
`MEMORY_TYPE_INFO_HOB_GUID`. This HOB describes a pre-allocated contiguous memory region that
PEI set aside for the bins.

```text
+----------+--------------------------+------+------+-----------+-----------+
| Header   | Owner                    | Type | Attr | PhysStart | Length    |
| (Hob)    | 4C19049F-...-A83FFDFA    | 0x00 | 0x07 |           |           |
+----------+--------------------------+------+------+-----------+-----------+
                                        ^      ^
                                        |      PRESENT | INITIALIZED | TESTED
                                        EFI_RESOURCE_SYSTEM_MEMORY
```

**Fields used:**

| Field               | Value/Constraint                          | Description                                                                |
|---------------------|-------------------------------------------|----------------------------------------------------------------------------|
| `Owner`             | `MEMORY_TYPE_INFO_HOB_GUID`               | Identifies this as the bin region HOB.                                     |
| `ResourceType`      | `EFI_RESOURCE_SYSTEM_MEMORY` (0)          | Must be system memory.                                                     |
| `ResourceAttribute` | `PRESENT \| INITIALIZED \| TESTED` (0x07) | Must have all three tested-memory flags.                                   |
| `PhysicalStart`     | Address                                   | Base address of the bin region.                                            |
| `ResourceLength`    | Size in bytes                             | Must be ≥ total bin size (sum of aligned page counts).                     |

**Validation rules:**

- Exactly one Resource Descriptor HOB with this owner GUID must exist. Multiple are rejected.
- `ResourceLength` must be large enough to hold all bins (checked via `calculate_total_bin_size()`).
- If this HOB is present and valid, bins use the provided range (Path A). If absent, DXE allocates
  its own range (Path B).

#### 3. Memory Allocation HOBs with Name GUID (optional)

Memory Allocation HOBs (`EFI_HOB_TYPE_MEMORY_ALLOCATION`) whose `Name` field in the allocation
descriptor matches `MEMORY_TYPE_INFO_HOB_GUID`. These are produced by PEI's bin-aware allocator
to mark runtime allocations that PEI made within the bin region.

```text
+----------+------------------------------+-----------+-----------+------+---+
| Header   | Name                         | MemBase   | MemLen    | Type |...|
| (Hob)    | 4C19049F-...-A83FFDFA        |           |           |      |   |
+----------+------------------------------+-----------+-----------+------+---+
```

**Fields used:**

| Field               | Description                                                                 |
|---------------------|-----------------------------------------------------------------------------|
| `Name`              | `MEMORY_TYPE_INFO_HOB_GUID` - marks this as a PEI bin-aware allocation.     |
| `MemoryBaseAddress` | Physical address of the allocation.                                         |
| `MemoryLength`      | Size in bytes (converted to pages).                                         |
| `MemoryType`        | `efi::MemoryType` of the allocation.                                        |

Patina iterates these HOBs after bin initialization and calls `seed_statistics_from_hob()` for each. If the allocation
falls within the type's bin range, `current_number_of_pages()` is incremented to account for PEI-phase bin usage.
Allocations that fall outside all bin ranges are not counted.

Memory Allocation HOBs without the `MEMORY_TYPE_INFO_HOB_GUID` name are processed normally by
`process_hob_allocations()` but are not included in bin statistics.

### Initialization

Bin initialization runs once during `init_memory_support()`, after the GCD and pre-DXE HOB allocations have been fully
processed.

The initialization flow resolves a contiguous bin range and then subdivides it into per-type bins. The range is
resolved in priority order:

1. PEI bins (Path A): Use a pre-allocated range from an incoming Resource Descriptor HOB.
2. DXE bins (Path B): Designate a single contiguous block from the GCD.

### Path A: PEI provided bin range

If PEI allocated memory bins (indicated by a Resource Descriptor HOB with an owner GUID of`MEMORY_TYPE_INFO_HOB_GUID`),
Patina uses that pre-allocated range directly:

- The Resource Descriptor HOB must describe `EFI_RESOURCE_SYSTEM_MEMORY` with `PRESENT | INITIALIZED | TESTED`
  attributes.
- Exactly one such HOB must exist. If multiple are found, all are rejected since this request would be ambiguous.
- The range must be large enough to fit all bins (including alignment padding).
- Bins are divided within the range from the top address downward, with each bin aligned to its type's allocation
  granularity (64 KB for runtime types on AArch64, 4 KB otherwise).

This path provides the most resilience for hibernate stability because the bin region lives at the same physical
address every boot (the platform controls where it is placed in PEI).

After bin ranges are established, Patina scans Memory Allocation HOBs whose `Name` field matches
`MEMORY_TYPE_INFO_HOB_GUID`. These are allocations PEI's bin-aware allocator made. For each one that falls within
a bin range, the bin's `current_number_of_pages` is incremented to seed the statistics with pre-DXE usage.

### Path B: DXE-allocated bins

If no Resource Descriptor HOB is found, Patina allocates a single contiguous block from the GCD that is large enough
to hold all bins plus worst-case alignment padding:

1. A conservative total size is calculated which is the sum of all entry sizes plus one unit of `max_granularity` per
   entry for alignment padding, rounded up to `max_granularity`.
2. The block is allocated with `GCD.allocate_memory_space()` with alignment matching the maximum granularity
   across all bin types (`MemoryBinManager::max_granularity()`).
3. The block is immediately freed back to the GCD so it can be re-claimed.
4. The block is then subdivided into per-type bins using the same logic as Path A.

This path provides less stability than Path A because the bin addresses depend on the GCD allocator's state at
the time of initialization, which has a relatively greater chance to vary between boots.

### Integration with Per-Type Allocators

Patina uses a per-type allocator model where each EFI memory type has its own `FixedSizeBlockAllocator` that manages
a pool of pages obtained from the GCD. Two allocation paths must respect bin boundaries:

1. UEFI API path: `EFI_BOOT_SERVICES.AllocatePages()` calls `core_allocate_pages()`, which delegates to the
   per-type allocator's `allocate_pages()`.
2. Internal expansion path: Pool allocations (`AllocatePool`), Rust heap allocations (`Vec`, `Box`), and
   allocator expansion call `GCD.allocate_memory_space()` directly from within the `SpinLockedFixedSizeBlockAllocator`,
   bypassing `core_allocate_pages()`.

Two mechanisms ensure allocations land in bins and bins are protected:

#### GCD Ownership Protection

During bin initialization, bin pages are allocated then freed with `free_memory_space_preserving_ownership()`. The
pages become free but retain the per-type allocator's handle as the GCD owner. Other allocators using
`AllocateRespectingOwnership` (the default strategy) skip blocks owned by a different handle, preventing
cross-type intrusion at the GCD level.

#### Bin-Preference Allocation

The `reserved_range` set on each per-type `SpinLockedFixedSizeBlockAllocator` enables two behaviors:

- Allocation preference: `allocate_from_gcd()` first attempts `TopDown(bin_end)` to allocate within the bin
  range. If the result lands within the bin, it is used immediately. If the bin is full (the result lands below
  the bin base), the allocation is freed and retried with the original strategy. This preference applies to
  unconstrained strategies (`TopDown(None)`, `BottomUp(None)`) and constrained strategies
  (`TopDown(Some(max))`, `BottomUp(Some(max))`) when the max address is at or above the bin range. `Address`
  strategies are never redirected. This ensures that `AllocateMaxAddress` calls from DXE drivers also land in
  their designated bins when possible, preventing fragmented special-type allocations outside bin ranges.
- Ownership-preserving free: `free_pages()` checks `in_reserved_range()`. Pages within the bin are freed with
  `free_memory_space_preserving_ownership()`, retaining the GCD ownership handle so the bin pages remain protected
  after free.

### Statistics Tracking

`core_allocate_pages()` and `core_free_pages()` record each operation in the bin manager for special memory types only
(runtime, reserved, ACPI, and PAL types that persist into the OS). Non-special types like `BootServicesCode` and
`BootServicesData` are skipped. Their allocations do not affect bin descriptors or BDS recommendations, so tracking them
would be pure overhead.

For tracked types:

- Every allocation and free for the type increments/decrements the "number of pages".
- If `current_number_of_pages` exceeds the previous peak, the memory type information entry is updated.
  BDS can use this value to recommend a larger bin size for the next boot.

Internal allocator expansion (pool growth, Rust heap allocations) is not individually tracked in the bin
statistics. This is consistent with edk2, where pool allocations are only visible through the page-level
expansion events they trigger.

### GetMemoryMap() Bin Descriptors

When `GetMemoryMap()` is called, the GCD populates the memory map as usual, then the bin manager post-processes the
buffer. For each "special" memory type with an active bin:

1. Find `EfiConventionalMemory` entries that overlap the bin range.
2. Convert entries fully within the bin to the bin's memory type.
3. Split entries that partially overlap at the bin boundaries.
4. Set `EFI_MEMORY_RUNTIME` on entries for runtime types.

This ensures the OS sees a single large descriptor for each bin type, regardless of how much of the bin is actually
allocated. Free space within the bin is reported as the bin type rather than as conventional memory.

The buffer size calculation in `get_memory_map()` accounts for the worst-case number of additional entries from bin
splitting (2 extra entries per active bin).

### Config Table

The bin manager's memory type information is published as the `gMemoryTypeInformationGuid` config table via
`install_memory_type_info_table()`. BDS consumes this table to decide whether bin sizes need adjustment.

The config table data comes from a fixed-size `[EFiMemoryTypeInformation]` array inside the `MemoryBinManager`
static. It is populated from the HOB during initialization with the original HOB values. `record_allocation()`
updates entries when in-bin usage exceeds the original value, creating a monotonically increasing high-water mark.

If bins are not initialized (no Memory Type Information HOB was present), the config table is not installed.

## Comparison with edk2

Since both the edk2 DXE Core and Patina DXE Core implement PEI memory bin support, this section makes comparison of
the two implementations easier by summarizing the key design and implementation differences in one place.

PEI bin support is provided by the PEI Core (C code in edk2), the Patina DXE Core consumes the HOBs that PEI produces.

| Aspect                         | edk2                                                                                                                                                  | Patina                                                                                                                                            |
|--------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------|
| Primary Implementation         | Shared implementation in `MemoryBin.c` (C, used by both PEI and DXE cores)                                                                            | Not applicable. Patina is DXE-only. PEI bin logic lives in edk2's PEI Core.                                                                       |
| Bin state storage              | Global arrays `mMemoryTypeStatistics[]` and `gMemoryTypeInformation[]`                                                                                | `MemoryBinManager` struct behind a `TplMutex` static. Statistics and memory type info are fixed-size arrays inside the struct.                    |
| Allocation steering            | `FindFreePages()` checks `mMemoryTypeStatistics[Type].MaximumAddress` / `BaseAddress` first, falls back to `mDefaultMaximumAddress`, then full range. | `allocate_from_gcd()` tries `TopDown(bin_end)` first for unconstrained and reachable constrained strategies, falls back to the original strategy. |
| GetMemoryMap() bin descriptors | Inline loop in `CoreGetMemoryMap()` splits and converts `EfiConventionalMemory` entries overlapping bins.                                             | `apply_bin_descriptors()` post-processes the buffer after `populate_efi_memory_map()`. Same splitting logic.                                      |
| Statistics update              | `UpdateMemoryStatistics()` counts allocations in the bin range or in the "default range".                                                             | `record_allocation()` / `record_free()` count all special-type allocations in and outside the bin range.                                          |
| PEI HOB seeding                | `InitializeBinStatisticsFromRange()` processes Memory Allocation HOBs with the `MEMORY_TYPE_INFO_HOB_GUID` name.                                      | `seed_bin_statistics_from_hobs()` performs the same scan and calls `seed_statistics_from_hob()` for each matching HOB.                            |
| Memory type info storage       | `gMemoryTypeInformation[]` is a global C array.                                                                                                       | Fixed-size `[EFiMemoryTypeInformation]` array inside the `MemoryBinManager` static, to allow pointers to a stable memory location.                |

### What Patina Does Not Implement

- PEI bin allocation. PEI bin setup, PCD-based opt-in, PHIT updates, and Memory Allocation HOB marking are all
  handled by the pre-DXE environment.
- BDS heuristics. The bin size recommendation logic and `PcdResetOnMemoryTypeInformationChange` reboot are in
  BDS code outside of the DXE Core.

## Logging

All memory bin log messages use the `memory_bin` log target. These are some key messages and their log levels:

| Level   | Message                                | When                                 |
|---------|----------------------------------------|--------------------------------------|
| `info`  | Memory Type Information HOB found      | HOB extraction during init           |
| `info`  | Bin layout per type (base, max, pages) | Bin initialization                   |
| `info`  | Bins allocated/initialized from range  | Initialization complete              |
| `debug` | PEI seed per allocation HOBs           | Statistics seeding                   |
| `debug` | GetMemoryMap() bin processing          | Each bin processed in GetMemoryMap() |
| `trace` | Individual alloc/free recording        | Every page allocation/free           |
| `trace` | Bin stats old -> new transitions       | Every record_allocation/record_free  |
| `trace` | Bin table peak updates                 | When peak exceeds previous value     |

Filter with the `memory_bin` log target to isolate bin-related output.
