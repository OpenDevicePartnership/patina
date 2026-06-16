# RFC: Boot Orchestration

This RFC proposes modular components for UEFI-compliant boot functionality in Patina firmware.

Within the scope of this RFC, boot functionality includes console initialization, boot option discovery,
and boot orchestration as defined in UEFI Specification 2.11 Chapter 3 (Boot Manager). This RFC also
includes BDS phase event signaling (EndOfDxe and ReadyToBoot) as defined in the Platform Initialization
(PI) Specification.

## Change Log

- 2025-11-04: Initial RFC draft
- 2025-11-06:
  - Rewritten with modular component architecture
  - Added optional service dependencies for platform customization
  - Added BDS phase event signaling (EndOfDxe, ReadyToBoot)
- 2025-12-04:
  - Updated watchdog timer reference to UEFI Specification 2.11 Section 3.1.2
  - Added requirement to disable watchdog when boot option returns control
- 2025-12-09:
  - Added component rationales and decomposition justification
- 2025-12-10:
  - Fixed EndOfDxe timing: now signaled after PCIe enumeration AND GOP discovery complete
  - Added Future Work section for Connect Controller Service
  - Expanded ConsoleDiscovery prerequisites (PCI/USB bus enumeration requirements)
  - Added custom orchestrator example
- 2025-12-12:
  - Added BootOptionDiscovery component for full UEFI boot variable management
  - Reframed RFC: UEFI-compliant boot as primary case, hardcoded paths as alternative for divergent platforms
- 2025-12-18:
  - Added Unresolved Questions section for BDS phase dispatch mechanism
- 2026-01-14:
  - Resolved BDS phase dispatch mechanism: dual integration paths (native Patina and legacy bridge)
