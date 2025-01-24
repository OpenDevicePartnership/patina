# Differences from EDK II

The Rust DXE core has several functional and implementation differences from the edk2 DXE core.

Platforms should ensure the following specifications are met when transitioning over to the Rust DXE core:

- Traditional SMM is not supported.

  - This specifically means that the following SMM module types that require cooperation between the SMM and DXE
    dispatchers are not supported:

    - `EFI_FV_FILETYPE_SMM` (`0xA`)
    - `EFI_FV_FILETYPE_SMM_CORE` (`0xD`)

  - Further, combined DXE modules will not be dispatched. These include:

    - `EFI_FV_FILETYPE_COMBINED_PEIM_DRIVER` (`0x8`)
    - `EFI_FV_FILETYPE_COMBINED_SMM_DXE` (`0xC`)

  - DXE drivers and Firmware volumes **will** be dispatched:

    - `EFI_FV_FILETYPE_DRIVER` (`0x7`)
    - `EFI_FV_FILETYPE_FIRMWARE_VOLUME_IMAGE` (`0xB`)

  - Because Traditional SMM is not supported, events such as the `gEfiEventDxeDispatchGuid` that was used in the C DXE
    Core to signal the end of a DXE dispatch round so SMM drivers with DXE dependency expressions could be reevaluated
    will not be signaled.

  - Dependency expressions such as `EFI_SECTION_SMM_DEPEX` will not be evaluated on firmware volumes.

  - Reason: Traditional SMM is not supported to prevent coupling between the DXE and MM environments. This is error
    prone, unnecessarily increases the scopes of DXE responsibilities, and can lead to security vulnerabilities.
    Standalone MM should be used instead. The combined drivers have not gained traction in actual implementations due
    to their lack of compatibility for most practical purposes and further increase the likelihood of coupling between
    core environments and user error when authoring those modules. The Rust DXE Core focuses on modern use cases and
    simplification of the overall DXE environment.

- When `EXIT_BOOT_SERVICES` is signalled, the memory map is not allowed to change. See [Exit Boot Services Handlers](../dxe_core/memory_management.md#exit-boot-services-handlers)
- Writing pure rust components do not use the standard EDKII entry point
  (`EFI_HANDLE ImageHandle,EFI_SYSTEM_TABLE *SystemTable`). Instead, the dispatcher uses dependency injection, allowing
  component writers to define their own custom function interface defining all necessary data to be dispatched. See
  [Monolithically Compiled Components](../driver/interface.md).
