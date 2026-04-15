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

By default (e.g. `Performance::new()`), performance measurements are disabled. Performance measurements can then be
enabled by one of two ways:

1. Usage of the `Performance::with_measurements(...)` method to specify the bitmask of `Measurement` values that should
   be recorded.
2. Production of the `PerformanceConfigHob` prior to Patina DXE Core execution.

If both methods are used, the configuration via `PerformanceConfigHob` takes priority.

```rust
use patina_dxe_core::*;
use patina_performance::component::*;

struct ExampleComponent;

impl ComponentInfo for ExampleComponent {
  fn components(mut add: Add<Component>) {
    // Performance measurements are disabled by default, but can be overridden by a performance config HOB.
    add.component(Performance::new());

    // Performance measurements are enabled by default, but can be overridden by a performance config HOB.
    add.component(Performance::new().with_measurements(
       Measurement::DriverBindingStart
        | Measurement::DriverBindingStop
        | Measurement::DriverBindingSupport
        | Measurement::LoadImage
        | Measurement::StartImage
    ));
  }  
}
```

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