- 2026-02-03:
  - Added partial device path expansion support with `expand_device_path()` helper (#1280)
  - `boot_from_device_path()` now handles both full and partial paths transparently
- 2026-02-10:
  - Added Implementation Phases section defining Minimal Boot as first deliverable
  - Minimal Boot specifies two boot flows: normal (ordered fallback chain) and hotkey (direct to error on failure)
  - Reordered Guide-Level Explanation to lead with Minimal Boot usage
- 2026-02-10:
  - Added Implementation Phases section defining Minimal Boot as first deliverable
  - Minimal Boot demonstrates patina_boot booting Windows with a shorter BDS phase
  - Reordered Guide-Level Explanation to lead with Minimal Boot usage
- 2026-02-12:
  - Refactored BootOrchestrator from concrete component to trait-based design
  - Added `BootOrchestrator` trait, `BootDispatcher` component, `SimpleBootManager` reference impl
  - Boot options passed directly to `SimpleBootManager::new()` instead of `Config<BootConfig>`
  - `BootDispatcher` is the Patina component gated by `Service<dyn BdsPhase>`
  - Platforms provide custom boot flows by implementing `BootOrchestrator` trait
- 2026-03-31:
  - Updated RFC to match implementation: `BootConfig` naming, `DxeDispatch` service,
    BDS architectural protocol installation, import paths
  - Removed Phase 1/Phase 2 split — core framework fully implemented
  - `BootOptionDiscovery` implemented as `discover_boot_options()` helper function
    instead of a separate component (simpler, no framework machinery needed)
  - `interleave_connect_and_dispatch()` made private to `SimpleBootManager`
    (uses `connect_all()` which is not recommended for most platforms)
- 2026-06-16: RFC rejected:
  - The design is being refined and tested outside the Patina project
  - Planned for future reevaluation; moved to the rejected folder as a suspended state

## Motivation

Most Patina platforms require UEFI-compliant boot functionality: discovering boot options from
`Boot####`/`BootOrder`/`BootNext` variables, managing console devices, and orchestrating the boot
flow per UEFI Specification Chapter 3. This RFC
provides common components that implement full UEFI Boot Manager compliance.

Some platforms may diverge from UEFI compliance (e.g., embedded systems with hardcoded boot paths). The modular
architecture supports this by allowing platforms to use individual components selectively or provide boot options
directly via configuration.

## Technology Background

The UEFI Boot Manager (UEFI Specification 2.11 Chapter 3) operates through UEFI variables (`Boot####`, `BootOrder`,
`BootNext`, `ConIn`/`ConOut`/`ErrOut`, `ConInDev`/`ConOutDev`/`ErrOutDev`) containing boot configuration
and console device paths. Boot options use `EFI_LOAD_OPTION`
structures with device paths and descriptions. The BDS phase signals EndOfDxe after DXE completion and ReadyToBoot
before boot attempts to coordinate firmware initialization and security lockdown.

## Goals

1. Provide UEFI-compliant boot option discovery (`Boot####`/`BootOrder`/`BootNext` variable management)
2. Provide foundational boot orchestration for executing boot options
3. Provide a common boot flow implementation suitable for most platforms
4. Support platform-specific boot flows through custom orchestration
5. Maintain modular architecture where components can be used, replaced, or omitted independently
6. Provide reusable library functions to minimize code duplication across platforms

## Requirements

This RFC implements specific elements from UEFI Specification 2.11 Chapter 3 (Boot Manager) and Platform
Initialization (PI) Specification Volume 2 (DXE/BDS phase). The following requirements define the scope
of boot functionality provided by these components.

### UEFI Specification 2.11 Chapter 3 (Boot Manager)

**Boot Option Discovery**:

- Read and parse `Boot####` variables containing `EFI_LOAD_OPTION` structures
- Read `BootOrder` variable to determine boot attempt sequence
- Check `BootNext` variable to override boot sequence for a single boot (UINT16 referencing a
  `Boot####` option; deleted after consumption per Section 3.1.2)
- Manage boot option list (add, remove, reorder boot options)
- Support boot option attributes (LOAD_OPTION_ACTIVE, LOAD_OPTION_HIDDEN, etc.)
- Expand partial device paths to full device paths by matching against discovered device topology
  (e.g., hard drive media device paths containing only partition GUID/signature)

**Console Variables**:

- Discover console devices via GOP (graphics) and SimpleTextInput protocols
- Populate `ConIn`, `ConOut`, and `ErrOut` variables with console device paths (saved to NVRAM,
  represent default/selected consoles)
- Populate `ConInDev`, `ConOutDev`, and `ErrOutDev` volatile variables with all discovered console
  device paths each boot (represent all available console devices found during enumeration)

**Boot Orchestration**:

- If `BootNext` is set, attempt that boot option first and delete the variable regardless of outcome
- Execute boot options from `BootOrder` sequence (or platform-provided configuration)
- Load boot images via `LoadImage()` service
- Start boot images via `StartImage()` service
- Enable 5-minute watchdog timer before `StartImage()` per Section 3.1.2
- Disable watchdog timer when boot option returns control
- Handle boot failures and attempt next option in sequence
- Detect hotkey via `Key####` variables (UEFI-compliant) or platform-configured scancode to trigger
  alternative boot path (e.g., F12 for secondary boot)
- Read `Key####` variables containing `EFI_KEY_OPTION` structures to determine hotkey-to-boot-option
  mappings per Section 3.1.6
- Attempt `PlatformRecovery####` boot options (in `PlatformRecoveryOrder` sequence) when all normal
  boot options and platform-configured fallback chain are exhausted, per Section 3.4

### PI Specification Volume 2 (DXE/BDS Phase)

**BDS Phase Event Signaling**:

- Signal `gEfiEndOfDxeEventGroupGuid` after device enumeration completes
- Signal `gEfiEventReadyToBootGuid` immediately before attempting first boot option
- Enable event notification to C-based PI drivers (MM, security) via EventServices

**Driver Dispatch Coordination**:

- Coordinate driver dispatch during device connection to ensure complete device topology enumeration
- Support interleaved connect-dispatch pattern: connect controllers, dispatch newly loaded drivers, repeat until stable

### Component Integration

**Patina Component Model**:

- Integrate with Patina's component model using standard dependency injection patterns
- Enable platform orchestration that controls execution order and can skip or replace steps
- Support progressive customization from default behavior to full custom orchestration

**Service Dependencies** (from RFC 0017):

- `RuntimeVariableServices`: Read/write UEFI variables (boot options, console variables)
- `ImageServices`: Load and start boot images (`LoadImage()`, `StartImage()`)
- `TimingServices`: Configure watchdog timer
- `ProtocolServices`: Locate protocol handles, connect controllers, query device paths
- `DxeDispatch`: Orchestrate driver dispatch during connect-dispatch interleaving
- `EventServices`: Signal BDS phase events (`signal_event_group()` for event group GUIDs)

*Note: This RFC uses `ProtocolServices::connect_controller()` for device connection. If future
components require additional connection abstractions (e.g., connection filtering, connection
policies), a dedicated `ConnectionService` could be introduced. For this RFC's scope,
`ProtocolServices` is sufficient.*

**Customization Support**:

- Default: Use `BootOptionDiscovery` for UEFI-compliant boot variable management
- Alternative: Accept boot options via `BootConfig` passed directly to
  `SimpleBootManager::new()` for platforms not requiring UEFI compliance
- Enable platforms to use common components as-is, compose selectively, or replace entirely
- Provide library functions for platforms implementing custom boot flows

### Planned Extensions

The following features are natural extensions of the current implementation and may be
added as helper functions or orchestrator enhancements:

- **Boot menu UI**: Boot timeout countdown and boot option selection UI, using
  options from `discover_boot_options()`. Platforms with GOP consoles could display
  a menu; headless platforms could use serial console.
- **OS indications**: `OsIndications`/`OsIndicationsSupport` for boot-to-firmware-UI
  requests and other OS-to-firmware communication.

### Out of Scope

The following features are outside the scope of boot orchestration and belong in
separate subsystems:

- Capsule processing and update capsule handling
- Language management (`PlatformLang`, `PlatformLangCodes`)
- Variable policy registration (`EDKII_VARIABLE_POLICY_PROTOCOL`)
- Hardware error record variables (`HwErr####`, `HwErrRecSupport`)
- Deferred image loading (`EFI_DEFERRED_IMAGE_LOAD_PROTOCOL`)
- Automatic boot device provisioning UI

### Planned: Connect Controller Service

This RFC currently uses `connect_controller()` (the UEFI boot service) for device
enumeration. This works but has limitations: `connect_all()` connects every controller
on every round (inefficient for large topologies), and native Patina components cannot
participate in the connect controller flow.

A `Service<dyn ConnectController>` abstraction would allow:

- Selective controller connection (e.g., PCI-only, skip USB for headless)
- Native Patina bus drivers participating in device discovery
- Connection filtering and policy components
- Custom binding logic for platform-specific device handling
- Replacing `connect_all()` inside `interleave_connect_and_dispatch()` with
  platform-appropriate connection strategies

This is the primary remaining gap between the current implementation and production
readiness for platforms with complex device topologies. Until implemented, platforms
requiring custom connect controller behavior must implement custom `BootOrchestrator`
traits that call `connect_controller()` directly with platform-specific logic.

## Design Decision: BDS Phase Dispatch Mechanism

The PI Specification (Section II-12.2) defines the BDS Architectural Protocol as the mechanism for
transferring control from DXE phase to BDS phase. This RFC adopts a Patina-native approach while
providing a bridge component for platforms with existing C-based BDS implementations.

### Chosen Approach: Dual Integration Paths

Patina provides two integration paths, allowing platforms to choose based on their needs:

#### Path 1: Native Patina (BootDispatcher + BootOrchestrator trait)

Boot orchestration uses a trait-based design. `BootOrchestrator` is a plain Rust trait defining the
boot flow interface. `BootDispatcher` is the Patina component that installs the BDS architectural protocol and
holds a `Box<dyn BootOrchestrator>`, invoking it when the DXE core calls BDS. `SimpleBootManager` is the reference
implementation.

```rust
/// Trait defining the boot orchestration interface.
pub trait BootOrchestrator: Send + Sync + 'static {
    fn execute(
        &self,
        boot_services: &StandardBootServices,
        runtime_services: &StandardRuntimeServices,
        dxe_services: &dyn DxeDispatch,
        image_handle: efi::Handle,
    ) -> Result<!, EfiError>;
}

/// Component that invokes the orchestrator at BDS time.
#[component]
impl BootDispatcher {
    fn entry_point(
        self,
        boot_services: StandardBootServices,
        runtime_services: StandardRuntimeServices,
        dxe_services: Service<dyn DxeDispatch>,
        image_handle: Option<Handle>,
    ) -> Result<()> {
        let handle = *image_handle.ok_or(EfiError::InvalidParameter)?;
        self.orchestrator.execute(&boot_services, &runtime_services, *dxe_services, handle);
        Ok(())
    }
}

/// Reference implementation with configurable boot options.
impl BootOrchestrator for SimpleBootManager {
    fn execute(&self, boot_services: &StandardBootServices, runtime_services: &StandardRuntimeServices, dxe_services: &dyn DxeDispatch, image_handle: efi::Handle) -> Result<!, EfiError> {
        interleave_connect_and_dispatch(boot_services, dxe_services)?;
        signal_bds_phase_entry(boot_services)?;
        discover_console_devices(boot_services, runtime_services)?;
        // Hotkey detection, ReadyToBoot signal, boot attempts...
    }
}
```

*Benefits*: Trait enforces the contract at compile time. Plain Rust trait with `Box<dyn>` dispatch —
no Patina service indirection needed since there is only one consumer (`BootDispatcher`). Platforms
swap boot orchestration by providing a different `BootOrchestrator` impl. Single component
registration: `BootDispatcher::new(impl BootOrchestrator)`.

#### Path 2: Legacy/Hybrid (BdsArchProtocolBridge)

For platforms with existing C-based BDS implementations (e.g., EDK II's `BdsDxe`), a minimal bridge
component calls the BDS Architectural Protocol. This enables incremental migration and keeps Patina
agnostic of what owns BDS.

```rust
/// Minimal component that bridges to existing BDS arch protocol
#[component]
impl BdsArchProtocolBridge {
    fn entry_point(self, protocol_services: Service<dyn ProtocolServices>) -> Result<()> {
        let bds_protocol = protocol_services.locate_protocol::<bds::Protocol>()?;
        unsafe { ((*bds_protocol).entry)(bds_protocol) };
        Ok(())
    }
}
```

*Benefits*: Works with existing C-based BDS drivers, no DXE Core modification required, clean
migration path for platforms transitioning to native Patina boot.

### Rationale

The BDS Architectural Protocol is a *dispatch mechanism*, not a functional requirement. The PI spec
requires that BDS phase enumerate boot devices, signal events, and attempt boot options—requirements
about *what* happens, not *how* it's dispatched.

Providing both paths:

1. Allows native Patina platforms to use idiomatic component patterns
2. Supports platforms with existing C-based BDS investments
3. Keeps Patina decoupled from any specific BDS implementation
4. Enables gradual migration without forcing immediate rewrites

C drivers that register for BDS events (EndOfDxe, ReadyToBoot) via `CreateEventEx()` continue to
work because both paths signal these events through EventServices.

## Prior Art (Existing EDK II Implementation)

EDK II implements boot functionality through `BdsDxe` (`MdeModulePkg/Universal/BdsDxe`), a monolithic
driver that combines console initialization, boot option processing, and platform integration in a single
component. Platform customization occurs through `PlatformBootManagerLib` callbacks:
`PlatformBootManagerBeforeConsole()`, `PlatformBootManagerAfterConsole()`,
`PlatformBootManagerWaitCallback()`, and `PlatformBootManagerUnableToBoot()`.

Boot option management uses `UefiBootManagerLib`, which provides functions for parsing `EFI_LOAD_OPTION`
structures, enumerating boot options, and managing boot variables. Console device connection and boot
target discovery are hardcoded into the boot flow with limited platform control over execution order.

This design influenced our component architecture by demonstrating the need for console initialization,
boot discovery, and orchestration as distinct responsibilities. However, the callback pattern limits
platform flexibility compared to platform-controlled orchestration.

## Alternatives

### Callback-Based Pattern (EDK II Approach)

Single Boot Manager component with fixed execution flow and platform callback hooks (`before_console()`,
`after_console()`, etc.). Platforms customize behavior through callback traits.

**Limitation**: Callbacks have fixed execution order. Platforms cannot skip steps, change ordering, or
implement custom control flow like network-before-console or interleaved fallbacks.

### Monolithic Boot Manager Component

Single comprehensive component handling console initialization, boot discovery, and orchestration with
configuration options.

**Limitation**: Individual pieces cannot be replaced independently of the reference implementation.

### Library-Only Approach

Library functions only (`parse_boot_variables()`, `discover_console_devices()`, etc.) with no reference
components. Platforms implement orchestration.

**Limitation**: Code duplication across platforms. Simple platforms implement significant boilerplate.
Higher adoption barrier.

### Chosen Approach: Modular Components with Platform Orchestration

Three components (`BootOptionDiscovery`, `ConsoleDiscovery`, `BootDispatcher`) that platforms use
as-is, compose selectively, or replace entirely. `BootDispatcher` invokes a `BootOrchestrator` trait
implementation at BDS time; platforms provide custom boot flows by implementing the trait.
UEFI-compliant platforms use `BootOptionDiscovery` for boot variable management; platforms
diverging from UEFI compliance can provide boot options directly to `SimpleBootManager::new()`.

**Benefits**: Full UEFI compliance for most platforms. Custom orchestration via `BootOrchestrator`
trait (change order, skip steps, custom control flow). Independent component replacement. Platforms
carry only used code. Escape hatch for non-UEFI-compliant platforms via direct configuration.

## Design Assumptions

This RFC's architecture is based on the following design considerations:

1. **Platform diversity**: Platforms range from simple embedded systems with fixed boot paths to complex
   servers with network boot, remote attestation, and custom boot policies
2. **Customization requirement**: Platform-specific boot flows, connection policies, and orchestration
   requirements necessitate composable components rather than monolithic implementations
3. **Progressive disclosure**: Architecture supports both simple use cases (single composite component)
   and complex scenarios (individual component composition with custom orchestration)
4. **Scope**: This RFC covers boot option discovery and console initialization alongside boot
   orchestration because all are UEFI Boot Manager responsibilities. BootOptionDiscovery is optional
   for platforms diverging from UEFI compliance; ConsoleDiscovery is optional for headless systems

## Implementation Status

The core boot orchestration framework is implemented and validated on QEMU Q35 (x64) and
QEMU SBSA (AArch64), replacing the C-based BdsDxe driver with patina_boot.

### What Is Implemented

- `BootDispatcher` component with BDS architectural protocol installation
- `BootOrchestrator` trait for custom boot flows
- `SimpleBootManager` reference implementation
- `BootConfig` with platform-provided device paths
- Connect-dispatch interleaving via `DxeDispatch` service
- Console discovery (GOP + SimpleTextInput + `ConIn`/`ConOut`/`ErrOut` and
  `ConInDev`/`ConOutDev`/`ErrOutDev` variable writing)
- BDS phase event signaling (EndOfDxe, ReadyToBoot)
- Boot image execution via `LoadImage()`/`StartImage()` with watchdog timer
- Partial device path expansion
- Hotkey detection via `Key####` variables or platform-configured scancode
- `PlatformRecovery####` recovery boot paths after normal boot exhaustion
- Failure handler callback
- Boot option discovery from UEFI variables (`discover_boot_options()` helper)

### Boot Flows

`SimpleBootManager` defines two boot flows selected at runtime based on hotkey state. Both flows
share the same device enumeration, event signaling, and error handling infrastructure.

**Common Preamble** (runs before either flow):

1. Connect-dispatch interleaving — connect all controllers and dispatch newly loaded drivers until stable
2. Console discovery — locate GOP and SimpleTextInput handles, populate console variables
3. Signal EndOfDxe — security components perform lockdown
4. Check hotkey — read `Key####` variables or platform-configured scancode; select boot flow

**Flow 1: Normal Boot** (no hotkey detected)

The platform configures an ordered list of boot device paths. The orchestrator attempts each in
sequence. If all fail, the error path is taken. This supports arbitrary fallback chains (e.g.,
Windows → Linux → UEFI Shell → PXE → USB → front page).

```text
Signal ReadyToBoot
    |
    v
For each device path in BootConfig (in order):
    |
    Try device path (LoadImage/StartImage with 5-min watchdog)
        |
    Success --> boot (never returns)
        |
    Failure --> try next device path
    |
All device paths exhausted
    |
    v
For each PlatformRecovery#### option (in PlatformRecoveryOrder):
    |
    Try recovery device path
        |
    Success --> boot (never returns)
        |
    Failure --> try next recovery option
    |
All recovery options exhausted
    |
    v
Error path: call failure_handler (demo: show error screen)
```

**Flow 2: Hotkey Boot** (hotkey detected, e.g., F12)

When the platform-configured hotkey is held during boot, the orchestrator switches to the hotkey
device list. This flow does **not** fall through to the normal boot devices — if the hotkey device
fails, the error path is taken directly.

```text
Signal ReadyToBoot
    |
    v
Try hotkey device path (LoadImage/StartImage with 5-min watchdog)
    |
Success --> boot (never returns)
    |
Failure
    |
    v
Error path: call failure_handler (demo: show error screen)
```

**Error Path**: Before calling `failure_handler`, the orchestrator attempts `PlatformRecovery####`
boot options (if any are defined via UEFI variables). The `failure_handler` is a platform-provided
callback invoked only after all normal and recovery options are exhausted. Production platforms can
implement diagnostics, user prompts, or system reset as needed.

### Platform-Provided Configuration

```rust
use patina_boot::{BootDispatcher, SimpleBootManager};
use patina_boot::config::BootConfig;

// In ComponentInfo::components():
add.component(BootDispatcher::new(SimpleBootManager::new(
    BootConfig::new(nvme_esp_device_path())    // 1st: NVMe EFI System Partition
        .with_device(nvme_recovery_path())       // 2nd: NVMe recovery partition
        .with_device(pxe_device_path())          // 3rd: PXE network boot
        .with_device(usb_device_path())          // 4th: USB fallback
        .with_hotkey(0x16)                       // F12
        .with_hotkey_device(usb_device_path())   // Hotkey: boot from USB
        .with_failure_handler(|| show_error_screen("All boot options exhausted"))
)));
```

### UEFI-Compliant Configuration (with discover_boot_options)

Platforms requiring full UEFI compliance use `discover_boot_options()` to read boot
options from `Boot####`/`BootOrder`/`BootNext` variables instead of hardcoding device paths.
A custom `BootOrchestrator` calls the helper at boot time:

```rust
use patina_boot::{BootDispatcher, helpers};

struct UefiCompliantBoot;

impl BootOrchestrator for UefiCompliantBoot {
    fn execute(&self, boot_services: &StandardBootServices,
               runtime_services: &StandardRuntimeServices,
               dxe_services: &dyn DxeDispatch,
               image_handle: efi::Handle) -> Result<!, EfiError> {
        let config = helpers::discover_boot_options(runtime_services)?;
        // Use config.devices() for boot attempts...
    }
}

add.component(BootDispatcher::new(UefiCompliantBoot));
```

## Rust Code Design

### Component Architecture

This RFC provides components and a trait that transition firmware from initialization to OS execution.
Each piece operates independently and can be replaced without modifying the others.

**BootOrchestrator** (trait): Defines the boot orchestration interface. Platforms provide
implementations with custom boot flows. This is a plain Rust trait (not a Patina service) because
there is only one consumer (`BootDispatcher`) — service indirection would add complexity without
benefit.

```rust
pub trait BootOrchestrator: Send + Sync + 'static {
    fn execute(
        &self,
        boot_services: &StandardBootServices,
        runtime_services: &StandardRuntimeServices,
        dxe_services: &dyn DxeDispatch,
        image_handle: efi::Handle,
    ) -> Result<!, EfiError>;
}
```

**BootDispatcher** (component): The Patina component that installs the BDS architectural protocol.
Holds a `Box<dyn BootOrchestrator>` and invokes it when the DXE core calls the BDS entry point.
Platforms register it with their chosen orchestrator: `BootDispatcher::new(impl BootOrchestrator)`.

**SimpleBootManager**: Reference implementation of `BootOrchestrator`. Constructed from `BootConfig`
via `SimpleBootManager::new(config)`. Implements the common boot flow (connect, signal
events, detect hotkey, boot from device paths, handle failure).

*Rationale: The trait-based design enforces the boot orchestration contract at compile time. Platforms
swap boot behavior by providing a different `BootOrchestrator` impl to `BootDispatcher::new()`,
requiring only a single component registration. This follows the principle that a trait is better than
a service when there is only one consumer.*

**`discover_boot_options()`** (helper function): Reads `Boot####` variables containing
`EFI_LOAD_OPTION` structures, `BootOrder` variable, and `BootNext` variable to build a `BootConfig`.
If `BootNext` is set, it is included as the first boot option and the variable is deleted. Takes
`RuntimeServices` as a parameter.

*Rationale: Boot option discovery is a helper function rather than a component because
it has a single consumer and no need for framework dispatch ordering. Platforms call it
in their orchestrator when they need UEFI-compliant boot variable support. Platforms
diverging from UEFI compliance simply provide `BootConfig` directly to `SimpleBootManager`.*

**ConsoleDiscovery** (component): Locates GOP and SimpleTextInput protocol handles, creates device
paths, and writes `ConIn`/`ConOut`/`ErrOut` variables (NVRAM, default selection) and
`ConInDev`/`ConOutDev`/`ErrOutDev` variables (volatile, all discovered devices each boot). Depends on
RuntimeVariableServices and ProtocolServices.

*Prerequisites:*

- PCI bus enumeration must complete before GOP devices are discoverable (graphics controllers are
  typically PCI devices)
- USB bus enumeration must complete before USB input devices are discoverable (keyboards, mice)
- When used with `SimpleBootManager`, these prerequisites are satisfied by the `connect_all()` call
  that runs before console discovery
- Platforms using `ConsoleDiscovery` standalone must ensure bus enumeration completes first

*Rationale: Console setup is a component because platforms may need custom console configuration
(serial-only, specific GOP preference, headless operation). As a component, platforms can replace
it entirely without modifying orchestration code.*

**Common Boot Flow** (what `SimpleBootManager` implements):

1. **Device enumeration**: Interleave controller connection with DXE driver dispatch until stable
2. **Signal EndOfDxe**: Notify security components to perform lockdown
3. **Console discovery**: Locate GOP and input devices, populate ConIn/ConOut/ErrOut and
   ConInDev/ConOutDev/ErrOutDev variables
4. **Hotkey detection**: Check `Key####` variables or platform-configured scancode; if pressed,
   use alternate boot options
5. **Signal ReadyToBoot**: Notify drivers that boot is imminent
6. **Execute boot options**: For each device path in `BootConfig`:
   - Enable 5-minute watchdog timer
   - Call `LoadImage()` / `StartImage()`
   - Disable watchdog if boot returns control
   - On failure, attempt next option
7. **Platform recovery**: If all normal boot options fail, attempt `PlatformRecovery####` options
8. **Handle boot failure**: Call platform-provided failure handler if all options (including recovery) exhausted

**BootConfig**: Configuration specifying boot options as device paths. `SimpleBootManager` receives
boot options via `BootConfig` at construction time. Any boot target expressible as a legal UEFI
device path is supported: storage devices, network boot, ramdisks, or platform-specific boot media.
Configuration also includes an optional hotkey and alternate boot options to use when the hotkey is
detected.

Boot options are sourced in one of two ways:

- **UEFI-compliant platforms**: Use `discover_boot_options()` helper, which reads
  `Boot####`/`BootOrder`/`BootNext` variables and returns a `BootConfig`
- **Non-UEFI-compliant platforms**: Provide `BootConfig` directly to
  `SimpleBootManager::new()`

### Driver Dispatch

Connecting device controllers may expose firmware volumes containing additional drivers (e.g., PCI
option ROM drivers). `SimpleBootManager` handles connect-dispatch interleaving internally: connect
controllers, dispatch newly-loaded drivers, repeat until the topology stabilizes.

Platforms needing custom enumeration (selective buses, specific ordering, minimal enumeration) can
implement a custom `BootOrchestrator`. Example with PCI-only enumeration:

```rust
struct PciOnlyOrchestrator { /* config */ }

impl BootOrchestrator for PciOnlyOrchestrator {
    fn execute(
        &self,
        boot_services: &StandardBootServices,
        runtime_services: &StandardRuntimeServices,
        dxe_services: &dyn DxeDispatch,
        image_handle: efi::Handle,
    ) -> Result<!, EfiError> {
        // Selective: Connect only PCI roots (skip USB for headless system)
        let pci_handles = boot_services.protocol().locate_handle_buffer(
            HandleSearchType::ByProtocol(&PCI_ROOT_BRIDGE_IO_PROTOCOL_GUID)
        ).unwrap();

        for handle in pci_handles {
            boot_services.protocol().connect_controller(handle, true).unwrap();
        }

        // Continue with event signaling, boot execution...
    }
}

// Platform registration:
add.component(BootDispatcher::new(PciOnlyOrchestrator::new(/* config */)));
```

### Event Signaling

`SimpleBootManager` (the reference `BootOrchestrator`) signals two required BDS phase events:

- `EndOfDxe` - After device topology is stable (PCIe enumeration and GOP discovery complete).
  Security components register for this event and perform lockdown.
- `ReadyToBoot` - Immediately before attempting first boot option

EventServices (RFC 0017) bridges Patina components and C-based PI drivers. C drivers register via
`CreateEventEx()`, and Patina signals via `event_services.signal_event_group()`.

### Library Module

A library module provides helper functions for platforms implementing custom orchestration. These
functions allow platforms to build custom boot flows using the same underlying logic as common
components.

**Available Functions:**

- `boot_from_device_path()` - Load and start image with UEFI spec compliance (5-minute watchdog per
  Section 3.1.2). Accepts both full and partial device paths, automatically expanding partial paths
  before boot.
- `expand_device_path()` - Expand partial device paths (e.g., HD media paths with partition GUID) to
  full device paths by matching against the current device topology. Returns full paths unchanged.
- `connect_all()` - Connect all controllers recursively until device topology stabilizes
- `signal_bds_phase_entry()` - Signal EndOfDxe event
- `signal_ready_to_boot()` - Signal ReadyToBoot event
- `discover_console_devices()` - Discover console devices and populate UEFI console variables
- `detect_hotkey()` - Check for hotkey press via `Key####` variables (UEFI-compliant) or
  platform-configured scancode
- `discover_recovery_options()` - Read `PlatformRecovery####`/`PlatformRecoveryOrder` variables
  and return recovery boot options
- `is_partial_device_path()` - Check whether a device path is partial (short-form) or full
- `discover_boot_options()` - Read `Boot####`/`BootOrder`/`BootNext` variables and return a `BootConfig`

`SimpleBootManager` uses these internally. Custom `BootOrchestrator` implementations use them directly.

*Note: `interleave_connect_and_dispatch()` is private to `SimpleBootManager` rather than a public
helper, because it uses `connect_all()` which connects every controller on every round. This is
not recommended for most platforms with large device topologies. Platforms needing custom
connect-dispatch interleaving should implement their own strategy in a custom `BootOrchestrator`.*

#### Device Path Expansion

UEFI `Boot####` variables can contain partial (short-form) device paths that specify only a portion
of the full hardware path. For example, a hard drive boot option might contain only `HD(1,GPT,<GUID>)`
without the preceding PCI path. These partial paths must be expanded by matching against the current
device topology before they can be used for booting.

Partial paths are detected by examining the first device path node:

- **Full paths** start with hardware/ACPI root nodes (`PciRoot`, `Acpi`)
- **Partial paths** start with media nodes (`HD`, `CDROM`), messaging nodes without root, or file paths

Both `expand_device_path()` and `boot_from_device_path()` are provided to support different use cases:

| Function | Use Case |
|---|---|
| `boot_from_device_path()` | Simple case: pass any device path and boot. Expansion happens transparently. |
| `expand_device_path()` | Advanced case: explicit control over expansion for caching, custom prioritization when multiple devices match, or pre-boot validation. |

**Design rationale**: Providing both functions ensures the library remains composable and future-proof.
Most platforms just call `boot_from_device_path()` and it works transparently. Platforms needing
explicit control (e.g., caching resolved paths across retries, custom device selection logic) can
call `expand_device_path()` directly.

```rust
// Simple: just boot, expansion handled internally
boot_from_device_path(&partial_path)?;

// Advanced: expand first for caching/custom logic, then boot
let full_path = expand_device_path(&partial_path)?;
boot_from_device_path(&full_path)?;
```

## Guide-Level Explanation

Platforms integrate boot functionality by adding components and configuration during DXE Core
initialization.

### Platform-Provided Device Paths

The simplest integration provides boot device paths directly. The platform knows its boot devices
ahead of time and configures an ordered fallback chain and hotkey paths:

```rust
use patina_boot::{BootDispatcher, SimpleBootManager};
use patina_boot::config::BootConfig;

// In ComponentInfo::components():
add.component(BootDispatcher::new(SimpleBootManager::new(
    BootConfig::new(nvme_esp_path())            // 1st: NVMe EFI System Partition
        .with_device(nvme_recovery_path())        // 2nd: NVMe recovery partition
        .with_device(pxe_device_path())           // 3rd: PXE network boot
        .with_device(usb_device_path())           // 4th: USB fallback
        .with_hotkey(0x16)                        // F12
        .with_hotkey_device(usb_device_path())    // Used when F12 is held
        .with_failure_handler(|| show_error_screen("Boot failed"))
)));
```

This is sufficient to boot Windows. `SimpleBootManager` handles device enumeration, BDS event
signaling, hotkey detection, and boot execution automatically. `BootDispatcher` installs the BDS
architectural protocol; the DXE core invokes it after all architectural protocols are satisfied.

### UEFI-Compliant Usage (with discover_boot_options)

Platforms requiring full UEFI compliance use `discover_boot_options()` to read boot
options from `Boot####`/`BootOrder`/`BootNext` variables. A custom orchestrator calls the helper
at boot time and uses the returned `BootConfig` for boot attempts. See the
UEFI-Compliant Configuration section above for an example.

### Custom Orchestration

Platforms can implement the `BootOrchestrator` trait for specialized behavior and pass their
implementation to `BootDispatcher::new()`. See the Driver Dispatch section for an example custom
orchestrator with selective PCI-only enumeration.

## Rejection Summary

This RFC was moved to the rejected folder as a suspended state. Rather than being rejected on its merits,
the boot orchestration design is being refined and tested outside the Patina project before it is
reevaluated for inclusion.

### Key Rejection Reasons

1. **Design Still Maturing**: The component architecture and boot orchestration model are still being
   iterated on. Refining the design against real platform integrations outside Patina allows the proposal
   to stabilize before it is committed to the project's RFC history.

2. **Validation Outside Patina**: The design is being implemented and tested in a separate context. Keeping
   the RFC out of the accepted set until that validation completes avoids prematurely standardizing an
   interface that may still change.

### Future Considerations

The design is planned for future reevaluation. Once it has been refined and validated outside the Patina
project, a follow-up RFC is expected to propose the boot orchestration component for acceptance.

**Note**: This RFC remains available for reference in its suspended state. The component decomposition,
`BootOrchestrator` trait design, and BDS phase signaling approach documented here are expected to inform the
future proposal.
