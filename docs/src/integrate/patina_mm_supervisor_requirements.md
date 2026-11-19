# Patina MM Supervisor Requirements

The Patina MM Supervisor (`patina_mm_supervisor`) and MM User Core (`patina_mm_user_core`) replace the C based
Standalone MM Core and MM Supervisor. They expect more from the platform than the
[Platform Initialization (PI) Spec](https://uefi.org/specifications) and the EDK II Standalone MM Core do.

The supervisor runs in Ring 0 inside MMRAM. It takes over the page table MM Init built and is then the only code that
changes it, and it owns the security policy and the processors. The user core and every MM driver run in Ring 3. The
supervisor does not rebuild the state it inherits from the pre-MM phase (MM Init): it validates it on the first MMI
and halts the boot on any mismatch. Most requirements below exist so those checks pass.

- [Initialization Validation](../mm/validation.md) lists every check the supervisor makes, and what is not checked
  yet. [Theory and Operation](../mm/operation.md) explains how both cores work.
- The [MM Supervisor RFC](https://github.com/OpenDevicePartnership/patina/blob/main/docs/src/rfc/text/0026-mm-supv.md)
  explains the design.

## Platform Requirements

### 1. Processor Requirements

#### 1.1 x64 Only

The supervisor and the user core support x64 only. The privilege transitions, the MMI entry checks and the save state
access are written for x64.

#### 1.2 Required CPU Features

The supervisor halts during initialization unless the processor reports:

- MTRRs, SMRRs and the extended SMRR capability
- SMM Code Access Check, which the supervisor enables and locks on every core
- UMIP, SMEP and SMAP, which must be set in CR4 when each core enters an MMI

> **Guidance:**
> These features keep MMRAM out of reach of non-MM code and stop Ring 0 from running or reading Ring 3 memory by
> mistake. There is no fallback for processors without them.

#### 1.3 Every Core Joins Every MMI

Every core the MP Information HOB reports must enter every MMI. The BSP waits up to one second for every AP to arrive
before serving a request, and up to one second for every AP to leave afterwards. A core that misses either window
halts the system.

> **Guidance:**
> Deliver SMIs to all cores (broadcast SMIs). The supervisor never serves a request with only part of the system in
> MM.

#### 1.4 Save State Layout

The supervisor reads the Intel x64 save state layout by default. Platforms whose save state uses the AMD64 layout
enable the `save_state_amd` feature. Pick the layout the platform presents, which is not always the CPUID vendor.

### 2. MMRAM Requirements

#### 2.1 MMRAM Forms One Contiguous Span

MMRAM is described by `EFI_SMRAM_DESCRIPTOR` entries in GUID HOBs named `gEfiSmmSmramMemoryGuid` or
`gEfiMmPeiMmramMemoryReserveGuid`; the supervisor combines the descriptors of every such HOB. A descriptor whose
`RegionState` has `EFI_ALLOCATED` set is already in use, so its pages are counted as supervisor pages and never
allocated.

The descriptors may split MMRAM into several regions, in any order. The regions must not overlap and must be
adjacent with no gaps, so together they form one contiguous span. The span must start at or above 1 MiB and hold the
supervisor image. The supervisor treats the span as MMRAM for its checks and the SMRRs, but no single allocation
crosses a region boundary.

#### 2.2 MMRAM Must Fit the SMRRs

The SMRRs protect all of MMRAM, so the range must be a power of two in size, at least 4 KiB, aligned to its size, and
end at or below 4 GiB.

> **Guidance:**
> Size and place TSEG or the MMRAM carve-out so one SMRR pair covers it exactly. The supervisor does not split MMRAM
> across several SMRR ranges.

#### 2.3 MSEG Is Not Present

When the platform publishes an MSEG SMRAM HOB, MSEG must be non-empty, page aligned, below 4 GiB, and not present in
the MM page table.

### 3. MM Init Requirements

The supervisor adopts the page table, GDT, TSS, stacks and MMI entry code MM Init leaves behind. It checks them instead
of building its own.

#### 3.1 Use the V5 MMI Entry

Each core must enter through the version 5 MMI entry code with its fixups filled in: the live CR3, the supervisor
entry point, the HOB list, the core's SMBASE, XD enabled, and the debug, CET and reserved slots zero.

#### 3.2 Per-Core Descriptor Tables and Stacks

For every core:

- The GDT, call gate and TSS match the supervisor's template, and the TSS directly follows the GDT.
- The TSS has no I/O permission bitmap, so every port access from Ring 3 faults.
- The GDT and TSS are mapped supervisor-only and read-only.
- The IST1 stack has a guard page that is not present.
- The top of the Ring 0 stack holds the core's index.
- No two cores share a GDT, a GDT descriptor, a Ring 0 stack or an IST1 stack, and every core loads the same CR3.

#### 3.3 Page Table Contents

Every mapping is an identity mapping. Page 0 and everything at or above the physical address width are not present.
No page is both writable and executable.

Inside MMRAM, these regions must be mapped as listed. S is supervisor-only, U user-accessible.

| Region                                                                       | Required                                                |
| ---------------------------------------------------------------------------- | ------------------------------------------------------- |
| Supervisor core image                                                        | S, present; entry point on a read-only, executable page |
| User core and MM driver images                                               | U, present; entry point on a read-only, executable page |
| MM Init image                                                                | S, present                                              |
| Each core's MMI entry code (`SMBASE + 0x8000`)                               | S, read-only, executable                                |
| Each core's SMM descriptor and save state (`SMBASE + 0xFB00` to `+ 0x10000`) | S, non-executable                                       |
| Firmware security policy                                                     | S, read-only, non-executable                            |
| SMBASE array                                                                 | S, non-executable                                       |
| MM-initialized buffer                                                        | S, writable, non-executable                             |
| Ring 3 stacks                                                                | U, writable, non-executable                             |
| Everything else in MMRAM                                                     | S, non-executable                                       |

User-accessible pages may lie only in the user core, an MM driver or the Ring 3 stacks. Executable pages may lie only
in the supervisor core, the user core, an MM driver or the MMI entry code.

Outside MMRAM, a page may be present only if a HOB names it, with these attributes:

| Source                                        | Required                                                    |
| --------------------------------------------- | ----------------------------------------------------------- |
| MMIO and firmware-device resource descriptors | U, non-executable                                           |
| Unblock region HOBs                           | Non-executable; S exactly when the HOB sets `EFI_MEMORY_SP` |
| Communication buffers and status mailboxes    | S, non-executable                                           |
| IOMMU protected regions                       | U, read-only, non-executable                                |

All other memory outside MMRAM, including system memory, must be not present.

#### 3.4 MM Init Housekeeping

The page table checks above fail unless MM Init also:

- frees its own copies of the MM driver and user core images before it takes the final memory map;
- allocates no internal copy of the user communication buffer;
- makes every heap guard page not present before hand off, when heap guard is enabled;
- allocates the SMBASE array with whole pages.

> **Guidance:**
> These are properties of the MM Init that ships with the MM Supervisor. A platform that brings its own MM Init must
> meet them. Each failed check names the region and the attributes it found.

### 4. Hand Off Block (HOB) Requirements

#### 4.1 HOB List

The HOB list must start with the PHIT, be well formed, end within 1 MiB, and lie inside MMRAM.

#### 4.2 Required HOBs

| HOB                           | Requirement                                                                           |
| ----------------------------- | ------------------------------------------------------------------------------------- |
| SMRAM reserve HOB             | Describes MMRAM; GUIDs in [2.1](#21-mmram-forms-one-contiguous-span)                  |
| MP Information HOB            | Present, inside MMRAM, well formed, and reports at least one core                     |
| MM Supervisor `PassDown` HOB  | Revision 2; see [4.5](#45-passdown-hob)                                               |
| Memory allocation module HOBs | MM Init, supervisor core and user core each described exactly once; one per MM driver |
| MM Communication Buffer HOB   | The user communication buffer and its status mailbox                                  |
| MM Common Region HOB          | The supervisor communication buffer and its status mailbox                            |
| Resource descriptor HOBs      | See [4.4](#44-resource-descriptor-hob-v2)                                             |

The CPU HOB, MSEG SMRAM HOB, unblock region HOBs and protected region HOBs are optional.

#### 4.3 Memory Allocation HOBs

- At most 64 module allocation HOBs.
- Each module lies inside MMRAM, is page aligned, overlaps no other module, and holds its own entry point.
- No other memory allocation HOB overlaps MMRAM.
- An MM driver's allocation HOB may be followed by a GUID HOB that carries its dependency expression. A driver without
  one is dispatched without waiting on protocols.

#### 4.4 Resource Descriptor HOB v2

- Use resource descriptor HOB v2. A memory descriptor carries exactly one cacheability attribute and never
  `EFI_MEMORY_UCE`. An I/O descriptor carries no attributes.
- No two descriptors overlap in the same address space.
- One system memory or reserved memory descriptor holds all of MMRAM, so no MMIO or firmware device descriptor reaches
  into it.

> **Guidance:**
> V1 descriptors are ignored, as in the Patina DXE Core. A platform that still produces V1 descriptors can enable the
> temporary `v1_resource_descriptor_support` feature, which checks V1 descriptors instead.

#### 4.5 PassDown HOB

- The revision is 2.
- The Ring 3 stacks, SMBASE array, MM-initialized buffer and firmware policy lie inside MMRAM, in pages MM Init marked
  allocated, sized for the core count.
- The Ring 3 stacks start and end on page boundaries, and the SMBASE array starts on one.
- These buffers, the module images, the HOB list, MSEG and the SMBASE area never share a page.
- No core's MMI entry code, or SMM descriptor and save state, overlaps another core's.

#### 4.6 Communication Buffers

Both communication buffers and both status mailboxes must lie entirely outside MMRAM, at a non-zero address, and be
mapped supervisor-only. The supervisor copies each request into MMRAM before any handler reads it.

#### 4.7 Unblock and Protected Region HOBs

Each region is non-empty, page aligned and entirely outside MMRAM. Unblock regions name their requester with a
non-zero GUID. Protected regions must be of the IOMMU type.

#### 4.8 CPU HOB

When present, its physical address width must not exceed the processor's MAXPHYADDR.

### 5. Security Policy Requirements

The firmware security policy decides which I/O ports, MSRs, privileged instructions and save state registers Ring 3
may reach. Each policy root is an allow list or a deny list.

Build the policy binary from an XML description (see [Policy Blob](../mm/security_policy.md#policy-blob)) and place it
in the same firmware volume as the supervisor, as a FREEFORM file named `gMmSupervisorPolicyFileGuid`:

```text
FILE FREEFORM = gMmSupervisorPolicyFileGuid {
  SECTION RAW = $(MM_POLICY_BIN_PATH)
}
```

MM Init finds the file by that GUID, copies the policy into MMRAM and passes its address and size in the PassDown HOB.
The supervisor reads only that copy.

The policy must meet these rules:

- The policy is version 1.0. The header, the root array and every descriptor array are aligned for their types and
  lie inside the declared size, which equals the size of the header, roots and descriptors.
- Every root has a type the supervisor recognizes, and each type appears at most once.
- Every reserved, flags and capabilities field is zero.
- I/O ranges are non-empty and end at or below port `0xFFFF`.
- Save state descriptors grant read access only. A descriptor uses conditional read exactly when it names a condition.
- The policy carries no legacy memory policy. The supervisor builds the memory policy itself from the page table at
  ready to lock, or at the first `FETCH_POLICY` request if that comes first.
- The policy carries a `TYPE_MEM` root. `FETCH_POLICY` points it at that memory policy; without it the request fails
  with `EFI_DEVICE_ERROR`.

> **Guidance:**
> The policy is checked once, during initialization. A malformed policy halts the boot.

### 6. MM Driver Requirements

MM drivers run in Ring 3 under the user core. Code written for Ring 0 Standalone MM needs the changes below.

#### 6.1 No Privileged Instructions

Ring 3 cannot run `in`, `out`, `rdmsr`, `wrmsr`, `cli`, `wbinvd` or `hlt`. Drivers ask the supervisor through
syscalls, which the security policy gates:

| Need                         | Syscall                                                            |
| ---------------------------- | ------------------------------------------------------------------ |
| Read or write an MSR         | `RdMsr`, `WrMsr`                                                   |
| Read or write an I/O port    | `IoRead`, `IoWrite`                                                |
| Run `cli`, `wbinvd` or `hlt` | `Cli`, `Wbinvd`, `Hlt`                                             |
| Read a save state register   | `SaveStateRead`, `SaveStateRead2` (through `EFI_MM_CPU_PROTOCOL`)  |
| Allocate or free pages       | `AllocPage`, `FreePage` (through `MmAllocatePages`, `MmFreePages`) |
| Run a procedure on an AP     | `StartApProc`                                                      |
| Check a buffer outside MMRAM | `MmMemoryUnblocked`, `MmIsCommBuffer`                              |

> **Guidance:**
> Build drivers with the syscall based library instances that ship with the MM Supervisor. A privileged instruction
> in Ring 3 faults and halts the system.

#### 6.2 Image Layout

Driver images must have page aligned sections, with no section both writable and executable, and the entry point on a
read-only, executable page. MM Init loads every driver before MM starts; the user core does not load images.

#### 6.3 MM System Table Differences

| Service            | Behavior                                                 |
| ------------------ | -------------------------------------------------------- |
| `MmAllocatePages`  | Only `AllocateAnyPages`; the memory type is ignored      |
| `MmAllocatePool`   | At least eight byte alignment; the pool type is ignored  |
| `MmFreePool`       | Accepts only pointers `MmAllocatePool` returned          |
| `MmLocateHandle`   | Only `AllHandles` and `ByProtocol`                       |
| `MmIo` (`MmCpuIo`) | Returns `EFI_UNSUPPORTED`; use the I/O syscalls          |
| `MmStartupThisAp`  | Returns `EFI_UNSUPPORTED`; use the `StartApProc` syscall |

#### 6.4 Save State Access

`EFI_MM_CPU_PROTOCOL` is produced by the `patina_mm_cpu` component in the user core. `ReadSaveState` goes through the
supervisor, which applies the save state policy. `WriteSaveState` returns `EFI_UNSUPPORTED`.

#### 6.5 Memory Outside MMRAM

Drivers can reach only the memory outside MMRAM that the page table maps for Ring 3: MMIO from resource descriptors,
unblock regions, and protected regions read-only. Before using a pointer from a caller, a driver confirms the range:

- `MmMemoryUnblocked` reports whether it lies in an unblocked region outside MMRAM. It never reports a range that
  touches MMRAM.
- `MmIsCommBuffer` reports whether it lies in the user communication buffer. Handlers receive the supervisor's copy of
  that buffer in MMRAM, not the buffer outside MM.

#### 6.6 AP Procedures

Only the BSP may start an AP procedure, the procedure and its argument must lie in Ring 3 memory, and the BSP waits for
it to finish. AP procedures run in parallel without synchronization, so they must not touch shared data.

### 7. Non-MM Interface Requirements

#### 7.1 Supervisor Requests

Requests to the supervisor go through the supervisor communication buffer with `MM_SUPERVISOR_REQUEST_HANDLER_GUID`:

| Request        | Behavior                                                                                                               |
| -------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `VERSION_INFO` | Returns the supervisor version                                                                                         |
| `FETCH_POLICY` | Returns the security policy; the first request also locks the memory policy                                            |
| `UNBLOCK_MEM`  | Maps a page aligned region outside MMRAM for MM; accepted only before ready to lock, and only for pages not mapped yet |
| `COMM_UPDATE`  | Returns `EFI_ACCESS_DENIED`; communication buffers cannot change after boot                                            |

#### 7.2 Signal Ready to Lock Before Exit Boot Services

The `MM_DXE_READY_TO_LOCK_PROTOCOL_GUID` request on the supervisor channel snapshots the memory policy and stops
further unblock requests. After the Exit Boot Services request, the supervisor refuses every supervisor channel
request.

> **Guidance:**
> Send ready to lock before Exit Boot Services, or the memory policy is never locked.

### 8. Known Limitations

| Limitation                                  | Detail                                                                                     |
| ------------------------------------------- | ------------------------------------------------------------------------------------------ |
| Paging structures                           | Only the page table root is checked to lie inside MMRAM, not every table page              |
| Paging pool                                 | Page table pages come from a fixed pool of 128 pages                                       |
| Unblocked regions                           | At most 64                                                                                 |
| Communication buffers                       | Fixed at boot; `COMM_UPDATE` is not supported                                              |
| `MmStartupThisAp`, `MmIo`, `WriteSaveState` | Not supported, see [6.3](#63-mm-system-table-differences) and [6.4](#64-save-state-access) |
