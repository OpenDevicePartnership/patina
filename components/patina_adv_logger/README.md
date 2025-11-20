# Patina Advanced Logger

The Patina Advanced Logger provides the Patina core with in-memory and serial logging capabilities.

## Platform Integration

1. Instantiate `AdvancedLogger` with the desired format, filters, level, and serial implementation.
   Register it with `log::set_logger` as early as possible.
2. Call `AdvancedLogger::init` with the physical HOB list pointer.
   This allows the logger to adopt the buffer and record its address for later protocol publication.
3. Discover or provision an Advanced Logger buffer.
   Note: Platform firmware typically passes the Advanced Logger HOB from a prior boot phase.
4. Dispatch the `AdvancedLoggerComponent` through Patina so it can install the Advanced Logger protocol via boot
   services.

## Memory Log Behavior

The crate stores records in a shared buffer that begins with an `ADVANCED_LOGGER_INFO` header.

- Aligned entries follow the header.
- Each entry records the boot phase identifier, EFI debug level mask, timestamp counter, and message bytes.

## Parser Support

When the `std` feature is enabled the crate provides a `parser` module for captured log buffers.

It opens the buffer, prints header metadata, and emits log lines with optional level and timestamp context.
This parser underpins host utilities and remains version-aligned with the memory layout implemented in `memory_log.rs`.

## Documentation

- [Advanced Logger Details](https://opendevicepartnership.github.io/patina/dxe_core/advanced_logger.html)
