# Summary

[Introduction](introduction.md)
[Patina Background](patina.md)
[RFC Lifecycle](rfc_lifecycle.md)
[Code Organization](dev/code_organization.md)

# Background Information

- [Patina DXE Core Memory Safety Strategy](background/memory_safety_strategy.md)
- [Rust Tooling in Patina](background/rust_tools.md)
- [UEFI Memory Safety Case Studies](background/uefi_memory_safety_case_studies.md)

# Best Practices

- [Abstractions](dev/principles/abstractions.md)
- [Code Reuse](dev/principles/reuse.md)
- [Dependency Management](dev/principles/dependency-management.md)
- [Error Handling](dev/principles/error-handling.md)

# Developer Guides

- [Documenting](dev/documenting.md)
  - [Quick Reference](dev/documenting/reference.md)
- [Formatting](dev/formatting.md)
- [Other Resources](dev/other.md)
- [Process for Unstable Features](dev/unstable.md)
- [RFC Template](rfc/template.md)
- [Testing](dev/testing.md)
  - [Unit Testing](dev/testing/unit.md)
  - [Integration Testing](dev/testing/integration.md)
  - [On-Platform Testing](dev/testing/platform.md)
  - [Mocking](dev/testing/mock.md)
- [Debugging](dev/debugging.md)
  - [Windbg Debugging](dev/debugging/windbg_debugging.md)
  - [Windbg Debugging Example](dev/debugging/windbg_example.md)

# Patina DXE Core Platform Integration

- [Patina DXE Core Requirements](integrate/patina_dxe_core_requirements.md)
- [Setting up the Patina DXE Core](integrate/dxe_core.md)

# Patina Component Model

- [Getting Started with Components](component/getting_started.md)
- [Component Crate Requirements](component/requirements.md)
- [Component Interface](component/interface.md)

# Patina DXE Core Subsystems

- [Theory and Operation](dxe_core/operation.md)
  - [Advanced Logger](dxe_core/advanced_logger.md)
  - [Component Model](dxe_core/component_model.md)
  - [CPU](dxe_core/cpu.md)
  - [Debugging](dxe_core/debugging.md)
  - [Dispatcher](dxe_core/dispatcher.md)
  - [Event, Timer, and Task Priority](dxe_core/events.md)
  - [Image Loading and Execution](dxe_core/images.md)
  - [Memory Management](dxe_core/memory_management.md)
  - [Protocol Database](dxe_core/protocol_database.md)
  - [Synchronization](dxe_core/synchronization.md)
  - [Testing](dxe_core/testing.md)
  - [UEFI Driver Model](dxe_core/driver_model.md)

# Component Documentation

- [Performance Analysis](components/patina_performance.md)

-----------
[Contributors](misc/contributors.md)
