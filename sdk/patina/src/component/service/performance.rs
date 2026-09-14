//! Performance measurement service interface.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::standard::efi;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

use crate::performance::{
    error::Error,
    measurement::{CallerIdentifier, PerfAttribute},
    record::{GenericPerformanceRecord, known::KnownPerfId},
};

/// Service that records firmware performance measurements into the Firmware Basic Boot Performance Table (FBPT).
///
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait PerformanceManager: Send + Sync {
    /// Function to log performance record with event description and a timestamp.
    #[allow(clippy::too_many_arguments)]
    fn create_measurement<'a>(
        &self,
        caller_identifier: CallerIdentifier,
        guid: Option<&'a efi::Guid>,
        string: Option<&'a str>,
        ticker: u64,
        address: usize,
        perf_id: u16,
        attribute: PerfAttribute,
    ) -> Result<(), Error>;

    /// Adds an already-formed generic performance record to the FBPT.
    fn add_generic_record(&self, record: &GenericPerformanceRecord) -> Result<(), Error>;

    /// Returns the number of bytes that must be allocated to publish the Firmware Basic Boot Performance Table.
    fn published_table_size(&self) -> Result<usize, Error>;

    /// Serializes the tracked records into the caller-provided `buffer` and puts the table into a published state.
    fn publish_table(&self, buffer: &'static mut [u8]) -> Result<(), Error>;

    /// Begins performance measurement of a behavior in different modules.
    fn perf_cross_module_begin(&self, measurement_str: &str, caller_id: &efi::Guid) {
        let _ = self.create_measurement(
            CallerIdentifier::Guid(*caller_id),
            None,
            Some(measurement_str),
            0,
            0,
            KnownPerfId::PerfCrossModuleStart.as_u16(),
            PerfAttribute::PerfEntry,
        );
    }

    /// Ends performance measurement of a behavior in different modules.
    fn perf_cross_module_end(&self, measurement_str: &str, caller_id: &efi::Guid) {
        let _ = self.create_measurement(
            CallerIdentifier::Guid(*caller_id),
            None,
            Some(measurement_str),
            0,
            0,
            KnownPerfId::PerfCrossModuleEnd.as_u16(),
            PerfAttribute::PerfEntry,
        );
    }

    /// Measure the time from power-on to this function execution.
    fn perf_event(&self, event_string: &str, caller_id: &efi::Guid) {
        let _ = self.create_measurement(
            CallerIdentifier::Guid(*caller_id),
            None,
            Some(event_string),
            0,
            0,
            KnownPerfId::PerfEvent.as_u16(),
            PerfAttribute::PerfEntry,
        );
    }

    /// Begins performance measurement of event signal behavior in any module.
    fn perf_event_signal_begin(&self, event_guid: &efi::Guid, fun_name: &str, caller_id: &efi::Guid) {
        let _ = self.create_measurement(
            CallerIdentifier::Guid(*caller_id),
            Some(event_guid),
            Some(fun_name),
            0,
            0,
            KnownPerfId::PerfEventSignalStart.as_u16(),
            PerfAttribute::PerfEntry,
        );
    }

    /// Ends performance measurement of event signal behavior in any module.
    fn perf_event_signal_end(&self, event_guid: &efi::Guid, fun_name: &str, caller_id: &efi::Guid) {
        let _ = self.create_measurement(
            CallerIdentifier::Guid(*caller_id),
            Some(event_guid),
            Some(fun_name),
            0,
            0,
            KnownPerfId::PerfEventSignalEnd.as_u16(),
            PerfAttribute::PerfEntry,
        );
    }

    /// Begins performance measurement of a callback function in any module.
    fn perf_callback_begin(&self, trigger_guid: &efi::Guid, fun_name: &str, caller_id: &efi::Guid) {
        let _ = self.create_measurement(
            CallerIdentifier::Guid(*caller_id),
            Some(trigger_guid),
            Some(fun_name),
            0,
            0,
            KnownPerfId::PerfCallbackStart.as_u16(),
            PerfAttribute::PerfEntry,
        );
    }

    /// Ends performance measurement of a callback function in any module.
    fn perf_callback_end(&self, trigger_guid: &efi::Guid, fun_name: &str, caller_id: &efi::Guid) {
        let _ = self.create_measurement(
            CallerIdentifier::Guid(*caller_id),
            Some(trigger_guid),
            Some(fun_name),
            0,
            0,
            KnownPerfId::PerfCallbackEnd.as_u16(),
            PerfAttribute::PerfEntry,
        );
    }

    /// Begin performance measurement of any function in any module.
    fn perf_function_begin(&self, fun_name: &str, caller_id: &efi::Guid) {
        let _ = self.create_measurement(
            CallerIdentifier::Guid(*caller_id),
            None,
            Some(fun_name),
            0,
            0,
            KnownPerfId::PerfFunctionStart.as_u16(),
            PerfAttribute::PerfEntry,
        );
    }

    /// Ends performance measurement of any function in any module.
    fn perf_function_end(&self, fun_name: &str, caller_id: &efi::Guid) {
        let _ = self.create_measurement(
            CallerIdentifier::Guid(*caller_id),
            None,
            Some(fun_name),
            0,
            0,
            KnownPerfId::PerfFunctionEnd.as_u16(),
            PerfAttribute::PerfEntry,
        );
    }

    /// Begin performance measurement of a behavior within one module.
    fn perf_in_module_begin(&self, measurement_str: &str, caller_id: &efi::Guid) {
        let _ = self.create_measurement(
            CallerIdentifier::Guid(*caller_id),
            None,
            Some(measurement_str),
            0,
            0,
            KnownPerfId::PerfInModuleStart.as_u16(),
            PerfAttribute::PerfEntry,
        );
    }

    /// Ends performance measurement of a behavior within one module.
    fn perf_in_module_end(&self, measurement_str: &str, caller_id: &efi::Guid) {
        let _ = self.create_measurement(
            CallerIdentifier::Guid(*caller_id),
            None,
            Some(measurement_str),
            0,
            0,
            KnownPerfId::PerfInModuleEnd.as_u16(),
            PerfAttribute::PerfEntry,
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use std::string::String;
    use std::sync::Mutex;
    use std::vec::Vec;

    struct Recorded {
        caller_guid: Option<efi::Guid>,
        guid: Option<efi::Guid>,
        string: Option<String>,
        perf_id: u16,
        attribute: PerfAttribute,
    }

    /// Test implementation that records every `create_measurement` call so the default trait methods can be verified.
    struct RecordingService {
        calls: Mutex<Vec<Recorded>>,
    }

    impl RecordingService {
        fn new() -> Self {
            Self { calls: Mutex::new(Vec::new()) }
        }

        fn take_last(&self) -> Recorded {
            self.calls.lock().unwrap().pop().expect("a measurement should have been recorded")
        }
    }

    impl PerformanceManager for RecordingService {
        fn create_measurement(
            &self,
            caller_identifier: CallerIdentifier,
            guid: Option<&efi::Guid>,
            string: Option<&str>,
            _ticker: u64,
            _address: usize,
            perf_id: u16,
            attribute: PerfAttribute,
        ) -> Result<(), Error> {
            self.calls.lock().unwrap().push(Recorded {
                caller_guid: caller_identifier.as_guid().copied(),
                guid: guid.copied(),
                string: string.map(std::string::ToString::to_string),
                perf_id,
                attribute,
            });
            Ok(())
        }

        fn add_generic_record(&self, _record: &GenericPerformanceRecord) -> Result<(), Error> {
            Ok(())
        }

        fn published_table_size(&self) -> Result<usize, Error> {
            Ok(0)
        }

        fn publish_table(&self, _buffer: &'static mut [u8]) -> Result<(), Error> {
            Ok(())
        }
    }

    const CALLER: efi::Guid = efi::Guid::from_bytes(&[1; 16]);
    const EVENT: efi::Guid = efi::Guid::from_bytes(&[2; 16]);

    #[test]
    fn test_performance_measurement_cross_module_markers() {
        let service = RecordingService::new();

        service.perf_cross_module_begin("phase", &CALLER);
        let call = service.take_last();
        assert_eq!(call.perf_id, KnownPerfId::PerfCrossModuleStart.as_u16());
        assert_eq!(call.attribute, PerfAttribute::PerfEntry);
        assert_eq!(call.caller_guid, Some(CALLER));
        assert!(call.guid.is_none());
        assert_eq!(call.string.as_deref(), Some("phase"));

        service.perf_cross_module_end("phase", &CALLER);
        assert_eq!(service.take_last().perf_id, KnownPerfId::PerfCrossModuleEnd.as_u16());
    }

    #[test]
    fn test_performance_measurement_event() {
        let service = RecordingService::new();

        service.perf_event("boot", &CALLER);
        let call = service.take_last();
        assert_eq!(call.perf_id, KnownPerfId::PerfEvent.as_u16());
        assert_eq!(call.caller_guid, Some(CALLER));
        assert!(call.guid.is_none());
        assert_eq!(call.string.as_deref(), Some("boot"));
    }

    #[test]
    fn test_performance_measurement_event_signal_markers() {
        let service = RecordingService::new();

        service.perf_event_signal_begin(&EVENT, "signal_fn", &CALLER);
        let call = service.take_last();
        assert_eq!(call.perf_id, KnownPerfId::PerfEventSignalStart.as_u16());
        assert_eq!(call.guid, Some(EVENT));
        assert_eq!(call.caller_guid, Some(CALLER));
        assert_eq!(call.string.as_deref(), Some("signal_fn"));

        service.perf_event_signal_end(&EVENT, "signal_fn", &CALLER);
        assert_eq!(service.take_last().perf_id, KnownPerfId::PerfEventSignalEnd.as_u16());
    }

    #[test]
    fn test_performance_measurement_callback_markers() {
        let service = RecordingService::new();

        service.perf_callback_begin(&EVENT, "cb_fn", &CALLER);
        let call = service.take_last();
        assert_eq!(call.perf_id, KnownPerfId::PerfCallbackStart.as_u16());
        assert_eq!(call.guid, Some(EVENT));
        assert_eq!(call.string.as_deref(), Some("cb_fn"));

        service.perf_callback_end(&EVENT, "cb_fn", &CALLER);
        assert_eq!(service.take_last().perf_id, KnownPerfId::PerfCallbackEnd.as_u16());
    }

    #[test]
    fn test_performance_measurement_function_markers() {
        let service = RecordingService::new();

        service.perf_function_begin("fn_name", &CALLER);
        let call = service.take_last();
        assert_eq!(call.perf_id, KnownPerfId::PerfFunctionStart.as_u16());
        assert!(call.guid.is_none());
        assert_eq!(call.string.as_deref(), Some("fn_name"));

        service.perf_function_end("fn_name", &CALLER);
        assert_eq!(service.take_last().perf_id, KnownPerfId::PerfFunctionEnd.as_u16());
    }

    #[test]
    fn test_performance_measurement_in_module_markers() {
        let service = RecordingService::new();

        service.perf_in_module_begin("work", &CALLER);
        let call = service.take_last();
        assert_eq!(call.perf_id, KnownPerfId::PerfInModuleStart.as_u16());
        assert!(call.guid.is_none());
        assert_eq!(call.string.as_deref(), Some("work"));

        service.perf_in_module_end("work", &CALLER);
        assert_eq!(service.take_last().perf_id, KnownPerfId::PerfInModuleEnd.as_u16());
    }
}
