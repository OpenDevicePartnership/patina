# Summary

[Introduction](introduction.md)
[Patina Background](patina.md)
[Patina in the UEFI Rust Ecosystem](background/patina_in_the_rust_ecosystem.md)
[Training Videos](training_videos.md)

# Background Information

- [Patina DXE Core Memory Safety Strategy](background/memory_safety_strategy.md)
- [Rust Tooling in Patina](background/rust_tools.md)
- [UEFI Memory Safety Case Studies](background/uefi_memory_safety_case_studies.md)

# Best Practices

- [Abstractions](dev/principles/abstractions.md)
- [Architecture Abstraction](dev/principles/architecture-abstraction.md)
- [Code Organization](dev/code_organization.md)
- [Code Reuse](dev/principles/reuse.md)
- [Dependency Management](dev/principles/dependency-management.md)
- [Error Handling](dev/principles/error-handling.md)
- [FFI Authoring](dev/principles/ffi.md)
- [Unsafe Guidance](dev/principles/unsafe.md)

# Developer Guides

- [Code First Process](code_first/code_first_process.md)
- [Code First Template](code_first/template.md)
- [Creating a New Unstable Feature](dev/unstable_feature.md)
- [Debugging](dev/debugging.md)
  - [Core Reload](dev/debugging/core_reload.md)
  - [Windbg Debugging Example](dev/debugging/windbg_example.md)
  - [Windbg Debugging](dev/debugging/windbg_debugging.md)
- [Documenting](dev/documenting.md)
  - [Quick Reference](dev/documenting/reference.md)
- [Formatting](dev/formatting.md)
- [Hardware Access](dev/hardware_access.md)
  - [Memory-Mapped I/O (MMIO)](dev/hardware_access/mmio.md)
- [Other Resources](dev/other.md)
- [Process for Unstable Features](dev/unstable.md)
- [RFC Lifecycle](rfc_lifecycle.md)
- [RFC Template](rfc/template.md)
- [Testing](dev/testing.md)
  - [Integration Testing](dev/testing/integration.md)
  - [Mocking](dev/testing/mock.md)
  - [On-Platform Testing](dev/testing/platform.md)
  - [QEMU PR Validation](dev/testing/qemu_pr_validation.md)
  - [Unit Testing](dev/testing/unit.md)
- [Toolchain Configuration](dev/toolchain_configuration.md)

# Patina Component Model

- [Component Crate Requirements](component/requirements.md)
- [Component Interface](component/interface.md)
- [Getting Started with Components](component/getting_started.md)

# Patina DXE Core Platform Integration

- [Patina DXE Core Requirements Checklist](integrate/patina_dxe_core_requirements_checklist.md)
- [Patina DXE Core Requirements](integrate/patina_dxe_core_requirements.md)
- [Setting up the Patina DXE Core](integrate/dxe_core.md)

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

-----------
- [Contributors](misc/contributors.md)
- [License](misc/license_history.md)
