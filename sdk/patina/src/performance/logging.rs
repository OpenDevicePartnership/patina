//! Functionality for logging performance measurements.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use r_efi::efi;

use crate::{
    performance::{
        Measurement, globals::get_perf_measurement_mask, measurement::CallerIdentifier, record::known::KnownPerfId,
    },
    uefi_protocol::performance_measurement::{CreateMeasurement, PerfAttribute},
};

/// Create performance record
///
/// `caller_identifier` is either a Handle or a pointer to a caller ID GUID.
fn log_perf_measurement(
    caller_identifier: CallerIdentifier,
    guid: Option<&efi::Guid>,
    string: Option<&str>,
    address: usize,
    identifier: u16,
    create_performance_measurement: CreateMeasurement,
) {
    if let Err(e) = (create_performance_measurement)(
        caller_identifier,
        guid,
        string,
        0,
        address,
        identifier,
        PerfAttribute::PerfEntry,
    ) {
        // We should not panic here as performance measurement failure should not block normal execution.
        // Instead, log and continue.
        log::error!("Failed to log performance measurement: {:?}", e);
    }
}

// Adds a record that records the start time of a performance measurement.
fn start_perf_measurement(
    handle: efi::Handle,
    token: Option<&str>,
    module: Option<&str>,
    timestamp: u64,
    identifier: u32,
    create_performance_measurement: CreateMeasurement,
) {
    let string = token.or(module);

    if let Err(e) = (create_performance_measurement)(
        CallerIdentifier::Handle(handle),
        None,
        string,
        timestamp,
        0,
        identifier as u16,
        PerfAttribute::PerfStartEntry,
    ) {
        log::error!("Failed to log start performance measurement: {:?}", e);
    }
}

// Adds a record that records the end time of a performance measurement.
fn end_perf_measurement(
    handle: efi::Handle,
    token: Option<&str>,
    module: Option<&str>,
    timestamp: u64,
    identifier: u32,
    create_performance_measurement: CreateMeasurement,
) {
    let string = token.or(module);

    if let Err(e) = (create_performance_measurement)(
        CallerIdentifier::Handle(handle),
        None,
        string,
        timestamp,
        0,
        identifier as u16,
        PerfAttribute::PerfEndEntry,
    ) {
        log::error!("Failed to log end performance measurement: {:?}", e);
    }
}

/// Begins performance measurement of start image in core.
pub fn perf_image_start_begin(module_handle: efi::Handle, create_performance_measurement: CreateMeasurement) {
    if get_perf_measurement_mask() & Measurement::StartImage as u32 == 0 {
        return;
    }
    log_perf_measurement(
        CallerIdentifier::Handle(module_handle),
        None,
        None,
        0,
        KnownPerfId::ModuleStart.as_u16(),
        create_performance_measurement,
    )
}

/// Ends performance measurement of start image in core.
pub fn perf_image_start_end(image_handle: efi::Handle, create_performance_measurement: CreateMeasurement) {
    if get_perf_measurement_mask() & Measurement::StartImage as u32 == 0 {
        return;
    }
    log_perf_measurement(
        CallerIdentifier::Handle(image_handle),
        None,
        None,
        0,
        KnownPerfId::ModuleEnd.as_u16(),
        create_performance_measurement,
    )
}

/// Begins performance measurement of load image in core.
pub fn perf_load_image_begin(module_handle: efi::Handle, create_performance_measurement: CreateMeasurement) {
    if get_perf_measurement_mask() & Measurement::LoadImage as u32 == 0 {
        return;
    }
    log_perf_measurement(
        CallerIdentifier::Handle(module_handle),
        None,
        None,
        0,
        KnownPerfId::ModuleLoadImageStart.as_u16(),
        create_performance_measurement,
    )
}

/// Ends performance measurement of load image in core.
pub fn perf_load_image_end(module_handle: efi::Handle, create_performance_measurement: CreateMeasurement) {
    if get_perf_measurement_mask() & Measurement::LoadImage as u32 == 0 {
        return;
    }
    log_perf_measurement(
        CallerIdentifier::Handle(module_handle),
        None,
        None,
        0,
        KnownPerfId::ModuleLoadImageEnd.as_u16(),
        create_performance_measurement,
    )
}

/// Begins performance measurement of driver binding support in the core.
pub fn perf_driver_binding_support_begin(
    driver_binding_handle: efi::Handle,
    controller_handle: efi::Handle,
    create_performance_measurement: CreateMeasurement,
) {
    if get_perf_measurement_mask() & Measurement::DriverBindingSupport as u32 == 0 {
        return;
    }
    log_perf_measurement(
        CallerIdentifier::Handle(driver_binding_handle),
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbSupportStart.as_u16(),
        create_performance_measurement,
    )
}

