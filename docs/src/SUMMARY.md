# Summary

[Introduction](introduction.md)
[Core Concepts](concepts.md)
[Code Organization](dev/code_organization.md)
[Patina Background](patina.md)

# Rust Development for UEFI

- [Best Practices](dev/principles.md)
  - [Abstractions](dev/principles/abstractions.md)
  - [Code Reuse](dev/principles/reuse.md)
  - [Configuration in Code](dev/principles/config.md)
  - [Dependency Management](dev/principles/dependency-management.md)
  - [Error Handling](dev/principles/error-handling.md)
- [Testing](dev/testing.md)
  - [Unit Testing](dev/testing/unit.md)
  - [Integration Testing](dev/testing/integration.md)
  - [On-Platform Testing](dev/testing/platform.md)
  - [Mocking](dev/testing/mock.md)
- [Formatting](dev/formatting.md)
- [Documenting](dev/documenting.md)
  - [Quick Reference](dev/documenting/reference.md)
- [RFC Template](rfc/template.md)
- [Process for Unstable Features](dev/unstable.md)
- [Other Resources](dev/other.md)

# Integrating the Rust DXE Core

- [Workspace Setup](integrate/workspace.md)
  - [Local to the Platform](integrate/compile_local.md)
  - [External to the Platform](integrate/compile_external.md)
- [Setting up the DXE Core](integrate/dxe_core.md)
- [Updating the Platform](integrate/platform.md)
  - [Local to the Platform](integrate/platform_local.md)
  - [External to the Platform](integrate/platform_external.md)
- [Rust DXE Core vs. EDK II](integrate/rust_vs_edk2.md)

# Contributing to the Rust DXE Core

- [Theory and Operation](dxe_core/operation.md)
  - [Advanced Logger](dxe_core/advanced_logger.md)
  - [CPU](dxe_core/cpu.md)
  - [Debugging](dxe_core/debugging.md)
  - [Event, Timer, and Task Priority](dxe_core/events.md)
  - [Protocol Database](dxe_core/protocol_database.md)
  - [UEFI Driver Model](dxe_core/driver_model.md)
  - [Component Model](dxe_core/component_model.md)
  - [Memory Management](dxe_core/memory_management.md)
  - [Image Loading and Execution](dxe_core/images.md)
  - [Dispatcher](dxe_core/dispatcher.md)
  - [Performance Analysis]()
  - [Synchronization](dxe_core/synchronization.md)
  - [Testing](dxe_core/testing.md)

# Creating a Patina Component

- [Component Interface](component/interface.md)
- [Component Crate Requirements](component/requirements.md)

-----------
[Contributors](misc/contributors.md)
