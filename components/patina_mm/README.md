# Patina Management Mode (MM) Component Crate

Patina MM provides Management Mode (MM) integration for Patina-based firmware. It focuses on safe MM communication,
deterministic MMI handling, and platform hooks that enable Patina components to interact with existing MM handlers
without relying on C implementations.

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

## Components and services

- **MmCommunicator component**: Consumes locked MM configuration, registers the `MmCommunication` service, and
  coordinates MM execution through a swappable executor abstraction that enables in-depth host-based testing.
- **SwMmiManager component**: Consumes the same configuration, registers the `SwMmiTrigger` service, and optionally
  invokes `PlatformMmControl` before exposing MM interrupt capabilities.
- **PlatformMmControl service (optional)**: Lets platforms implement platform-specific logic to prepare for MM
  interrupts.

## Configuration

The crate defines `MmCommunicationConfiguration` as the shared configuration structure. Platforms populate it with:

- ACPI base information so the trigger service can manipulate ACPI fixed hardware registers.
- Command and data port definitions using typed `MmiPort` wrappers (SMI or SMC).
- A list of `CommunicateBuffer` entries that remain page-aligned, zeroed, and tracked by identifier for MM message
  exchange.

> The configuration enforces buffer validation, including alignment, bounds checking, and consistency between tracked
> metadata and buffer contents.

## Integration guidance

- Register `MmCommunicationConfiguration` to set platform-specific MM parameters.
- Add `SwMmiManager` so the software MMI trigger service can be produced for other Patina components to consume.
- Add `MmCommunicator` to expose the `MmCommunication` service to other Patina components.
- Provide a `PlatformMmControl` implementation when the platform needs to clear or program hardware state before MM
  interrupts are triggered.
