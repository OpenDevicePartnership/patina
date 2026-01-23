# Patina Boot

Boot orchestration component for Patina-based firmware implementing UEFI boot manager functionality.

## Components

- **BootOrchestrator**: Orchestrates device enumeration, BDS phase events, and boot option execution.
- **ConsoleDiscovery**: Discovers console devices and populates UEFI console variables.

## Usage

```rust
use patina_boot::{component::BootOrchestrator, config::BootOptions};

// Configure boot options (devices are tried in order)
let config = BootOptions::new()
    .with_device(primary_boot_path)
    .with_device(fallback_boot_path)
    .with_failure_handler(|| { /* handle boot failure */ });

// Add BootOrchestrator as a platform component
add.component(BootOrchestrator);
```

## Helper Functions

For custom boot flows, use the helper functions in the `helpers` module:

- `connect_all()` - Connect all controllers for device enumeration
- `signal_bds_phase_entry()` - Signal EndOfDxe event
- `signal_ready_to_boot()` - Signal ReadyToBoot event
- `discover_console_devices()` - Populate console variables
- `boot_from_device_path()` - Load and start a boot image
