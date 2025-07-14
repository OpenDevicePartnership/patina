# Patina Performance

The Patina performance component is a native Rust implementation for managing firmware performance data.

## Enabling Performance Measurements

Enabling performance in Patina is done by adding the `Performance` component to the Patina DXE Core build.

```rust
// ...

Core::default()
 // ...
 .with_component(patina_performance::Performance)
 .start()
 .unwrap();

// ...
```

> **Note:** Performance measurements for a given platform may need to be enabled. For example, if building in
`patina-qemu`, this build variable should be set to true: `BLD_*_PERF_TRACE_ENABLE=TRUE`.

The Patina performance component uses a feature mask in its configuration to control how performance is measured.

```rust

// ...

Core::default()
 // ...
 .with_config(patina_performance::config::PerfConfig {
     enable_component: true,
     enabled_measurements: {
        patina_sdk::performance::Measurement::DriverBindingStart         // Adds driver binding start measurements.
        | patina_sdk::performance::Measurement::DriverBindingStop        // Adds driver binding stop measurements.
        | patina_sdk::performance::Measurement::DriverBindingSupport     // Adds driver binding support measurements.
        | patina_sdk::performance::Measurement::LoadImage                // Adds load image measurements.
        | patina_sdk::performance::Measurement::StartImage               // Adds start image measurements.
     }
 })
 .with_component(patina_performance::component::Performance))
 .start()
 .unwrap();

// ...
```

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

```rust
use mu_rust_helpers::guid::CALLER_ID;

perf_function_begin("foo" &CALLER_ID, create_performance_measurement);
```

*Example of measurement from outside the core:*

```rust
use mu_rust_helpers::guid::CALLER_ID;

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

## References

[**ACPI: Firmware Performance Data Table**](https://uefi.org/htmlspecs/ACPI_Spec_6_4_html/05_ACPI_Software_Programming_Model/ACPI_Software_Programming_Model.html?highlight=fbpt#firmware-performance-data-table-fpdt)

**Performance source code in the EDK II repository.**

- <https://github.com/tianocore/edk2/blob/master/MdePkg/Include/Library/PerformanceLib.h>
- <https://github.com/tianocore/edk2/blob/master/MdeModulePkg/Library/DxeCorePerformanceLib/DxeCorePerformanceLib.c>
