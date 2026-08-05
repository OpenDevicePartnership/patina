//! Patina Testing Service
//!
//! This module defines the internal service used by the crate to register and execute tests marked with the
//! `#[patina_test]` attribute. The [`TestRunner`](crate::component::TestRunner) component checks for the presence of
//! the [Recorder] service, registering a new one if it does not. It then uses the Recorder service to register all
//! discovered tests based on the filtered list each individual `TestRunner` is configured to run. The Recorder service
//! is then responsible for executing the tests, recording their results, and logging the results at the appropriate
//! time during the boot process.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::{
    __private_api::{TestCase, TestTrigger},
    alloc::{boxed::Box, collections::BTreeMap, fmt::Display, string::String, vec::Vec},
};

use core::{cell::Cell, ptr::NonNull, time::Duration};

use patina::{
    BinaryGuid,
    component::{
        Storage,
        service::{
            IntoService, Service,
            uefi_services::{
                event::{Event, EventServices, EventServicesExt},
                timer_event::{TimerEventServices, TimerEventServicesExt, TimerType},
                tpl::Tpl,
            },
        },
    },
    uefi::event::{EXIT_BOOT_SERVICES_EVENT_GROUP_GUID, READY_TO_BOOT_EVENT_GROUP_GUID},
    writelncrlf,
};

/// Registers `callback` for `group`, closing the registration's own event the first time it fires so it runs at
/// most once even if the group is signaled again later in boot (e.g. a failed boot attempt re-signaling Ready To
/// Boot).
fn on_event_group_once(
    events: &Service<dyn EventServices>,
    group: BinaryGuid,
    mut callback: impl FnMut() + 'static,
) -> patina::error::Result<()> {
    let event_cell: &'static Cell<Option<Event>> = Box::leak(Box::new(Cell::new(None)));
    let closing_events = *events;
    let event = events.on_event_group(group, Tpl::Callback, move || {
        callback();
        if let Some(event) = event_cell.take() {
            let _ = closing_events.close_event(event);
        }
    })?;
    event_cell.set(Some(event));
    Ok(())
}

/// A structure containing all necessary data to execute a test at any time.
#[derive(Clone)]
pub(crate) struct TestRecord {
    /// Whether or not to log debug messages in the test or not
    debug_mode: bool,
    /// The test case to execute.
    test_case: &'static TestCase,
    /// Callback functions to be called on test failure.
    callback: Vec<fn(&'static str, &'static str)>,
    /// The number of times this test has executed and passed.
    pass: u32,
    /// The number of times this test has executed and failed.
    fail: u32,
    /// The error message from the most recent failure, if any.
    err_msg: Option<&'static str>,
}

#[allow(unused)]
impl TestRecord {
    /// Creates a new instance of `TestRecord`.
    pub fn new(
        debug_mode: bool,
        test_case: &'static TestCase,
        callback: Option<fn(&'static str, &'static str)>,
    ) -> Self {
        let callback = callback.into_iter().collect();
        Self { debug_mode, test_case, callback, pass: 0, fail: 0, err_msg: None }
    }

