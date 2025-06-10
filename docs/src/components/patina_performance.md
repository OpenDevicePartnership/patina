# Patina Performance

The patina performance component is a native rust implementation for managing firmware performance data.

## How to enable performance measurements

Enabling performance in patina is done by adding the `Performance` component to the patina DXE Core build.

```rust
// ...

Core::default()
 // ...
 .with_component(patina_performance::Performance)
 .start()
 .unwrap();
 
// ...
```

Then enable performance when building. For example, if building in `patina-qemu`,
this build variable should be set to true: `BLD_*_PERF_TRACE_ENABLE=TRUE`.

The patina performance component uses a feature mask in its configuration to control how performance is measured.

```rust

// ...

Core::default()
 // ...
 .with_config(patina_performance::EnabledMeasurement(&[
     patina_performance::Measurement::DriverBindingStart, // Adds driver binding start measurements.
        patina_performance::Measurement::DriverBindingStop, // Adds driver binding stop measurements.
        patina_performance::Measurement::DriverBindingSupport, // Adds driver binding support measurements.
        patina_performance::Measurement::LoadImage, // Adds load image measurements.
        patina_performance::Measurement::StartImage, // Adds start image measurements.
    ]))
 .with_component(patina_performance::Performance)
 .start()
 .unwrap();
 
// ...
```

## API

| Macro name in edkII                                                   | Function name in rust component                                          | Description                                                     |
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
| `PERF_START`<br>`PERF_START_EX`<br>`PERF_END`<br>`PERF_END_EX`        | `perf_start`<br>`perf_start_ex`<br>`perf_end`<br>`perf_end_ex`           | Do a performance measurement.                                   |

### How to log a performance measurement with patina performance

Depending of if you are trying to do a performance measurement from the core or not, some difference apply.

*Example of measurement from the core:*

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

## General architecture

The performance component follows the behavior of `DxeCorePerformanceLib.c`.
This include, providing performance measurement interfaces and initialized the global data structure
for performance logging, the Firmware Basic Boot Performance Table (FBPT).
This data structure is define in the ACPI spec 5.2.23 Firmware Performance Data Table (FPDT).
Since the only thing optional for a normal boot, is the other boot performance records in the FBPT,
only this table is publish by the performance component and not the entirety of the FBPT.
The performance component also make sure to copy the performance records generated by pre DXE phase to the FBPT,
these records can be found in HOBs.  
It also install a protocol for allowing performance measurement outside the core, and then set some events to
add mm performance records and the publishing of the FBPT table so it can be found later and added to the FPDT.

## References

**ACPI: Firmware Performance Data Table**
<https://uefi.org/htmlspecs/ACPI_Spec_6_4_html/05_ACPI_Software_Programming_Model/ACPI_Software_Programming_Model.html?highlight=fbpt#firmware-performance-data-table-fpdt>

**Performance file in EDKII repository.**

- <https://github.com/tianocore/edk2/blob/master/MdePkg/Include/Library/PerformanceLib.h>
- <https://github.com/tianocore/edk2/blob/master/MdeModulePkg/Library/DxeCorePerformanceLib/DxeCorePerformanceLib.c>
