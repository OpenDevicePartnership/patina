# Patina Performance Component

The Patina performance component acts as the translation layer between the core's performance implementation, and the
UEFI and ACPI implementations.

## Responsibilities

- Publish the FBPT at End of DXE so the operating system can consume it later.
- Expose the EDK II measurement protocol (`EdkiiPerformanceMeasurement`) for C drivers that need to log performance
  data.
- Publish performance properties through a configuration table.
- Optionally merge Management Mode (MM) performance records when an MM communication region is available.

## Configuration

Whether performance measurement is enabled is decided by the DXE Core, not by this component. The core resolves the
performance configuration from a `PerformanceConfigHob`, falling back to the platform-provided
`PlatformInfo::DEFAULT_PERFORMANCE_CONFIG` when no such HOB is present. When performance is enabled the core
publishes the [`PerformanceManager`] service; when it is disabled that service is absent, so this component's
service dependency is unsatisfied and it does not dispatch.

A platform therefore enables performance in one of two ways:

1. Production of the `PerformanceConfigHob` prior to Patina DXE Core execution (this takes priority), or
2. Setting `PlatformInfo::DEFAULT_PERFORMANCE_CONFIG` to an enabled configuration with the desired `Measurement` values.

```rust,ignore
use patina::performance::config::PerformanceConfig;
use patina_dxe_core::*;

struct ExamplePlatform;

impl PlatformInfo for ExamplePlatform {
    // Optional override if the platform does not publish the performance configuration HOB.
    const DEFAULT_PERFORMANCE_CONFIG: PerformanceConfig = PerformanceConfig::new()
        .with_measurement(patina::performance::Measurement::DriverBindingStart) // Adds driver binding start measurements.
        .with_measurement(patina::performance::Measurement::DriverBindingStop)  // Adds driver binding stop measurements.
        .with_measurement(patina::performance::Measurement::LoadImage)          // Adds load image measurements.
        .with_measurement(patina::performance::Measurement::StartImage);        // Adds start image measurements.
}

impl ComponentInfo for ExamplePlatform {
    fn components(mut add: Add<Component>) {
        // The component dispatches only when the DXE Core enables performance measurement, via a performance
        // config HOB or the platform's `PlatformInfo::DEFAULT_PERFORMANCE_CONFIG` override.
        add.component(patina_performance::component::Performance::new());
    }
}
```

## API

The functions below are provided by the [`PerformanceManager`] service (produced by the DXE Core) and the core
internals; this component makes them reachable from external C modules through the `EdkiiPerformanceMeasurement`
protocol.

| Macro name in EDK II                                                  | Function name in Patina                                                  | Description                                                     |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------ | --------------------------------------------------------------- |
| `PERF_START_IMAGE_BEGIN` <br>`PERF_START_IMAGE_END`                   | `perf_image_start_begin`<br>`perf_image_start_end`                       | Measure the performance of start image in core.                 |
| `PERF_LOAD_IMAGE_BEGIN`<br>`PERF_LOAD_IMAGE_END`                      | `perf_load_image_begin`<br>`perf_load_image_end`                         | Measure the performance of load image in core.                  |
| `PERF_DRIVER_BINDING_SUPPORT_BEGIN` `PERF_DRIVER_BINDING_SUPPORT_END` | `perf_driver_binding_support_begin`<br>`perf_driver_binding_support_end` | Measure the performance of driver binding support in core.      |
| `PERF_DRIVER_BINDING_START_BEGIN`<br>`PERF_DRIVER_BINDING_START_END`  | `perf_driver_binding_start_begin`<br>`perf_driver_binding_start_end`     | Measure the performance of driver binding start in core.        |
| `PERF_DRIVER_BINDING_STOP_BEGIN`<br>`PERF_DRIVER_BINDING_STOP_END`    | `perf_driver_binding_stop_begin`<br>`perf_driver_binding_stop_end`       | Measure the performance of driver binding stop in core.         |
| `PERF_EVENT`                                                          | `perf_event`                                                             | Measure the time from power-on to this function execution.      |
| `PERF_EVENT_SIGNAL_BEGIN`<br>`PERF_EVENT_SIGNAL_END`                  | `perf_event_signal_begin`<br>`perf_event_signal_end`                     | Measure the performance of event signal behavior in any module. |
| `PERF_CALLBACK_BEGIN`<br>`PERF_CALLBACK_END`                          | `perf_callback_begin`<br>`perf_callback_end`                             | Measure the performance of a callback function in any module.   |
| `PERF_FUNCTION_BEGIN`<br>`PERF_FUNCTION_END`                          | `perf_function_begin`<br>`perf_function_end`                             | Measure the performance of a general function in any module.    |
| `PERF_INMODULE_BEGIN`<br>`PERF_INMODULE_END`                          | `perf_in_module_begin`<br>`perf_in_module_end`<br>                       | Measure the performance of a behavior within one module.        |
| `PERF_CROSSMODULE_BEGIN`<br>`PERF_CROSSMODULE_END`                    | `perf_cross_module_begin`<br>`perf_cross_module_end`                     | Measure the performance of a behavior in different modules.     |
| `PERF_START`<br>`PERF_START_EX`<br>`PERF_END`<br>`PERF_END_EX`        | `perf_start`<br>`perf_start_ex`<br>`perf_end`<br>`perf_end_ex`           | Make a performance measurement.                                 |

### Logging Performance Measurements

Performance measurements are recorded through the [`PerformanceManager`] service, which is produced by the DXE
Core and consumed both internally by the core and by components via dependency injection.

*Example of recording a measurement through the service:*

```rust,no_run
# extern crate patina;
use patina::component::service::{Service, performance::PerformanceManager};
use patina::guid::CALLER_ID;

fn record(perf: Service<dyn PerformanceManager>) {
    perf.perf_cross_module_begin("DXE", CALLER_ID.as_efi_guid());
}
```

[`PerformanceManager`]: patina::component::service::performance::PerformanceManager

## Performance Component Overview

The performance measurement API is provided by the [`PerformanceManager`] service, which is produced by the DXE
Core. This component contributes the UEFI-facing pieces on top of it:

- The EDK II Performance Measurement protocol, produced by this component, for use by external (C) modules.
- Publishing of the FBPT and performance properties.

Patina code (core or components) records measurements through the [`PerformanceManager`] service. External modules
use the function returned by the `EdkiiPerformanceMeasurement` protocol, which routes back into the same service.

---

### Initialization and Setup

The DXE Core initializes the FBPT, seeds it with any pre-DXE performance HOB data, and applies the measurement mask
before this component runs. Upon initialization, the component performs the following steps:

1. **Install the `EdkiiPerformanceMeasurement` Protocol**

   - Enables external modules to log performance data through the measurement service.

2. **Register Events**

   - One event collects performance records logged in Management Mode (MM).
   - Another event publishes the FBPT to allocate the table in reserved memory at the end of the DXE phase.

3. **Install Performance Properties**

   - Exposes performance-related properties through a configuration table for use by other components.

---

### Scope and Limitations

This component **only publishes the FBPT**, as it specifically manages the additional record fields within it.
Other tables, such as the **Firmware Performance Data Table (FPDT)**, are published by separate components.