    pub fn name(&self) -> &'static str {
        self.test_case.name
    }

    /// Merges another test record into this one, combining their results and callbacks.
    fn merge(&mut self, other: &Self) {
        assert_eq!(self.test_case.name, other.test_case.name, "Can only merge records for the same test case.");
        self.debug_mode |= other.debug_mode;
        self.pass += other.pass;
        self.fail += other.fail;
        self.callback.extend(other.callback.clone());
        if self.err_msg.is_none() && other.err_msg.is_some() {
            self.err_msg = other.err_msg;
        }
    }

    /// Runs the test case case.
    ///
    /// Calls the test failure callbacks if the test fails.
    fn run(&mut self, storage: &mut Storage) {
        let result = self.test_case.run(storage, self.debug_mode);

        match result {
            Ok(()) => self.pass += 1,
            Err(msg) => {
                self.fail += 1;
                self.err_msg = Some(msg);
                self.callback.iter().for_each(|cb| cb(self.test_case.name, msg));
            }
        }
    }

    /// Schedules the test to be run according to its triggers.
    pub fn schedule_run(
        &self,
        storage: &mut Storage,
        events: &Service<dyn EventServices>,
        timer_events: &Service<dyn TimerEventServices>,
    ) -> patina::error::Result<()> {
        let name = self.test_case.name;

        for trigger in self.test_case.triggers {
            match trigger {
                TestTrigger::Manual => {
                    // Do nothing. Test must be manually triggered.
                }
                TestTrigger::Event(guid) => {
                    let storage_ptr = NonNull::from_ref(storage);
                    events.on_event_group(*guid, Tpl::Callback, move || Self::run_test(storage_ptr, name))?;
                }
                TestTrigger::Timer(interval) => {
                    let storage_ptr = NonNull::from_ref(storage);
                    let timer_event =
                        timer_events.on_timer_event(Tpl::Callback, move || Self::run_test(storage_ptr, name))?;

                    // Disable the timer at ReadyToBoot so it does not continue firing while a bootloader is running.
                    let timer_events_for_cancel = *timer_events;
                    on_event_group_once(events, READY_TO_BOOT_EVENT_GROUP_GUID, move || {
                        let _ = timer_events_for_cancel.set_timer(timer_event, TimerType::Cancel);
                    })?;

                    let period = Duration::from_nanos(interval.saturating_mul(100));
                    timer_events.set_timer(timer_event, TimerType::Periodic(period))?;
                }
            }
        }

        Ok(())
    }

    /// Serializes the test record to a JSON string for logging or reporting purposes.
    fn json(&self) -> String {
        alloc::format!(
            r#"{{"name":"{}","pass":{},"fail":{},"err_msg":{}}}"#,
            self.test_case.name,
            self.pass,
            self.fail,
            self.err_msg.map_or(String::from("null"), |msg| alloc::format!(r#""{msg}""#))
        )
    }

    /// Runs the named test via the [Recorder] registered in `storage`, if any.
    fn run_test(mut storage: NonNull<Storage>, name: &'static str) {
        // SAFETY: `storage` is derived from a `&mut Storage` the caller holds for the lifetime of the DXE Core, and
        // event notifications run serially, so no other mutable access can occur concurrently.
        let storage = unsafe { storage.as_mut() };

        if let Some(recorder) = storage.get_service::<Recorder>() {
            let _ = recorder.with_mut(|records| records.get_mut(name).map(|record| record.run(storage)));
        }
    }
}

/// A private service to record test results.
#[derive(IntoService, Default)]
#[service(Recorder)]
pub(crate) struct Recorder {
    records: spin::Mutex<BTreeMap<&'static str, TestRecord>>,
}

#[allow(unused)]
impl Recorder {
    /// Allows updates to the test records via a closure to ensure interior mutability safety.
    fn with_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut BTreeMap<&'static str, TestRecord>) -> R,
    {
        let mut records = self.records.lock();
        f(&mut records)
    }

    /// Registers UEFI event callbacks to log the test results at specific points in the boot process.
    pub fn initialize(&self, storage: &mut Storage, events: &Service<dyn EventServices>) -> patina::error::Result<()> {
        let storage_ptr = NonNull::from_ref(storage);

        // Log results at ready to boot.
        on_event_group_once(events, READY_TO_BOOT_EVENT_GROUP_GUID, move || Self::run_tests_and_report(storage_ptr))?;

        // Log results at exit boot services.
        on_event_group_once(events, EXIT_BOOT_SERVICES_EVENT_GROUP_GUID, move || {
            Self::run_tests_and_report(storage_ptr);
        })?;

        Ok(())
    }

    /// Returns true if a test with the given name is already registered, false otherwise.
    pub fn test_registered(&self, test_name: &str) -> bool {
        self.with_mut(|data| data.contains_key(test_name))
    }

    // Updates an existing record or inserts a new record if it does not exist.
    pub fn update_record(&self, record: TestRecord) {
        let name = record.test_case.name;

        self.with_mut(|data| {
            if let Some(existing_record) = data.get_mut(name) {
                existing_record.merge(&record);
            } else {
                data.insert(name, record);
            }
        });
    }

    /// Runs all tests that are triggered by the [`TestTrigger::Manual`] trigger if they have not been run before.
    pub(crate) fn run_manual_tests(&self, storage: &mut Storage) {
        self.with_mut(|data| {
            data.values_mut()
                .filter(|record| {
                    record.test_case.triggers.contains(&TestTrigger::Manual) && record.pass == 0 && record.fail == 0
                })
                .for_each(|record| record.run(storage));
        });
    }

    /// Serializes all test records to a JSON string for logging or reporting purposes.
    fn json(&self) -> String {
        self.with_mut(|records| {
            let mut json_records = String::from("[");
            for record in records.values() {
                json_records.push_str(&record.json());
                json_records.push(',');
            }
            if !records.is_empty() {
                json_records.pop();
            }
            json_records.push(']');
            json_records
        })
    }

    /// Runs the manually-triggered tests and logs the current results of patina-test.
    fn run_tests_and_report(mut storage: NonNull<Storage>) {
        // SAFETY: event callbacks are executed in series, so there exists no other mutable access to storage.
        let storage = unsafe { storage.as_mut() };

        if let Some(recorder) = storage.get_service::<Recorder>() {
            recorder.run_manual_tests(storage);

            log::info!("{}", *recorder);
            log::info!(r#"{{"patina_on_system_unit_test_results":{}}}"#, recorder.json());
        }
    }
}

impl Display for Recorder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.with_mut(|records| {
            let mut total_passes = 0;
            let mut total_fails = 0;
            writelncrlf!(f, "Patina on-system unit-test results:")?;
            for (name, record) in records.iter() {
                total_passes += record.pass;
                total_fails += record.fail;
                if record.fail == 0 && record.pass == 0 {
                    writelncrlf!(f, "  {name} ... not triggered")?;
                    continue;
                }
                if record.fail == 0 {
                    writelncrlf!(f, "  {name} ... ok ({} passes)", record.pass)?;
                } else {
                    writelncrlf!(
                        f,
                        "  {name} ... fail ({} fails, {} passes): {}",
                        record.fail,
                        record.pass,
                        record.err_msg.unwrap_or("<no error message>")
                    )?;
                }
            }
            writelncrlf!(f, "Patina on-system unit-test result totals: {total_passes} passes, {total_fails} fails")?;

            Ok(())
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    extern crate std;

    use super::*;
    use crate::{alloc::format, component::tests::*};

    #[test]
    fn test_recorder_records_results() {
        let recorder = Recorder::default();

        let mut tr1 = TestRecord::new(false, &TEST_CASE2, None);
        tr1.pass = 2;
        tr1.fail = 1;
        tr1.err_msg = Some("Failure 1");
        recorder.update_record(tr1);

        let mut tr2 = TestRecord::new(false, &TEST_CASE3, None);
        tr2.pass = 0;
        tr2.fail = 2;
        tr2.err_msg = Some("Failure 2");
        recorder.update_record(tr2);

        let mut tr3 = TestRecord::new(false, &TEST_CASE4, None);
        tr3.pass = 1;
        recorder.update_record(tr3);

        let output = format!("{recorder}");
        assert!(output.contains("test ... fail (1 fails, 2 passes): Failure 1"));
        assert!(output.contains("test_that_fails ... fail (2 fails, 0 passes): Failure 2"));
        assert!(output.contains("event_triggered_test ... ok (1 passes)"));
    }

    #[test]
    fn test_test_data_test_running() {
        let mut storage = Storage::new();
        storage.add_config(1_i32);
        storage.add_service(Recorder::default());

        let test_case = &TEST_CASE1;
        let mut test_data = TestRecord::new(false, test_case, None);

        test_data.run(&mut storage);

        let recorder = storage.get_service::<Recorder>().expect("Recorder service should be registered.");
        recorder.update_record(test_data);

        let output = format!("{}", *recorder);
        std::println!("{output}");
        assert!(output.contains("test ... ok (1 passes)"));
    }

    #[test]
    fn test_update_record_with_existing_record() {
        let mut record1 = TestRecord::new(false, &TEST_CASE1, Some(|_, _| ()));
        record1.pass = 1;
        record1.fail = 0;

        let mut record2 = TestRecord::new(true, &TEST_CASE1, Some(|_, _| ()));
        record2.pass = 0;
        record2.fail = 2;
        record2.err_msg = Some("Failure");

        let recorder = Recorder::default();
        recorder.update_record(record1);
        recorder.update_record(record2);

        let record = recorder.with_mut(|data| data.get(&TEST_CASE1.name).cloned().expect("Record should exist."));

        assert!(record.debug_mode);
        assert_eq!(record.pass, 1);
        assert_eq!(record.fail, 2);
        assert_eq!(record.err_msg, Some("Failure"));
        assert!(record.debug_mode);
        assert_eq!(record.callback.len(), 2);
    }

    #[test]
    fn test_run_test() {
        let mut storage = Storage::new();
        storage.add_config(1_i32);

        let recorder = Recorder::default();
        recorder.update_record(TestRecord::new(false, &TEST_CASE1, None));
        storage.add_service(recorder);

        TestRecord::run_test(NonNull::from_ref(&storage), "test");

        let recorder = storage.get_service::<Recorder>().expect("Recorder service should be registered.");
        let output = format!("{}", *recorder);
        assert!(output.contains("test ... ok (1 passes)"));
    }

    #[test]
    fn test_run_tests_and_report() {
        let mut storage = Storage::new();
        storage.add_config(1_i32);

        let recorder = Recorder::default();
        recorder.update_record(TestRecord::new(false, &TEST_CASE1, None));
        storage.add_service(recorder);

        Recorder::run_tests_and_report(NonNull::from_ref(&storage));

        // Check that the test run
        let recorder = storage.get_service::<Recorder>().expect("Recorder service should be registered.");
        let output = format!("{}", *recorder);
        assert!(output.contains("test ... ok (1 passes)"));
    }

    #[test]
    fn test_record_json_string() {
        let test_case = &TEST_CASE1;
        let mut record = TestRecord::new(false, test_case, None);
        record.pass = 2;
        record.fail = 1;
        record.err_msg = Some("Failure message");

        let json = record.json();
        assert_eq!(json, r#"{"name":"test","pass":2,"fail":1,"err_msg":"Failure message"}"#);

        record.err_msg = None;
        let json = record.json();
        assert_eq!(json, r#"{"name":"test","pass":2,"fail":1,"err_msg":null}"#);
    }

    #[test]
    fn test_recorder_json_string() {
        let recorder = Recorder::default();

        let mut tr1 = TestRecord::new(false, &TEST_CASE2, None);
        tr1.pass = 2;
        tr1.fail = 1;
        tr1.err_msg = Some("Failure 1");
        recorder.update_record(tr1);

        let mut tr2 = TestRecord::new(false, &TEST_CASE3, None);
        tr2.pass = 0;
        tr2.fail = 2;
        tr2.err_msg = Some("Failure 2");
        recorder.update_record(tr2);

        // Cannot guarantee order of records, so we will just check that the JSON string contains both records in the correct format.
        let json = recorder.json();
        assert!(json.contains(r#"{"name":"test","pass":2,"fail":1,"err_msg":"Failure 1"}"#));
        assert!(json.contains(r#"{"name":"test_that_fails","pass":0,"fail":2,"err_msg":"Failure 2"}"#));
        assert!(json.starts_with('[') && json.ends_with(']'));
        assert!(json.contains(','));
    }
}
