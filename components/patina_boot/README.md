# Patina Boot

Boot orchestration component for Patina-based firmware implementing UEFI boot manager functionality.

## Components

- **BootDispatcher**: Installs the BDS architectural protocol and delegates to a `BootOrchestrator` implementation.
- **BootOrchestrator**: Trait for custom boot flows. Platforms implement this to define boot behavior.
- **SimpleBootManager**: Default `BootOrchestrator` for platforms with straightforward boot topologies.

## Usage

```rust
use patina_boot::{BootDispatcher, SimpleBootManager, config::BootConfig};

// Minimal boot:
let orchestrator = SimpleBootManager::new(
    BootConfig::new(nvme_esp_path())
        .with_device(nvme_recovery_path()),
);
add.component(BootDispatcher::new(orchestrator));

// Custom orchestrator:
add.component(BootDispatcher::new(MyCustomOrchestrator::new()));
```

## Helper Functions

For custom boot flows, use the helper functions in the `helpers` module:

- `connect_all()` - Connect all controllers for device enumeration
- `signal_bds_phase_entry()` - Signal EndOfDxe event
- `signal_ready_to_boot()` - Signal ReadyToBoot event
- `discover_console_devices()` - Populate console variables
- `boot_from_device_path()` - Load and start a boot image
