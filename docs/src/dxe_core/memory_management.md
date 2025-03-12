# Memory Management

## Exit Boot Services Handlers

Platforms should ensure that when handling an `EXIT_BOOT_SERVICES` signal (and `PRE_EXIT_BOOT_SERVICES_SIGNAL`),
they do not change the memory map. This means allocating and freeing are disallowed once
`EFI_BOOT_SERVICES.ExitBootServices()` (`exit_boot_services()`) is invoked.

In the Rust DXE core in release mode, allocating and freeing within the GCD (which changes the memory map and its key)
will return an error that can be handled by the corresponding driver.
In debug builds, any changes to the memory map following `exit_boot_services` will panic due to an assertion.
