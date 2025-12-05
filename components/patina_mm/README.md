# Patina Management Mode (MM) Component Crate

Patina MM provides Management Mode (MM) integration for Patina-based firmware. It focuses on safe MM communication,
deterministic MMI handling, and platform hooks that enable Patina components to interact with existing MM handlers
without relying a driver implemented in C. Read more about MM Technology [here](#mm-technology-background).

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

## Platform Integration guidance

Below is the integration guidance for platform owners which want patina to configure and produce the `MmCommunication`
and `SwMmiTrigger` services for consumption by components throughout the dispatch process.

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

Below is example usage of the `MmCommunication` service for component writers who wish to use this functionality in
their Patina component. If you are looking for a real world example, please refer to the [QemuQ35MmTest](https://github.com/OpenDevicePartnership/patina-dxe-core-qemu/blob/main/src/q35/component/service/mm_test.rs)
component in [patina-dxe-core-qemu](https://github.com/OpenDevicePartnership/patina-dxe-core-qemu/blob/main/src/q35/component/service/mm_test.rs).

```rust
use zerocopy_derive::*;
use zerocopy::IntoBytes;

use patina_mm::service::MmCommunication;
use patina::component::{component, prelude::Service};

#[derive(Debug, Clone, Copy, IntoBytes, FromBytes, Immutable)]
#[repr(C)]
pub struct DataToSend {
  pub signature: u32,
  pub buffer: [u8; 16],
  pub field1: u32,
  pub field2: u16,
  pub padding: [u8; 2],
}

#[derive(Default)]
pub struct ExampleComponent;

#[component]
impl ExampleComponent {
  /// Example Entry point that just sends a single message
  pub fn entry_point(self, mm_comm: Service<dyn MmCommunication>) -> patina::error::Result<()> {
    let data = DataToSend {
      signature: u32::from_le_bytes([b'M', b'S', b'U', b'P']),
      buffer: [b'H', b'E', b'L', b'L', b'O', b'\0', 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0],
      field1: 15,
      field2: 50,
      padding: [0; 2],
    };

    let recipient = patina::Guid::from_string("8c633b23-1260-4ea6-830f7ddc97382111");
  
    let _ = unsafe {
      mm_comm
        .communicate(
          0,
          data.as_bytes(),
          recipient
        )
        .map_err(|_| {
          log::error!("MM Communication failed");
          patina::error::EfiError::DeviceError // Todo: Map actual codes
        })?
    };
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
accessing services from the DXE environment, like boot services, runtime services, and the DXE protocol database are
restricted. MM contains its own configuration, such as IDTs, Page Tables, and provides services tables and protocol
data entirely managed in MMRAM.

MM is entered on a system by triggering a System Management Interrupt (SMI) also called a Management Mode Interrupt
(MMI). MMIs preempt all other running processes and may be either triggered by software (synchronous) or a hardware
(asynchronous) event.  On receipt of the interrupt, the processor saves the current state of the system and switches to
MM. Once in MM, the MM environment identifies the source of the MMI to and invokes a MMI handler to address the source
of the MMI.

There are industry wide ongoing efforts to reduce and even eliminate the use of MM in modern systems. MM represents a
large attack surface because of its pervasiveness throughout the system lifetime. It is especially impactful if
compromised due to its privileged system access. A vulnerability in MM implementations endanger the entire system as it
could be exploited to circumvent OS protections such as Virtualization-based Security (VBS). Current systems have yet
to be able to eliminate all MM usage.
