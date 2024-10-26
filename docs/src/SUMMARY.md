# Summary

[Introduction](introduction.md)
[Core Concepts](concepts.md)

# Rust Development for UEFI
- [Best Practices](dev/principles.md)
  - [Abstractions](dev/principles/abstractions.md)
  - [Code Reuse](dev/principles/reuse.md)
  - [Configuration in Code](dev/principles/config.md)
- [Testing](dev/testing.md)
  - [Unit Testing](dev/testing/unit.md)
  - [Integration Testing](dev/testing/integration.md)
  - [Mocking]()
- [Formatting](dev/formatting.md)
- [Documenting](dev/documenting.md)
  - [Quick Reference](dev/documenting/reference.md)
- [Other Resources](dev/other.md)

# Integrating the Rust DXE Core

- [Workspace Setup](integrate/workspace.md)
  - [Local to the Platform](integrate/compile_local.md)
  - [External to the Platform](integrate/compile_external.md)
- [Setting up the DXE Core](integrate/dxe_core.md)
- [Updating the Platform](integrate/platform.md)
  - [Local to the Platform](integrate/platform_local.md)
  - [External to the Platform](integrate/platform_external.md)

# Contributing to the Rust DXE Core

- [Theory and Operation](dxe_core/operation.md)
  - [Advanced Logger]()
  - [CPU]()
  - [Debugging]()
  - [Event, Timer, and Task Priority](dxe_core/events.md)
  - [Protocol Database](dxe_core/protocol_database.md)
  - [UEFI Driver Model](dxe_core/driver_model.md)
  - [Memory Management]()
  - [Image Loading and Execution](dxe_core/images.md)
  - [Dispatcher](dxe_core/dispatcher.md)
  - [Performance Analysis]()
  - [Synchronization](dxe_core/synchronization.md)
  - [Testing]()

# Creating a Rust DXE Driver
- [Component Interface](driver/interface.md)

-----------
[Contributors](misc/contributors.md)
