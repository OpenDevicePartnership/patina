# Differences from EDK II

The Rust DXE core has several functional and implementation differences from the edk2 DXE core.
Platforms should ensure the following specifications are met when transitioning over to the Rust DXE core:

- When `EXIT_BOOT_SERVICES` is signalled, the memory map is not allowed to change. See [Exit Boot Services Handlers](../dxe_core/memory_management.md#exit-boot-services-handlers)
