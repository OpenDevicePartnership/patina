# Patina Requirements

The Patina DXE Core has several functional and implementation differences from the
[Platform Initialization (PI) Spec](https://uefi.org/specifications) and EDK2 DXE Core implementation.

The [Patina DXE Readiness Tool](#todo) validates many of these requirements.

## Platform Requirements

Platforms should ensure the following specifications are met when transitioning over to the Patina DXE core:

### Dispatcher Requirements

The following are the set of requirements the Patina DXE Core has in regard to firmware volumes.

#### No Traditional SMM

Traditional System Management Mode (SMM) is not supported in Patina. Standalone MM is supported.

Traditional SMM is not supported to prevent coupling between the DXE and MM environments. This is error
prone, unnecessarily increases the scopes of DXE responsibilities, and can lead to security vulnerabilities.
Standalone MM should be used instead. The combined drivers have not gained traction in actual implementations due
to their lack of compatibility for most practical purposes, increased likelihood of coupling between core environments,
and user error when authoring those modules. The Patina DXE Core focuses on modern use cases and simplification of the
overall DXE environment.

This specifically means that the following SMM module types that require cooperation between the SMM and DXE
dispatchers are not supported:

- `EFI_FV_FILETYPE_SMM` (`0xA`)
- `EFI_FV_FILETYPE_SMM_CORE` (`0xD`)

Further, combined DXE modules will not be dispatched. These include:

- `EFI_FV_FILETYPE_COMBINED_PEIM_DRIVER` (`0x8`)
- `EFI_FV_FILETYPE_COMBINED_SMM_DXE` (`0xC`)

DXE drivers and Firmware volumes **will** be dispatched:

- `EFI_FV_FILETYPE_DRIVER` (`0x7`)
- `EFI_FV_FILETYPE_FIRMWARE_VOLUME_IMAGE` (`0xB`)

Because Traditional SMM is not supported, events such as the `gEfiEventDxeDispatchGuid` defined in the PI spec and used
in the EDK2 DXE Core to signal the end of a DXE dispatch round so SMM drivers with DXE dependency expressions could be
reevaluated will not be signaled.

Dependency expressions such as `EFI_SECTION_SMM_DEPEX` will not be evaluated on firmware volumes.

The use of Traditional SMM and combined drivers is detected by the Patina DXE Core Readiness Tool, which will report
this as an issue requiring remediation before Patina can be used.

Additional resources:

- [Standalone MM Information](https://github.com/microsoft/mu_feature_mm_supv/blob/main/Docs/TraditionalAndStandaloneMm.md)
- [Traditional MM vs Standalone MM Breakdown](https://github.com/microsoft/mu_feature_mm_supv/blob/main/Docs/TraditionalAndStandaloneMm.md)
- [Porting to Standalone MM](https://github.com/microsoft/mu_feature_mm_supv/blob/main/MmSupervisorPkg/Docs/PlatformIntegration/PlatformIntegrationSteps.md#standalone-mm-changes)

> **Guidance:**
> Platforms must transition to Standalone MM (or not use MM at all, as applicable) using the provided guides. All
> combined modules must be dropped in favor of single phase modules.

#### A Priori Driver Dispatch Is Not Allowed

The Patina DXE Core does not support A Priori driver dispatch as described in the PI spec and supported in edk2. See
the [Dispatcher Documentation](../dxe_core/dispatcher.md) for details and justification.

> **Guidance:**
> A Priori sections must be removed and proper driver dispatch must be ensured using depex statements. Drivers may
> produce empty protocols solely to ensure that other drivers can use that protocol as a depex statement, if required.

### Hand Off Block (HOB) Requirements

The following are the Patina DXE Core HOB requirements.

#### Resource Descriptor HOB v2

Patina uses the
[Resource Descriptor HOB v2](https://github.com/microsoft/mu_rust_pi/commit/4e5d3840f199a36c7c3b112790f1a88570b3aa22),
which is in process of being added to the PI spec, instead of the
[EFI_HOB_RESOURCE_DESCRIPTOR](https://uefi.org/specs/PI/1.9/V3_HOB_Code_Definitions.html#resource-descriptor-hob).

Platforms need to exclusively use the Resource Descriptor HOB v2 and not EFI_HOB_RESOURCE_DESCRIPTOR. Functionally,
this just requires adding an additional field to the v1 structure that describes the cacheability attributes to set on
this region.

Patina requires cacheability attribute information for memory ranges because it implements full control of memory
management and cache hierarchies in order to provide a cohesive and secure implementation of memory protection. This
means that pre-DXE paging/caching setups will be superseded by Patina and Patina will rely on the Resource Descriptor
HOB v2 structures as the canonical description of memory rather than attempting to infer it from page table/cache
control state.

Patina will ignore any EFI_HOB_RESOURCE_DESCRIPTORs. The Patina DXE Readiness Tool verifies that all
EFI_HOB_RESOURCE_DESCRIPTORs produced have a v2 HOB covering that region of memory and that all of the
EFI_HOB_RESOURCE_DESCRIPTOR fields match the corresponding v2 HOB fields for that region.

The Readiness Tool also verifies that a single valid cacheability attribute is set in every Resource Descriptor HOB v2.
The accepted attributes are EFI_MEMORY_UC, EFI_MEMORY_WC, EFI_MEMORY_WT, EFI_MEMORY_WB, and EFI_MEMORY_WP.
EFI_MEMORY_UCE, while defined as a cacheability attribute in the UEFI spec is not implemented by modern architectures
and so is prohibited. The Readiness Tool will fail if EFI_MEMORY_UCE is present in a v2 HOB.

> **Guidance:**
> Platforms must produce Resource Descriptor HOB v2s with a single valid cacheability attribute set. These can be the
> existing Resource Descriptor HOB fields with the cacheability attribute set as the only additional field in the v2
> HOB.

#### Overlapping HOBs Prohibited

Patina does not allow there to be overlapping Resource Descriptor HOB v2s in the system and the Readiness Tool will
fail if that is the case. Patina cannot choose which HOB should be valid for the overlapping region; the platform must
decide this and correctly build its resource descriptor HOBs to describe system resources.

The EDK2 DXE CORE silently ignores overlapping HOBs, which leads to unexpected behavior when a platform believes both
HOBs or part of both HOBs, is being taken into account.

> **Guidance:**
> Platforms must produce non-overlapping HOBs by splitting up overlapping HOBs into multiple HOBs and eliminating
> duplicates.

#### No Memory Allocation HOB for Page 0

Patina does not allow there to be a memory allocation HOB for page 0. The EDK2 DXE Core allows page 0 allocates. Page 0
must be unmapped in the page table to catch null pointer dereferences and this cannot be safely done if a driver has
allocated this page. The Readiness Tool will fail if a Memory Allocation HOB is discovered that covers page 0.

> **Guidance:**
> Platforms must not allocate page 0.

### Miscellaneous Requirements

This section details requirements that do not fit under another category.

#### Exit Boot Services Memory Allocations Are Not Allowed

When `EXIT_BOOT_SERVICES` is signaled, the memory map is not allowed to change. See
[Exit Boot Services Handlers](../dxe_core/memory_management.md#exit-boot-services-handlers). The EDK2 DXE Core does not
prevent memory allocations at this point, which causes hibernate resume failures, among other bugs.

The Readiness Tool is not able to detect this anti-pattern because it requires driver dispatching and specific target
configurations to trigger the memory allocation/free.

> **Guidance:**
> Platforms must ensure all memory allocations/frees take place before exit boot services callbacks.

### Temporary Requirements

This section details requirements Patina currently has due to limitations in implementation, but that support will be
added for in the future.

#### LZMA Compressed Section Support Is Not Yet Implemented

The Patina DXE Core has not added LZMA decompression functionality yet, so currently these sections cannot be processed
and must be converted to one of the support algorithms: Brotli or TianoCompress.

In practice, PEI decompresses most sections (when present), so this is not a large limitation and support will be added.

> **Guidance:**
> Temporarily, LZMA compressed sections that will be decompressed in DXE should use Brotli or TianoCompress.
