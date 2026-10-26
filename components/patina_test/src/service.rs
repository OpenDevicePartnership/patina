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
    alloc::{boxed::Box, collections::BTreeMap, fmt::Display, vec::Vec},
};

use core::{ptr::NonNull, time::Duration};

use patina::{
    component::{
        Storage,
        service::{
            IntoService, Service,
            uefi_services::{
                event::{EventError, EventServices, EventServicesExt, Tpl},
                timer_event::{TimerEventServices, TimerEventServicesExt, TimerType},
            },
        },
    },
    writelncrlf,
};

struct DisplayFn<F>(F);

impl<F> Display for DisplayFn<F>
where
    F: Fn(&mut core::fmt::Formatter<'_>) -> core::fmt::Result,
{
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        (self.0)(formatter)
    }
}

fn display_fn<F>(format: F) -> DisplayFn<F>
where
    F: Fn(&mut core::fmt::Formatter<'_>) -> core::fmt::Result,
{
    DisplayFn(format)
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
    /// Whether the test is skipped, i.e. it is marked `#[skip]` or filtered out by every runner that registered it.
    /// Skipped tests are recorded so they can be reported, but are never scheduled to run.
    skipped: bool,
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
        Self { debug_mode, test_case, callback, pass: 0, fail: 0, err_msg: None, skipped: false }
    }

    /// Creates a record for a skipped test, i.e. one that is marked `#[skip]` or filtered out.
    pub fn skipped(test_case: &'static TestCase) -> Self {
        Self { skipped: true, ..Self::new(false, test_case, None) }
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
        // A test is only skipped if every runner skipped it.
        self.skipped &= other.skipped;
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
        events: Service<dyn EventServices>,
        timer: Service<dyn TimerEventServices>,
        recorder: &'static Recorder,
        storage: &mut Storage,
    ) -> patina::error::Result<()> {
        let name = self.test_case.name;
        let mut storage = NonNull::from_mut(storage);

        for trigger in self.test_case.triggers {
            let result = match trigger {
                TestTrigger::Manual => Ok(()),
                TestTrigger::Event(guid) => events
                    .on_event_group(*guid, Tpl::Callback, move || {
                        // SAFETY: event callbacks are executed in series, so there exists no other mutable access to storage.
                        let mut storage = unsafe { storage.as_mut() };
                        recorder.with_mut(|records| records.get_mut(name).map(|record| record.run(storage)));
                    })
                    .map(|_| ()),
                TestTrigger::Timer(interval) => (|| -> Result<(), EventError> {
                    // Create a timer event for the specified interval. Cancel the timer when the RTB event occurs.
                    let timer_event = timer.on_timer_event(Tpl::Callback, move || {
                        // SAFETY: event callbacks are executed in series, so there exists no other mutable access to storage.
                        let mut storage = unsafe { storage.as_mut() };
                        recorder.with_mut(|records| records.get_mut(name).map(|record| record.run(storage)));
                    })?;

                    // Schedule the event to clean up the timer when the RTB event occurs.
                    let cleanup_event = events.create_event_for_group(
                        patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID,
                        Tpl::Callback,
                        Box::new(move |rtb_event| {
                            let cancel_result = timer.set_timer(timer_event, TimerType::Cancel);
                            let timer_close_result = events.close_event(timer_event);
                            let rtb_close_result = events.close_event(rtb_event);
                            if let Err(error) = cancel_result.and(timer_close_result).and(rtb_close_result) {
                                log::error!("Failed to clean up Timer trigger for patina_test {name}: {error:?}");
                            }
                        }),
                    );

                    // Close the timer event if we failed to create the cleanup event.
                    let cleanup_event = match cleanup_event {
                        Ok(cleanup_event) => cleanup_event,
                        Err(error) => {
                            return events.close_event(timer_event).and(Err(error));
                        }
                    };

                    // Close the timer event and the cleanup event if setting the periodic timer fails.
                    let interval = Duration::from_nanos(interval.saturating_mul(100));
                    if let Err(error) = timer.set_timer(timer_event, TimerType::Periodic(interval)) {
                        let timer_close_result = events.close_event(timer_event);
                        let cleanup_close_result = events.close_event(cleanup_event);
                        return timer_close_result.and(cleanup_close_result).and(Err(error));
                    }

                    Ok(())
                })(),
            };

            if let Err(error) = result {
                log::error!("Failed to schedule {trigger:?} trigger for patina_test {name}: {error:?}");
            }
        }

        Ok(())
    }

    /// Returns true if the test was scheduled to run, but has not run yet, i.e. it has neither passed nor failed.
    fn is_not_triggered(&self) -> bool {
        !self.skipped && self.pass == 0 && self.fail == 0
    }

    /// Formats the test record as JSON for logging or reporting purposes. This must not allocate because it may be
    /// called during `ExitBootServices`.
    fn json(&self) -> impl Display + '_ {
        display_fn(move |formatter| {
            write!(
                formatter,
                r#"{{"name":"{}","pass":{},"fail":{},"skipped":{},"err_msg":"#,
                self.test_case.name, self.pass, self.fail, self.skipped
            )?;
            match self.err_msg {
                Some(message) => write!(formatter, "\"{message}\""),
                None => formatter.write_str("null"),
            }?;
            formatter.write_str("}")
        })
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
    pub fn initialize(
        &'static self,
        events: Service<dyn EventServices>,
        storage: &mut Storage,
    ) -> patina::error::Result<()> {
        let mut storage = NonNull::from_mut(storage);

        let run_tests_and_close = move |event| {
            // SAFETY: event callbacks are executed in series, so there exists no other mutable access to storage.
            let storage = unsafe { storage.as_mut() };
            self.run_tests_and_report(storage);
            events.close_event(event);
        };

        events.on_event_group_self_managed(
            patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID,
            Tpl::Callback,
            run_tests_and_close,
        )?;

        events.on_event_group_self_managed(
            patina::uefi::event::EXIT_BOOT_SERVICES_EVENT_GROUP_GUID,
            Tpl::Callback,
            run_tests_and_close,
        )?;

        Ok(())
    }

    /// Returns true if a test with the given name is already scheduled to run, i.e. it is recorded and not skipped.
    pub fn is_test_scheduled(&self, test_name: &str) -> bool {
        self.with_mut(|data| data.get(test_name).is_some_and(|record| !record.skipped))
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
                .filter(|record| record.test_case.triggers.contains(&TestTrigger::Manual) && record.is_not_triggered())
                .for_each(|record| record.run(storage));
        });
    }

    /// Formats all test records as JSON. This must not allocate because it can be called during `ExitBootServices`.
    fn json(&self) -> impl Display + '_ {
        display_fn(move |formatter| {
            self.with_mut(|records| {
                formatter.write_str("[")?;
                for (index, record) in records.values().enumerate() {
                    if index != 0 {
                        formatter.write_str(",")?;
                    }
                    write!(formatter, "{}", record.json())?;
                }
                formatter.write_str("]")
            })
        })
    }

    fn run_tests_and_report(&self, storage: &mut Storage) {
        self.run_manual_tests(storage);

        log::info!("{}", *self);
        log::info!(r#"{{"patina_on_system_unit_test_results":{}}}"#, self.json());
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
                if record.skipped {
                    writelncrlf!(f, "  {name} ... skipped")?;
                    continue;
                }
                if record.is_not_triggered() {
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

    use std::boxed::Box;

    use super::*;
    use crate::{
        alloc::{format, string::String},
        component::tests::*,
    };

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
        assert!(output.contains("totals: 3 passes, 3 fails"));
    }

    #[test]
    fn test_recorder_distinguishes_skipped_and_not_triggered_tests() {
        let recorder = Recorder::default();
        recorder.update_record(TestRecord::new(false, &TEST_CASE4, None));
        recorder.update_record(TestRecord::skipped(&TEST_CASE5));

        let mut ran = TestRecord::new(false, &TEST_CASE2, None);
        ran.pass = 1;
        recorder.update_record(ran);

        let output = format!("{recorder}");
        assert!(output.contains("event_triggered_test ... not triggered"));
        assert!(output.contains("timer_triggered_test ... skipped"));
        assert!(output.contains("test ... ok (1 passes)"));
        assert!(output.contains("totals: 1 passes, 0 fails"));

        let json = format!("{}", recorder.json());
        assert!(json.contains(r#"{"name":"event_triggered_test","pass":0,"fail":0,"skipped":false,"err_msg":null}"#));
        assert!(json.contains(r#"{"name":"timer_triggered_test","pass":0,"fail":0,"skipped":true,"err_msg":null}"#));
        assert!(json.contains(r#"{"name":"test","pass":1,"fail":0,"skipped":false,"err_msg":null}"#));
    }

    #[test]
    fn test_test_data_test_running() {
        let mut storage = Storage::new();
        storage.add_config(1_i32);
        let recorder = Recorder::default();

        let test_case = &TEST_CASE1;
        let mut test_data = TestRecord::new(false, test_case, None);

        test_data.run(&mut storage);

        recorder.update_record(test_data);

        let output = format!("{recorder}");
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
    fn test_event_trigger_runs_test_record() {
        use patina::BinaryGuid;
        use patina::component::service::uefi_services::{
            event::{Event, EventNotifyCallback, MockEventServices},
            timer_event::MockTimerEventServices,
        };
        use std::{cell::RefCell, rc::Rc};

        fn dummy_event() -> Event {
            Event::from_raw(core::ptr::NonNull::<core::ffi::c_void>::dangling().as_ptr())
                .expect("a dangling non-null pointer should produce a test event")
        }

        let callback: Rc<RefCell<Option<EventNotifyCallback>>> = Rc::new(RefCell::new(None));
        let callback_for_mock = Rc::clone(&callback);
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().once().returning_st(move |group, tpl, event_callback| {
            assert_eq!(group, BinaryGuid::from_bytes(&[0; 16]));
            assert_eq!(tpl, Tpl::Callback);
            callback_for_mock.replace(Some(event_callback));
            Ok(dummy_event())
        });

        let mut storage = Storage::new();
        let recorder: &'static Recorder = Box::leak(Box::new(Recorder::default()));
        let record = TestRecord::new(false, &TEST_CASE4, None);
        let events: Service<dyn EventServices> = Service::mock(Box::new(events));
        let timer: Service<dyn TimerEventServices> = Service::mock(Box::new(MockTimerEventServices::new()));

        record.schedule_run(events, timer, recorder, &mut storage).expect("event test scheduling should succeed");
        recorder.update_record(record);

        let mut callback = callback.borrow_mut().take().expect("event callback should be registered");
        callback(dummy_event());

        let output = format!("{recorder}");
        assert!(output.contains("event_triggered_test ... fail (1 fails, 0 passes): Intentional Failure"));
    }

    #[test]
    fn test_timer_creation_failure_continues_to_next_trigger() {
        use patina::BinaryGuid;
        use patina::component::service::uefi_services::{
            event::{Event, EventError, MockEventServices},
            timer_event::MockTimerEventServices,
        };

        let triggers =
            Box::leak(Box::new([TestTrigger::Timer(1_000_000), TestTrigger::Event(BinaryGuid::from_bytes(&[0; 16]))]));
        let test_case = Box::leak(Box::new(TestCase {
            name: "timer_creation_failure",
            triggers,
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: TEST_CASE4.func,
        }));

        let mut timer = MockTimerEventServices::new();
        timer.expect_create_timer_event().once().returning(|_, _| Err(EventError::Internal));

        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().once().returning(|group, tpl, _| {
            assert_eq!(group, BinaryGuid::from_bytes(&[0; 16]));
            assert_eq!(tpl, Tpl::Callback);
            Event::from_raw(core::ptr::NonNull::<core::ffi::c_void>::dangling().as_ptr()).ok_or(EventError::Internal)
        });

        let mut storage = Storage::new();
        let recorder: &'static Recorder = Box::leak(Box::new(Recorder::default()));
        let record = TestRecord::new(false, test_case, None);
        let events: Service<dyn EventServices> = Service::mock(Box::new(events));
        let timer: Service<dyn TimerEventServices> = Service::mock(Box::new(timer));

        assert!(record.schedule_run(events, timer, recorder, &mut storage).is_ok());
    }

    #[test]
    fn test_ready_to_boot_creation_failure_closes_timer_without_arming_it() {
        use patina::component::service::uefi_services::{
            event::{Event, EventError, MockEventServices},
            timer_event::MockTimerEventServices,
        };

        let timer_event =
            Event::from_raw(core::ptr::NonNull::<core::ffi::c_void>::dangling().as_ptr()).expect("non-null event");
        let mut timer = MockTimerEventServices::new();
        timer.expect_create_timer_event().once().returning_st(move |_, _| Ok(timer_event));

        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().once().returning(|group, tpl, _| {
            assert_eq!(group, patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID);
            assert_eq!(tpl, Tpl::Callback);
            Err(EventError::Internal)
        });
        events.expect_close_event().once().returning_st(move |event| {
            assert_eq!(event, timer_event);
            Ok(())
        });

        let mut storage = Storage::new();
        let recorder: &'static Recorder = Box::leak(Box::new(Recorder::default()));
        let record = TestRecord::new(false, &TEST_CASE5, None);
        let events: Service<dyn EventServices> = Service::mock(Box::new(events));
        let timer: Service<dyn TimerEventServices> = Service::mock(Box::new(timer));

        assert!(record.schedule_run(events, timer, recorder, &mut storage).is_ok());
    }

    #[test]
    fn test_timer_arm_failure_closes_timer_and_ready_to_boot_events() {
        use patina::component::service::uefi_services::{
            event::{Event, EventError, MockEventServices},
            timer_event::MockTimerEventServices,
        };
        use std::{cell::RefCell, rc::Rc};

        fn test_event(address: usize) -> Event {
            Event::from_raw(address as *mut core::ffi::c_void).expect("a non-zero address should produce a test event")
        }

        let timer_event = test_event(1);
        let ready_to_boot_event = test_event(2);
        let mut timer = MockTimerEventServices::new();
        timer.expect_create_timer_event().once().returning_st(move |_, _| Ok(timer_event));
        timer.expect_set_timer().once().returning(|_, _| Err(EventError::Internal));

        let closed_events: Rc<RefCell<Vec<Event>>> = Rc::new(RefCell::new(Vec::new()));
        let closed_events_for_mock = Rc::clone(&closed_events);
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().once().returning_st(move |group, tpl, _| {
            assert_eq!(group, patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID);
            assert_eq!(tpl, Tpl::Callback);
            Ok(ready_to_boot_event)
        });
        events.expect_close_event().times(2).returning_st(move |event| {
            closed_events_for_mock.borrow_mut().push(event);
            Ok(())
        });

        let mut storage = Storage::new();
        let recorder: &'static Recorder = Box::leak(Box::new(Recorder::default()));
        let record = TestRecord::new(false, &TEST_CASE5, None);
        let events: Service<dyn EventServices> = Service::mock(Box::new(events));
        let timer: Service<dyn TimerEventServices> = Service::mock(Box::new(timer));

        assert!(record.schedule_run(events, timer, recorder, &mut storage).is_ok());
        assert_eq!(closed_events.borrow().as_slice(), &[timer_event, ready_to_boot_event]);
    }

    #[test]
    fn test_timer_trigger_runs_test_and_ready_to_boot_cleans_up() {
        use patina::component::service::uefi_services::{
            event::{Event, EventNotifyCallback, MockEventServices},
            timer_event::MockTimerEventServices,
        };
        use std::{cell::RefCell, rc::Rc};

        fn test_event(address: usize) -> Event {
            Event::from_raw(address as *mut core::ffi::c_void).expect("a non-zero address should produce a test event")
        }

        let timer_event = test_event(1);
        let ready_to_boot_event = test_event(2);

        let timer_callback: Rc<RefCell<Option<EventNotifyCallback>>> = Rc::new(RefCell::new(None));
        let timer_callback_for_mock = Rc::clone(&timer_callback);
        let timer_settings: Rc<RefCell<Vec<(Event, TimerType)>>> = Rc::new(RefCell::new(Vec::new()));
        let timer_settings_for_mock = Rc::clone(&timer_settings);
        let mut timer = MockTimerEventServices::new();
        timer.expect_create_timer_event().once().returning_st(move |tpl, callback| {
            assert_eq!(tpl, Tpl::Callback);
            timer_callback_for_mock.replace(Some(callback));
            Ok(timer_event)
        });
        timer.expect_set_timer().times(2).returning_st(move |event, timer_type| {
            timer_settings_for_mock.borrow_mut().push((event, timer_type));
            Ok(())
        });

        let ready_to_boot_callback: Rc<RefCell<Option<EventNotifyCallback>>> = Rc::new(RefCell::new(None));
        let ready_to_boot_callback_for_mock = Rc::clone(&ready_to_boot_callback);
        let closed_events: Rc<RefCell<Vec<Event>>> = Rc::new(RefCell::new(Vec::new()));
        let closed_events_for_mock = Rc::clone(&closed_events);
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().once().returning_st(move |group, tpl, callback| {
            assert_eq!(group, patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID);
            assert_eq!(tpl, Tpl::Callback);
            ready_to_boot_callback_for_mock.replace(Some(callback));
            Ok(ready_to_boot_event)
        });
        events.expect_close_event().times(2).returning_st(move |event| {
            closed_events_for_mock.borrow_mut().push(event);
            Ok(())
        });

        let mut storage = Storage::new();
        let recorder: &'static Recorder = Box::leak(Box::new(Recorder::default()));
        let record = TestRecord::new(false, &TEST_CASE5, None);
        let events: Service<dyn EventServices> = Service::mock(Box::new(events));
        let timer: Service<dyn TimerEventServices> = Service::mock(Box::new(timer));

        record.schedule_run(events, timer, recorder, &mut storage).expect("timer test scheduling should succeed");
        recorder.update_record(record);

        let mut callback = timer_callback.borrow_mut().take().expect("timer callback should be registered");
        callback(timer_event);

        let output = format!("{recorder}");
        assert!(output.contains("timer_triggered_test ... fail (1 fails, 0 passes): Intentional Failure"));

        let mut callback =
            ready_to_boot_callback.borrow_mut().take().expect("ready-to-boot callback should be registered");
        callback(ready_to_boot_event);

        assert_eq!(
            timer_settings.borrow().as_slice(),
            &[(timer_event, TimerType::Periodic(Duration::from_millis(100))), (timer_event, TimerType::Cancel),]
        );
        assert_eq!(closed_events.borrow().as_slice(), &[timer_event, ready_to_boot_event]);
    }

    #[test]
    fn test_recorder_lifecycle_events_run_tests_and_close_events() {
        use patina::component::service::uefi_services::event::{Event, EventNotifyCallback, MockEventServices};
        use std::{cell::RefCell, rc::Rc};

        fn test_event(address: usize) -> Event {
            Event::from_raw(address as *mut core::ffi::c_void).expect("a non-zero address should produce a test event")
        }

        type RegisteredCallback = (patina::BinaryGuid, Tpl, Event, EventNotifyCallback);

        let callbacks: Rc<RefCell<Vec<RegisteredCallback>>> = Rc::new(RefCell::new(Vec::new()));
        let callbacks_for_mock = Rc::clone(&callbacks);
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().times(2).returning_st(move |group, tpl, callback| {
            let event = if group == patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID {
                test_event(1)
            } else {
                assert_eq!(group, patina::uefi::event::EXIT_BOOT_SERVICES_EVENT_GROUP_GUID);
                test_event(2)
            };

            callbacks_for_mock.borrow_mut().push((group, tpl, event, callback));
            Ok(event)
        });

        let closed_events: Rc<RefCell<Vec<Event>>> = Rc::new(RefCell::new(Vec::new()));
        let closed_events_for_mock = Rc::clone(&closed_events);
        events.expect_close_event().times(2).returning_st(move |event| {
            closed_events_for_mock.borrow_mut().push(event);
            Ok(())
        });

        let mut storage = Storage::new();
        storage.add_config(1_i32);

        let recorder: &'static Recorder = Box::leak(Box::new(Recorder::default()));
        recorder.update_record(TestRecord::new(false, &TEST_CASE1, None));

        let events: Service<dyn EventServices> = Service::mock(Box::new(events));
        recorder.initialize(events, &mut storage).expect("recorder initialization should succeed");

        let mut callbacks = callbacks.borrow_mut();
        assert_eq!(callbacks.len(), 2);
        callbacks.sort_by_key(|(group, _, _, _)| *group != patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID);

        let (ready_group, ready_tpl, ready_event, ready_callback) = &mut callbacks[0];
        assert_eq!(*ready_group, patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID);
        assert_eq!(*ready_tpl, Tpl::Callback);
        ready_callback(*ready_event);

        let (exit_group, exit_tpl, exit_event, exit_callback) = &mut callbacks[1];
        assert_eq!(*exit_group, patina::uefi::event::EXIT_BOOT_SERVICES_EVENT_GROUP_GUID);
        assert_eq!(*exit_tpl, Tpl::Callback);
        exit_callback(*exit_event);
        drop(callbacks);

        let mut closed_events = closed_events.borrow().clone();
        closed_events.sort_by_key(Event::as_raw);
        assert_eq!(closed_events, [test_event(1), test_event(2)]);

        let output = format!("{recorder}");
        assert!(output.contains("test ... ok (1 passes)"));
    }

    #[test]
    fn test_recorder_formats_failure_without_error_message() {
        let recorder = Recorder::default();
        let mut record = TestRecord::new(false, &TEST_CASE3, None);
        record.fail = 1;
        recorder.update_record(record);

        let output = format!("{recorder}");
        assert!(output.contains("test_that_fails ... fail (1 fails, 0 passes): <no error message>"));
    }

    #[test]
    fn test_recorder_propagates_formatting_errors() {
        struct RejectMissingErrorMessage;

        impl core::fmt::Write for RejectMissingErrorMessage {
            fn write_str(&mut self, value: &str) -> core::fmt::Result {
                if value.contains("<no error message>") { Err(core::fmt::Error) } else { Ok(()) }
            }
        }

        let recorder = Recorder::default();
        let mut record = TestRecord::new(false, &TEST_CASE3, None);
        record.fail = 1;
        recorder.update_record(record);

        assert!(core::fmt::write(&mut RejectMissingErrorMessage, format_args!("{recorder}")).is_err());
    }

    #[test]
    fn test_recorder_reports_human_readable_and_json_results() {
        struct TestLogger(std::sync::Mutex<Vec<String>>);

        impl log::Log for TestLogger {
            fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
                metadata.level() <= log::Level::Info
            }

            fn log(&self, record: &log::Record<'_>) {
                if self.enabled(record.metadata()) {
                    self.0.lock().expect("test log should not be poisoned").push(format!("{}", record.args()));
                }
            }

            fn flush(&self) {}
        }

        static LOGGER: TestLogger = TestLogger(std::sync::Mutex::new(Vec::new()));

        log::set_logger(&LOGGER).expect("test logger should only be initialized once");
        log::set_max_level(log::LevelFilter::Info);

        let mut storage = Storage::new();
        let recorder = Recorder::default();
        let mut record = TestRecord::new(false, &TEST_CASE3, None);
        record.fail = 1;
        record.err_msg = Some("Failure");
        recorder.update_record(record);

        recorder.run_tests_and_report(&mut storage);

        let messages = LOGGER.0.lock().expect("test log should not be poisoned");
        assert!(messages.iter().any(|message| message.contains("test_that_fails ... fail")));
        assert!(
            messages
                .iter()
                .any(|message| message.contains(r#""patina_on_system_unit_test_results":[{"name":"test_that_fails""#))
        );
    }

    #[test]
    fn test_record_json_string() {
        let test_case = &TEST_CASE1;
        let mut record = TestRecord::new(false, test_case, None);
        record.pass = 2;
        record.fail = 1;
        record.err_msg = Some("Failure message");

        let json = format!("{}", record.json());
        assert_eq!(json, r#"{"name":"test","pass":2,"fail":1,"skipped":false,"err_msg":"Failure message"}"#);

        record.err_msg = None;
        let json = format!("{}", record.json());
        assert_eq!(json, r#"{"name":"test","pass":2,"fail":1,"skipped":false,"err_msg":null}"#);
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
        let json = format!("{}", recorder.json());
        assert!(json.contains(r#"{"name":"test","pass":2,"fail":1,"skipped":false,"err_msg":"Failure 1"}"#));
        assert!(json.contains(r#"{"name":"test_that_fails","pass":0,"fail":2,"skipped":false,"err_msg":"Failure 2"}"#));
        assert!(json.starts_with('[') && json.ends_with(']'));
        assert!(json.contains(','));
    }

    #[test]
    fn test_recorder_json_formats_into_fixed_buffer() {
        use core::fmt::Write;

        struct FixedBuffer {
            bytes: [u8; 512],
            len: usize,
        }

        impl Write for FixedBuffer {
            fn write_str(&mut self, value: &str) -> core::fmt::Result {
                let end = self.len + value.len();
                let destination = self.bytes.get_mut(self.len..end).ok_or(core::fmt::Error)?;
                destination.copy_from_slice(value.as_bytes());
                self.len = end;
                Ok(())
            }
        }

        let recorder = Recorder::default();
        let mut buffer = FixedBuffer { bytes: [0; 512], len: 0 };
        write!(buffer, "{}", recorder.json()).unwrap();
        assert_eq!(&buffer.bytes[..buffer.len], b"[]");

        let mut record = TestRecord::new(false, &TEST_CASE2, None);
        record.pass = 2;
        recorder.update_record(record);
        let mut record = TestRecord::new(false, &TEST_CASE3, None);
        record.fail = 1;
        record.err_msg = Some("Failure");
        recorder.update_record(record);

        buffer.len = 0;
        write!(buffer, r#"{{"patina_on_system_unit_test_results":{}}}"#, recorder.json()).unwrap();
        assert_eq!(
            core::str::from_utf8(&buffer.bytes[..buffer.len]).unwrap(),
            r#"{"patina_on_system_unit_test_results":[{"name":"test","pass":2,"fail":0,"skipped":false,"err_msg":null},{"name":"test_that_fails","pass":0,"fail":1,"skipped":false,"err_msg":"Failure"}]}"#
        );
    }
}