/// Ends performance measurement of driver binding support in the core.
pub fn perf_driver_binding_support_end(
    driver_binding_handle: efi::Handle,
    controller_handle: efi::Handle,
    create_performance_measurement: CreateMeasurement,
) {
    if get_perf_measurement_mask() & Measurement::DriverBindingSupport as u32 == 0 {
        return;
    }
    log_perf_measurement(
        CallerIdentifier::Handle(driver_binding_handle),
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbSupportEnd.as_u16(),
        create_performance_measurement,
    )
}

/// Begins performance measurement of driver binding start in the core.
pub fn perf_driver_binding_start_begin(
    driver_binding_handle: efi::Handle,
    controller_handle: efi::Handle,
    create_performance_measurement: CreateMeasurement,
) {
    if get_perf_measurement_mask() & Measurement::DriverBindingStart as u32 == 0 {
        return;
    }
    log_perf_measurement(
        CallerIdentifier::Handle(driver_binding_handle),
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbStart.as_u16(),
        create_performance_measurement,
    )
}

/// Ends performance measurement of driver binding start in the core.
pub fn perf_driver_binding_start_end(
    driver_binding_handle: efi::Handle,
    controller_handle: efi::Handle,
    create_performance_measurement: CreateMeasurement,
) {
    if get_perf_measurement_mask() & Measurement::DriverBindingStart as u32 == 0 {
        return;
    }
    log_perf_measurement(
        CallerIdentifier::Handle(driver_binding_handle),
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbEnd.as_u16(),
        create_performance_measurement,
    )
}

/// Begins performance measurement of driver binding stop in the core.
pub fn perf_driver_binding_stop_begin(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    create_performance_measurement: CreateMeasurement,
) {
    if get_perf_measurement_mask() & Measurement::DriverBindingStop as u32 == 0 {
        return;
    }
    log_perf_measurement(
        CallerIdentifier::Handle(module_handle),
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbStopStart.as_u16(),
        create_performance_measurement,
    )
}

/// Ends performance measurement of driver binding stop in the core.
pub fn perf_driver_binding_stop_end(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    create_performance_measurement: CreateMeasurement,
) {
    if get_perf_measurement_mask() & Measurement::DriverBindingStop as u32 == 0 {
        return;
    }
    log_perf_measurement(
        CallerIdentifier::Handle(module_handle),
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbStopEnd.as_u16(),
        create_performance_measurement,
    )
}

/// Measure the time from power-on to this function execution.
pub fn perf_event(event_string: &str, caller_id: &efi::Guid, create_performance_measurement: CreateMeasurement) {
    log_perf_measurement(
        CallerIdentifier::Guid(*caller_id),
        None,
        Some(event_string),
        0,
        KnownPerfId::PerfEvent.as_u16(),
        create_performance_measurement,
    )
}

/// Begins performance measurement of event signal behavior in any module.
pub fn perf_event_signal_begin(
    event_guid: &efi::Guid,
    fun_name: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        CallerIdentifier::Guid(*caller_id),
        Some(event_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfEventSignalStart.as_u16(),
        create_performance_measurement,
    )
}

/// Ends performance measurement of event signal behavior in any module.
pub fn perf_event_signal_end(
    event_guid: &efi::Guid,
    fun_name: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        CallerIdentifier::Guid(*caller_id),
        Some(event_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfEventSignalEnd.as_u16(),
        create_performance_measurement,
    )
}

/// Begins performance measurement of a callback function in any module.
pub fn perf_callback_begin(
    trigger_guid: &efi::Guid,
    fun_name: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        CallerIdentifier::Guid(*caller_id),
        Some(trigger_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfCallbackStart.as_u16(),
        create_performance_measurement,
    )
}

/// Ends performance measurement of a callback function in any module.
pub fn perf_callback_end(
    trigger_guid: &efi::Guid,
    fun_name: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        CallerIdentifier::Guid(*caller_id),
        Some(trigger_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfCallbackEnd.as_u16(),
        create_performance_measurement,
    )
}

/// Begin performance measurement of any function in any module.
pub fn perf_function_begin(fun_name: &str, caller_id: &efi::Guid, create_performance_measurement: CreateMeasurement) {
    log_perf_measurement(
        CallerIdentifier::Guid(*caller_id),
        None,
        Some(fun_name),
        0,
        KnownPerfId::PerfFunctionStart.as_u16(),
        create_performance_measurement,
    )
}

/// Ends performance measurement of any function in any module.
pub fn perf_function_end(fun_name: &str, caller_id: &efi::Guid, create_performance_measurement: CreateMeasurement) {
    log_perf_measurement(
        CallerIdentifier::Guid(*caller_id),
        None,
        Some(fun_name),
        0,
        KnownPerfId::PerfFunctionEnd.as_u16(),
        create_performance_measurement,
    )
}

/// Begin performance measurement of a behavior within one module.
pub fn perf_in_module_begin(
    measurement_str: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        CallerIdentifier::Guid(*caller_id),
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfInModuleStart.as_u16(),
        create_performance_measurement,
    )
}

