
# Patina DXE Core

This crate contains a Pure Patina DXE Core.

## DXE Core Goals

1. Construction of a bare-metal "kernel (DXE core)" to dispatch from `DxeIpl`.
   1. Log output over a basic subsystem such as serial I/O.
   2. Integrable into a UEFI build as a replacement for `DxeMain` with observable debug output.
   3. Greater than 80% unit test coverage across all code compiled into the DXE Core.
   4. A "monolithic" DXE environment that encapsulates functionality distributed across separate EFI modules today.
      This is accomplished with an internal dispatcher to the binary that executes individual components linked during
      platform integration and given to the common Patina DXE Core interface when the platform builds its version of
      Patina DXE Core.
   5. In addition to internal Rust component dispatch, UEFI driver dispatch - FVs and FFS files in the firmware ROM.
   6. No direct dependencies on PEI except PI abstracted structures.

2. Support for CPU interrupts/exception handlers.

3. Support for paging and heap allocation.

4. UEFI memory protections that implement best known practices and drive memory protections in UEFI firmware forward.

For more information, refer to [Setting up the DXE Core](https://opendevicepartnership.github.io/patina/integrate/dxe_core.html).

## Contributing

- Review Rust Documentation in the `docs` directory.
- Run unit tests and ensure all pass.
