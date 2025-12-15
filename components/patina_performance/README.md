# Patina Performance Component

The Patina performance component maintains the infrastructure to report firmware performance information.

## Responsibilities

- Initialize the FBPT and seed it with any measurements passed in performance data HOBs from prior boot phases.
- Track the current performance measurement mask and load-image count so event producers can filter their output.
- Publish performance properties through a configuration table and expose the measurement protocol
  (`EdkiiPerformanceMeasurement`) for C drivers that need to log performance data.
- Optionally merge Management Mode (MM) performance records when an MM communication region is available.
- Publish the FBPT so the operating system can consume it later.

## Configuration

- `PerfConfig.enable_component` must be set to enable the component.
- `PerfConfig.enabled_measurements` carries the bitmask of `patina::performance::Measurement` values that should be
  recorded.
- Platforms that need runtime configuration can include the `PerformanceConfigurationProvider` component, which reads a
  `PerformanceConfigHob` and locks the `PerfConfig` values for the session.

To enable performance measurements in Patina, add the `Performance` component and provide a `PerfConfig` using
`configs(mut add: Add<Config>)` in your platform's `ComponentInfo` implementation. For example:

```rust
use patina_performance::component::Performance;
use patina_performance::component::performance_config_provider::PerformanceConfigurationProvider;
use patina_dxe_core::*;

impl ComponentInfo for ExamplePlatform {
    fn configs(mut add: Add<Config>) {
        add.config(patina_performance::config::PerfConfig {
            enable_component: true,
            enabled_measurements: {
                patina::performance::Measurement::DriverBindingStart
                | patina::performance::Measurement::DriverBindingStop
                | patina::performance::Measurement::DriverBindingSupport
                | patina::performance::Measurement::LoadImage
                | patina::performance::Measurement::StartImage
            }
        });
    }

    fn components(mut add: Add<Component>) {
        // Add your other components as needed, e.g.:
        // add.component(AdvancedLoggerComponent::<YourUartType>::new(&LOGGER));
        add.component(PerformanceConfigurationProvider);
        add.component(Performance);
    }
}
```

> **Note:** The `PerformanceConfigurationProvider` component is optional. Include it if you want to allow runtime
configuration of performance measurements via a HOB. If you only want static configuration, you can omit it.

### Enabling Performance Measurements During Boot

A component called `PerformanceConfigurationProvider` is used to enable performance measurements during the boot
process. This component depends on a `PerformanceConfigHob` HOB to be produced during boot to determine whether the
performance component should be enabled and which measurements should be active.

If a platform needs to use a single Patina DXE Core and support firmware builds where performance measurements can
be enabled or disabled, it should produce a `PerformanceConfigHob` HOB during the boot process and include the
`PerformanceConfigurationProvider` component in the DXE Core build. The HOB can be populated by any platform-specific
logic, such as a PCD value or a build variable.

> **Note:** `PerformanceConfigurationProvider` will override the enabled measurements based on the HOB value.

## API

| Macro name in EDK II                                                  | Function name in Patina component                                        | Description                                                     |
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

The method to record performance measurements varies according to whether it is performed from within the core or an
external component.

*Example of measurement from within the core:*

```rust,no_run
# extern crate mu_rust_helpers;
# extern crate patina;
use patina::performance::{
   logging::perf_function_begin,
   measurement::create_performance_measurement,
};
use mu_rust_helpers::guid::CALLER_ID;

perf_function_begin("foo", &CALLER_ID, create_performance_measurement);
```

*Example of measurement from outside the core:*

```rust,no_run
# extern crate mu_rust_helpers;
# extern crate patina;
# let bs = patina::boot_services::StandardBootServices::new_uninit();
use mu_rust_helpers::guid::CALLER_ID;
use patina::{
   boot_services::BootServices,
   performance::logging::perf_function_begin,
   uefi_protocol::performance_measurement::EdkiiPerformanceMeasurement,
};

let create_performance_measurement = unsafe { bs.locate_protocol::<EdkiiPerformanceMeasurement>(None) }
 .map_or(None, |p| Some(p.create_performance_measurement));

create_performance_measurement.inspect(|f| perf_function_begin("foo", &CALLER_ID, *f));
```

## Performance Component Overview

The **Performance Component** provides an API for logging performance measurements during firmware execution. This
API includes:

- Utility functions to log specific events.
- A function to create performance measurements.

If the measurement is initiated from the core, use the `create_performance_measurement` function within the utility
function. Otherwise, use the function returned by the `EdkiiPerformanceMeasurement` protocol.

---

### Initialization and Setup

Upon initialization, the component performs the following steps:

1. **Initialize the Firmware Performance Data Table (FBPT)**

   - Sets up the FBPT data structure to store performance records.

2. **Populate FBPT with Pre-DXE Data**

   - Retrieves performance data from Hand-Off Blocks (HOBs) generated during the pre-DXE phase and adds them to the FBPT.

3. **Install the `EdkiiPerformanceMeasurement` Protocol**

   - Enables external modules to log performance data using the component API.

4. **Register Events**

   - One event collects performance records logged in Management Mode (MM).
   - Another event publishes the FBPT to allocate the table in reserved memory at the end of the DXE phase.

5. **Install Performance Properties**

   - Exposes performance-related properties through a configuration table for use by other components.

---

### Scope and Limitations

This component **only publishes the FBPT**, as it specifically manages the additional record fields within it.
Other tables, such as the **Firmware Performance Data Table (FPDT)**, are published by separate components.
