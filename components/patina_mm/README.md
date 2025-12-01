# Patina Management Mode (MM) Component Crate

Patina MM provides Management Mode (MM) integration for Patina-based firmware. It focuses on safe MM communication,
deterministic MMI handling, and platform hooks that enable Patina components to interact with existing MM handlers
without relying on C implementations. Read more about MM Technology in below, [here](#mm-technology-background).

## Capabilities

- Produces the `MmCommunication` service for dispatching requests to MM handlers through validated communicate
  buffers.
- Defines the `SwMmiTrigger` service to raise software MM interrupts using platform-configured ports.
- Supports optional `PlatformMmControl` hooks so platforms can run preparatory MM initialization before MM
  communication becomes available.
- Maintains page-aligned communicate buffers with explicit recipient tracking and length verification to detect
  corruption before and after MM execution.
- Emits focused log output to the `mm_comm` and `sw_mmi` targets. Information is detailed to aid in common debug
  like inspecting buffer setup, interrupt triggering details, and MM handler response.

## Platform Managed Components and services

- **MmCommunicator component**: Consumes locked MM configuration, registers the `MmCommunication` service, and
  coordinates MM execution through a swappable executor abstraction that enables in-depth host-based testing.
- **SwMmiManager component**: Consumes the same configuration, registers the `SwMmiTrigger` service, and optionally
  invokes `PlatformMmControl` before exposing MM interrupt capabilities.
- **PlatformMmControl service (optional)**: Lets platforms implement platform-specific logic to prepare for MM
  interrupts.

## Platform Configuration

The crate defines `MmCommunicationConfiguration` as the shared configuration structure. Platforms populate it with:

- ACPI base information so the trigger service can manipulate ACPI fixed hardware registers.
- Command and data port definitions using typed `MmiPort` wrappers (SMI or SMC).
- A list of `CommunicateBuffer` entries that remain page-aligned, zeroed, and tracked by identifier for MM message
  exchange.

> The configuration enforces buffer validation, including alignment, bounds checking, and consistency between tracked
> metadata and buffer contents.

## Integration guidance

Below is the integration guidance for platform owners who wish to configure and produce the `MmCommunication` and
SwMmiTrigger services for usage / consumption by components throughout the dispatch process.

- Register `MmCommunicationConfiguration` to set platform-specific MM parameters.
- Add `SwMmiManager` so the software MMI trigger service can be produced for other Patina components to consume.
- Add `MmCommunicator` to expose the `MmCommunication` service to other Patina components.
- Optionally provide a `PlatformMmControl` implementation when the platform needs to clear or program hardware state
  before MM interrupts are triggered.

```rust
use patina_dxe_core::*;
use patina::{component::service::IntoService, error::Result};
use patina_mm::service::PlatformMmControl;

/// An optional service to ensure Platform MM is initialized.
#[derive(IntoService, Default)]
#[service(dyn PlatformMmControl)]
struct ExamplePlatformMmControl;

impl PlatformMmControl for ExamplePlatformMmControl {
  /// Platform hardware enabling required to support MMIs
  fn init(&self) -> patina::error::Result<()> {
    /* platform MMI init code */
    Ok(())
  }
}

struct ExamplePlatform;

impl ComponentInfo for ExamplePlatform {
  fn configs(mut add: Add<Config>) {
    // See `MmCommunicationConfiguration` struct for configuration options
    add.config(patina_mm::config::MmCommunicationConfiguration {
      acpi_base: patina_mm::config::AcpiBase::Mmio(0x0), // Actual ACPI base address will be set during boot
      cmd_port: patina_mm::config::MmiPort::Smi(0xB2),
      data_port: patina_mm::config::MmiPort::Smi(0xB3),
      enable_comm_buffer_updates: false,
      updatable_buffer_id: None,
      comm_buffers: vec![],
    });
  }

  fn components(mut add: Add<Component>) {
    add.component(patina_mm::component::sw_mmi_manager::SwMmiManager::new());
    add.component(patina_mm::component::communicator::MmCommunicator::new());
  }

  fn services(mut add: Add<Service>) {
    // An optional service to enable platform MM. Since it has no dependencies, we register the service directly. If it
    // had dependencies, This would be a component instead.
    add.service(ExamplePlatformMmControl::default());
  }
}
```

## Service Usage guidance

Below is the integration guidance for component writers who wish to consume and use the `MmCommunication` and
`SwMmiTrigger` services in their Patina component.

```rust
use zerocopy_derive::*;

use patina_mm::service::MmCommunication;
use patina::component::prelude::{IntoComponent, Service};

#[derive(Debug, Clone, Copy, IntoBytes, FromBytes, Immutable)]
#[repr(C)]
pub struct MmSupervisorRequestHeader {
  pub signature: u32,
  pub revision: u32,
  pub request: u32,
  pub reserved: u32,
  pub result: u64,
}

#[derive(Debug, Clone, Copy, IntoBytes, FromBytes, Immutable)]
#[repr(C)]
pub struct MmSupervisorVersionInfo {
  pub version: u32,
  pub patch_level: u32,
  pub max_supervisor_request_level: u64,
}

#[derive(Default, IntoComponent)]
pub struct MmSupervisorDemo;

impl MmSupervisorDemo {
  pub fn new() -> Self {
      Self
  }
  /// Entry point for the MM Test component.
  ///
  /// Uses the `MmCommunication` service to send a request version information from the MM Supervisor. The MM
  /// Supervisor is expected to be the Standalone MM environment used on the QEMU Q35 platform.
  pub fn entry_point(self, mm_comm: Service<dyn MmCommunication>) -> patina::error::Result<()> {
    let mm_supv_req_header = MmSupervisorRequestHeader {
      signature: u32::from_le_bytes([b'M', b'S', b'U', b'P']),
      revision: 1,
      request: 0x0003, // Request Version Info
      reserved: 0,
      result: 0,
    };
  
    let result = unsafe {
      mm_comm
        .communicate(
          0,
          core::slice::from_raw_parts(
            &mm_supv_req_header as *const _ as *const u8,
            core::mem::size_of::<MmSupervisorRequestHeader>(),
          ),
          patina::Guid::from_fields(
            0x8c633b23,
            0x1260,
            0x4ea6,
            0x83,
            0x0F,
            [0x7d, 0xdc, 0x97, 0x38, 0x21, 0x11],
          ),
        )
        .map_err(|_| {
          log::error!("MM Communication failed");
          patina::error::EfiError::DeviceError // Todo: Map actual codes
        })?
    };

    let mm_supv_ver_info = unsafe {
      &*(result[core::mem::size_of::<MmSupervisorRequestHeader>()..].as_ptr() as *const MmSupervisorVersionInfo)
    };
    let version = mm_supv_ver_info.version;
    let patch_level = mm_supv_ver_info.patch_level;
    let max_request_level = mm_supv_ver_info.max_supervisor_request_level;
    log::info!(
      "MM Supervisor Version: {:#X}, Patch Level: {:#X}, Max Request Level: {:#X}",
      version,
      patch_level,
      max_request_level
    );
    Ok(())
  }
}
```

## MM Technology Background

System Management Mode (SMM) or Management Mode (MM) is a special-purpose operating mode in x86 architecture
with high execution privilege that is used to monitor and manage various system resources. MM code is often
written similarly to non-MM UEFI Code, built with the same toolset and included alongside non-MM UEFI code in
the same firmware image. However, MM code executes in a special region of memory that is isolated from the rest
of the system, and it is not directly accessible to the operating system or other software running on the system.

This region is called System Management RAM (SMRAM) or Management Mode RAM (MMRAM). Since this region is isolated,
constructs from the DXE environment like boot services, runtime services, and the DXE protocol database are not
available in MM. Instead, MM code uses its own services table and protocol data entirely managed in MMRAM.

MM is entered on a system by triggering a System Management Interrupt (SMI) also called a Management Mode
Interrupt (MMI). The MMI may be either triggered by software (synchronous) or a hardware (asynchronous) event. A
MMI is a high priority, non-maskable interrupt. On receipt of the interrupt, the processor saves the current state
of the system and switches to MM. Within MM, the code must set up its own execution environment such as applying
an interupt descriptor table (IDT), creating page tables, etc. It must also identify the source of the MMI to
determine what MMI handler to invoke in response.

Recently, there has been an effort to reduce and even eliminate the use of MM in modern systems. MM represents a
large attack surface because of its pervasiveness throughout the system lifetime. It is especially impactful if
compromised due to its ubiquity and system access privilege. A vulnerability in a given MM implementation could
further be used to compromise or circumvent OS protections such as Virtualization-based Security (VBS). Based on
the current use cases for MM and available alternatives, it is not possible to completely eliminate MM from
modern systems.