/// Ends performance measurement of a behavior within one module.
pub fn perf_in_module_end(
    measurement_str: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        CallerIdentifier::Guid(*caller_id),
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfInModuleEnd.as_u16(),
        create_performance_measurement,
    )
}

/// Begins performance measurement of a behavior in different modules.
pub fn perf_cross_module_begin(
    measurement_str: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        CallerIdentifier::Guid(*caller_id),
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfCrossModuleStart.as_u16(),
        create_performance_measurement,
    )
}

/// Ends performance measurement of a behavior in different modules.
pub fn perf_cross_module_end(
    measurement_str: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        CallerIdentifier::Guid(*caller_id),
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfCrossModuleEnd.as_u16(),
        create_performance_measurement,
    )
}

/// Adds a record that records the start time of a performance measurement.
pub fn perf_start(
    handle: efi::Handle,
    token: Option<&str>,
    module: Option<&str>,
    timestamp: u64,
    create_performance_measurement: CreateMeasurement,
) {
    start_perf_measurement(handle, token, module, timestamp, 0, create_performance_measurement)
}

/// Adds a record that records the end time of a performance measurement.
pub fn perf_end(
    handle: efi::Handle,
    token: Option<&str>,
    module: Option<&str>,
    timestamp: u64,
    create_performance_measurement: CreateMeasurement,
) {
    end_perf_measurement(handle, token, module, timestamp, 0, create_performance_measurement)
}

/// Adds a record that records the start time of a performance measurement.
pub fn perf_start_ex(
    handle: efi::Handle,
    token: Option<&str>,
    module: Option<&str>,
    timestamp: u64,
    identifier: u32,
    create_performance_measurement: CreateMeasurement,
) {
    start_perf_measurement(handle, token, module, timestamp, identifier, create_performance_measurement)
}

/// Adds a record that records the end time of a performance measurement.
pub fn perf_end_ex(
    handle: efi::Handle,
    token: Option<&str>,
    module: Option<&str>,
    timestamp: u64,
    identifier: u32,
    create_performance_measurement: CreateMeasurement,
) {
    end_perf_measurement(handle, token, module, timestamp, identifier, create_performance_measurement)
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    fn mock_create_measurement_ok(
        _caller_identifier: CallerIdentifier,
        _guid: Option<&efi::Guid>,
        _string: Option<&str>,
        _ticker: u64,
        _address: usize,
        _identifier: u16,
        _attribute: PerfAttribute,
    ) -> Result<(), crate::performance::error::Error> {
        Ok(())
    }

    fn mock_create_measurement_err(
        _caller_identifier: CallerIdentifier,
        _guid: Option<&efi::Guid>,
        _string: Option<&str>,
        _ticker: u64,
        _address: usize,
        _identifier: u16,
        _attribute: PerfAttribute,
    ) -> Result<(), crate::performance::error::Error> {
        Err(crate::performance::error::Error::OutOfResources)
    }

    #[test]
    fn test_start_measurement() {
        // Test with token valid.
        start_perf_measurement(0x2 as efi::Handle, Some("TestToken"), None, 100, 1, mock_create_measurement_ok);

        // Test with module valid.
        start_perf_measurement(0x2 as efi::Handle, None, Some("TestModule"), 100, 2, mock_create_measurement_ok);

        // Test with both token and module invalid.
        start_perf_measurement(0x2 as efi::Handle, None, None, 100, 3, mock_create_measurement_ok);

        // Should handle internal error without panic.
        start_perf_measurement(0x2 as efi::Handle, Some("TestToken"), None, 100, 4, mock_create_measurement_err);
    }

    #[test]
    fn test_end_measurement() {
        // Test with token valid.
        end_perf_measurement(0x2 as efi::Handle, Some("TestToken"), None, 100, 1, mock_create_measurement_ok);

        // Test with module valid.
        end_perf_measurement(0x2 as efi::Handle, None, Some("TestModule"), 100, 2, mock_create_measurement_ok);

        // Test with both token and module invalid.
        end_perf_measurement(0x2 as efi::Handle, None, None, 100, 3, mock_create_measurement_ok);

        // Should handle error without panic.
        end_perf_measurement(0x2 as efi::Handle, Some("TestToken"), None, 100, 4, mock_create_measurement_err);
    }

    #[test]
    fn test_perf_instrumentation() {
        perf_start(0x2 as efi::Handle, Some("TestToken"), None, 100, mock_create_measurement_ok);
        perf_start_ex(0x2 as efi::Handle, Some("TestToken"), None, 100, 100, mock_create_measurement_ok);
        perf_end_ex(0x2 as efi::Handle, Some("TestToken"), None, 100, 100, mock_create_measurement_ok);
        perf_end(0x2 as efi::Handle, Some("TestToken"), None, 100, mock_create_measurement_ok);
    }
}
