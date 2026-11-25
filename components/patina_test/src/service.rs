//! Patina Testing Service
//!
//! This module defines the internal [Recorder] service used to record and report results for tests marked with the
//! `#[patina_test]` attribute. [`TestRegistration`](crate::component::TestRegistration) creates a recorder containing
//! the discovered tests and their selection status, along with components for the selected tests and a reporter.
//! The recorder component initializes and publishes the service; each test component executes its test and records
//! the result. The reporter requests results from the recorder at Ready to Boot and Exit Boot Services.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{__private_api::TestCase, alloc::collections::BTreeMap};
use core::fmt::{self, Display};
use patina::{
    component::{
        Storage, component,
        service::{IntoService, Service, uefi_services::tpl::TplServices},
    },
    standard::efi,
    uefi::{boot_services::tpl::Tpl, tpl_mutex::TplMutex},
    writelncrlf,
};

pub(crate) type Callback = fn(&'static str, &'static str);

/// A structure containing all necessary data to execute a test at any time.
#[derive(Clone)]
pub(crate) struct TestRecord {
    /// Whether or not to log debug messages in the test or not
    debug_mode: bool,
    /// The test case to execute.
    test_case: &'static TestCase,
    /// Callback function to be called on test failure.
    callback: Option<Callback>,
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

impl TestRecord {
    /// Creates a new instance of `TestRecord`.
    pub fn new(debug_mode: bool, test_case: &'static TestCase, callback: Option<Callback>) -> Self {
        Self { debug_mode, test_case, callback, skipped: false, pass: 0, fail: 0, err_msg: None }
    }

    /// Creates a record for a skipped test, i.e. one that is marked `#[skip]` or filtered out.
    pub fn skipped(test_case: &'static TestCase) -> Self {
        Self { skipped: true, ..Self::new(false, test_case, None) }
    }

    /// Returns whether debug logging is enabled for this test.
    pub fn debug_mode(&self) -> bool {
        self.debug_mode
    }

    /// Returns the callback invoked on test failure, if any.
    pub fn callback(&self) -> Option<Callback> {
        self.callback
    }

    /// Returns true if the test was scheduled to run, but has not run yet, i.e. it has neither passed nor failed.
    fn is_not_triggered(&self) -> bool {
        !self.skipped && self.pass == 0 && self.fail == 0
    }

    /// Formats the test record as JSON for logging or reporting purposes. This must not allocate because it may be
    /// called during `ExitBootServices`.
    fn json(&self) -> impl Display + '_ {
        core::fmt::from_fn(move |formatter| {
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
#[derive(IntoService)]
#[service(Recorder)]
pub(crate) struct Recorder {
    records: TplMutex<BTreeMap<&'static str, TestRecord>, Service<dyn TplServices>>,
}

impl Recorder {
    /// Creates a recorder whose mutex is initialized when the component is dispatched.
    pub fn new(records: BTreeMap<&'static str, TestRecord>) -> Self {
        Self { records: TplMutex::new_uninit(Tpl(efi::TPL_NOTIFY), records) }
    }

    /// Returns a copy of the named record if it exists and is not skipped, even if it has already run.
    ///
    /// # Errors
    ///
    /// Returns `NotReady` if the recorder cannot be locked.
    pub fn get_unskipped(&self, name: &str) -> patina::error::Result<Option<TestRecord>> {
        self.records.with_checked_lock(|records| records.get(name).filter(|record| !record.skipped).cloned()).map_err(
            |error| {
                log::error!("Failed to lock test recorder: {error}");
                patina::error::EfiError::NotReady
            },
        )
    }

    /// Records a test execution result.
    ///
    /// Failures update the most recent error message; successful runs do not clear it.
    ///
    /// # Errors
    ///
    /// Returns `NotReady` if the recorder cannot be locked, or `NotFound` if the test has no record.
    pub fn record_result(&self, name: &'static str, result: crate::error::Result) -> patina::error::Result<()> {
        self.records
            .with_checked_lock(|records| {
                let record = records.get_mut(name).ok_or(patina::error::EfiError::NotFound)?;
                match result {
                    Ok(()) => record.pass += 1,
                    Err(message) => {
                        record.fail += 1;
                        record.err_msg = Some(message);
                    }
                }
                Ok(())
            })
            .map_err(|error| {
                log::error!("Failed to lock test recorder: {error}");
                patina::error::EfiError::NotReady
            })?
    }

    /// Formats all test records as JSON. This must not allocate because it can be called during `ExitBootServices`.
    fn json(&self) -> impl Display + '_ {
        core::fmt::from_fn(move |formatter| {
            self.records
                .with_checked_lock(|records| {
                    formatter.write_str("[")?;
                    for (index, record) in records.values().enumerate() {
                        if index != 0 {
                            formatter.write_str(",")?;
                        }
                        write!(formatter, "{}", record.json())?;
                    }
                    formatter.write_str("]")
                })
                .map_err(|error| {
                    log::error!("Failed to lock test recorder for JSON formatting: {error}");
                    fmt::Error
                })?
        })
    }

    /// Logs the current results in text and JSON formats without allocating.
    ///
    /// # Errors
    ///
    /// Returns `NotReady` if the recorder cannot be locked.
    pub fn report(&self) -> patina::error::Result<()> {
        self.records.with_checked_lock(|_| ()).map_err(|error| {
            log::error!("Failed to lock test recorder for reporting: {error}");
            patina::error::EfiError::NotReady
        })?;
        log::info!("{self}");
        log::info!(r#"{{"patina_on_system_unit_test_results":{}}}"#, self.json());
        Ok(())
    }
}

#[component]
impl Recorder {
    /// Initializes the mutex and publishes the recorder once TPL services are available.
    ///
    /// # Errors
    ///
    /// Returns `InvalidParameter` if a recorder is already registered.
    fn entry_point(self, tpl: Service<dyn TplServices>, storage: &mut Storage) -> patina::error::Result<()> {
        if storage.get_service::<Recorder>().is_some() {
            log::error!("Only one TestRegistration may be added to the platform.");
            return Err(patina::error::EfiError::InvalidParameter);
        }
        self.records.init(tpl);
        storage.add_service(self);
        Ok(())
    }
}

impl Display for Recorder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.records
            .with_checked_lock(|records| {
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
                writelncrlf!(f, "Patina on-system unit-test result totals: {total_passes} passes, {total_fails} fails")
            })
            .map_err(|error| {
                log::error!("Failed to lock test recorder for report formatting: {error}");
                fmt::Error
            })?
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    extern crate std;

    use super::*;
    use crate::{
        __private_api::{TestCase, TestTrigger, test_component},
        alloc::{boxed::Box, format},
    };
    use patina::component::service::uefi_services::{
        event::{EventServices, MockEventServices},
        timer_event::{MockTimerEventServices, TimerEventServices},
        tpl::{PreviousTpl, Tpl as ServiceTpl},
    };

    fn dispatch_triggered_test(
        test: &'static TestCase,
        mut events: MockEventServices,
        mut timer: MockTimerEventServices,
    ) {
        let timers = test.triggers.iter().filter(|trigger| matches!(trigger, TestTrigger::Timer(_))).count();
        events
            .expect_create_event_for_group()
            .times(test.triggers.iter().filter(|trigger| matches!(trigger, TestTrigger::Event(_))).count() + timers)
            .returning(|_, _, _| {
                Ok(patina::component::service::uefi_services::event::Event::from_raw(
                    core::ptr::NonNull::<u8>::dangling().as_ptr().cast(),
                )
                .unwrap())
            });
        timer.expect_create_timer_event().times(timers).returning(|_, _| {
            Ok(patina::component::service::uefi_services::event::Event::from_raw(
                core::ptr::NonNull::<u8>::dangling().as_ptr().cast(),
            )
            .unwrap())
        });
        timer.expect_set_timer().times(timers).returning(|_, _| Ok(()));
        use crate::alloc::sync::Arc;
        struct SchedulerMocks {
            events: Arc<MockEventServices>,
            timer: Arc<MockTimerEventServices>,
        }
        impl IntoService for SchedulerMocks {
            fn register_shared(self: Arc<Self>, storage: &mut Storage) {
                Self::register_service::<dyn EventServices>(storage, self.events.clone());
                Self::register_service::<dyn TimerEventServices>(storage, self.timer.clone());
            }
        }
        let mut storage = Storage::new();
        let mut records = BTreeMap::new();
        records.insert(test.name, TestRecord::new(false, test, None));
        Recorder::new(records).entry_point(Service::mock(Box::new(TestTpl)), &mut storage).unwrap();
        storage.add_service(SchedulerMocks { events: Arc::new(events), timer: Arc::new(timer) });
        let mut component = (test.component)(test);
        assert!(component.initialize(&mut storage));
        assert!(component.run(&mut storage).unwrap());
        let recorder = storage.get_service::<Recorder>().unwrap();
        let records = recorder.records.checked_lock().unwrap();
        let expected_passes = test.triggers.iter().filter(|trigger| **trigger == TestTrigger::Immediate).count();
        assert_eq!(records.get(test.name).unwrap().pass as usize, expected_passes);
    }

    struct TestTpl;
    impl TplServices for TestTpl {
        fn raise_tpl(&self, _: ServiceTpl) -> PreviousTpl {
            PreviousTpl::from_raw(efi::TPL_APPLICATION)
        }
        fn restore_tpl(&self, _: PreviousTpl) {}
    }

    fn test_body() -> crate::error::Result {
        Ok(())
    }

    static TEST: TestCase = TestCase {
        name: "test",
        triggers: &[TestTrigger::Immediate],
        skip: false,
        should_fail: false,
        fail_msg: None,
        component: |test| test_component(test, test_body),
    };

    static TRIGGERED_TEST: TestCase = TestCase {
        triggers: &[
            TestTrigger::Immediate,
            TestTrigger::Event(patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID),
            TestTrigger::Timer(500),
        ],
        ..TEST
    };

    #[test]
    fn test_recorder_component_dispatch_registers_event_and_retains_execution_state() {
        use crate::alloc::sync::Arc;
        use patina::component::service::uefi_services::{
            event::{Event, EventNotifyCallback, MockEventServices, Tpl as EventTpl},
            timer_event::MockTimerEventServices,
        };
        use std::{cell::RefCell, rc::Rc};

        struct SchedulerMocks {
            events: Arc<MockEventServices>,
            timer: Arc<MockTimerEventServices>,
        }
        impl IntoService for SchedulerMocks {
            fn register_shared(self: Arc<Self>, storage: &mut Storage) {
                Self::register_service::<dyn EventServices>(storage, self.events.clone());
                Self::register_service::<dyn TimerEventServices>(storage, self.timer.clone());
            }
        }

        static EVENT_TEST: TestCase = TestCase {
            triggers: &[
                TestTrigger::Event(patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID),
                TestTrigger::Immediate,
                TestTrigger::Immediate,
            ],
            ..TEST
        };
        let callback = Rc::new(RefCell::new(None::<EventNotifyCallback>));
        let captured = callback.clone();
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().times(1).returning_st(move |guid, tpl, notify| {
            assert_eq!(guid, patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID);
            assert_eq!(tpl, EventTpl::Callback);
            *captured.borrow_mut() = Some(notify);
            Ok(Event::from_raw(core::ptr::NonNull::<u8>::dangling().as_ptr().cast()).unwrap())
        });
        let mut storage = Box::new(Storage::new());
        let mut records = BTreeMap::new();
        records.insert(EVENT_TEST.name, TestRecord::new(false, &EVENT_TEST, None));
        Recorder::new(records).entry_point(Service::mock(Box::new(TestTpl)), &mut storage).unwrap();
        let mut component = (EVENT_TEST.component)(&EVENT_TEST);
        assert!(component.initialize(&mut storage));
        assert!(!component.run(&mut storage).unwrap());
        storage
            .add_service(SchedulerMocks { events: Arc::new(events), timer: Arc::new(MockTimerEventServices::new()) });
        assert!(component.run(&mut storage).unwrap());
        drop(component);
        let recorder = storage.get_service::<Recorder>().unwrap();
        assert_eq!(recorder.records.checked_lock().unwrap().get(EVENT_TEST.name).unwrap().pass, 2);
        let mut callback = callback.borrow_mut().take().unwrap();
        let event = Event::from_raw(core::ptr::NonNull::<u8>::dangling().as_ptr().cast()).unwrap();
        // Storage is boxed and alive, no storage borrows or test parameters are active, and the
        // component has been dropped. These controlled calls satisfy the callback's pending contract.
        callback(event);
        callback(event);
        assert_eq!(recorder.records.checked_lock().unwrap().get(EVENT_TEST.name).unwrap().pass, 4);
        drop(callback);
    }

    #[test]
    fn test_recorder_component_dispatch_timer_execution_and_cleanup() {
        use crate::alloc::sync::Arc;
        use core::time::Duration;
        use patina::component::{
            params::ConfigMut,
            service::uefi_services::{
                event::{Event, EventNotifyCallback, Tpl},
                timer_event::TimerType,
            },
        };
        use std::{cell::RefCell, rc::Rc};

        struct SchedulerMocks {
            events: Arc<MockEventServices>,
            timer: Arc<MockTimerEventServices>,
        }
        impl IntoService for SchedulerMocks {
            fn register_shared(self: Arc<Self>, storage: &mut Storage) {
                Self::register_service::<dyn EventServices>(storage, self.events.clone());
                Self::register_service::<dyn TimerEventServices>(storage, self.timer.clone());
            }
        }
        fn timer_test(mut config: ConfigMut<u8>) -> crate::error::Result {
            *config += 1;
            Ok(())
        }
        static TIMER_TEST: TestCase = TestCase {
            triggers: &[TestTrigger::Timer(u64::MAX)],
            component: |test| test_component(test, timer_test),
            ..TEST
        };
        let timer_event = Event::from_raw(core::ptr::NonNull::<u8>::dangling().as_ptr().cast()).unwrap();
        let cleanup_event = Event::from_raw(core::ptr::NonNull::<u16>::dangling().as_ptr().cast()).unwrap();
        let timer_callback = Rc::new(RefCell::new(None::<EventNotifyCallback>));
        let cleanup_callback = Rc::new(RefCell::new(None::<EventNotifyCallback>));
        let timer_operations = Rc::new(RefCell::new(alloc::vec::Vec::new()));
        let closed_events = Rc::new(RefCell::new(alloc::vec::Vec::new()));
        let captured = timer_callback.clone();
        let mut timer = MockTimerEventServices::new();
        timer.expect_create_timer_event().times(1).returning_st(move |tpl, callback| {
            assert_eq!(tpl, Tpl::Callback);
            *captured.borrow_mut() = Some(callback);
            Ok(timer_event)
        });
        let captured = timer_operations.clone();
        timer.expect_set_timer().times(2).returning_st(move |event, timer_type| {
            assert_eq!(event, timer_event);
            captured.borrow_mut().push(timer_type);
            Ok(())
        });
        let captured = cleanup_callback.clone();
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().times(1).returning_st(move |guid, tpl, callback| {
            assert_eq!(guid, patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID);
            assert_eq!(tpl, Tpl::Callback);
            *captured.borrow_mut() = Some(callback);
            Ok(cleanup_event)
        });
        let captured = closed_events.clone();
        events.expect_close_event().times(2).returning_st(move |event| {
            captured.borrow_mut().push(event);
            Ok(())
        });
        let mut storage = Box::new(Storage::new());
        let mut records = BTreeMap::new();
        records.insert(TIMER_TEST.name, TestRecord::new(false, &TIMER_TEST, None));
        Recorder::new(records).entry_point(Service::mock(Box::new(TestTpl)), &mut storage).unwrap();
        storage.add_service(SchedulerMocks { events: Arc::new(events), timer: Arc::new(timer) });
        let mut component = (TIMER_TEST.component)(&TIMER_TEST);
        assert!(component.initialize(&mut storage));
        assert!(component.run(&mut storage).unwrap());
        drop(component);
        let mut callback = timer_callback.borrow_mut().take().unwrap();
        // Storage is boxed and alive with no outstanding storage or parameter borrows.
        callback(timer_event);
        callback(timer_event);
        assert_eq!(*storage.get_config_mut::<u8>().unwrap(), 2);
        storage.lock_configs();
        callback(timer_event);
        let recorder = storage.get_service::<Recorder>().unwrap();
        assert_eq!(recorder.records.checked_lock().unwrap().get(TIMER_TEST.name).unwrap().pass, 2);
        let mut cleanup = cleanup_callback.borrow_mut().take().unwrap();
        cleanup(cleanup_event);
        assert_eq!(
            *timer_operations.borrow(),
            alloc::vec![TimerType::Periodic(Duration::from_nanos(u64::MAX)), TimerType::Cancel]
        );
        assert_eq!(*closed_events.borrow(), alloc::vec![timer_event, cleanup_event]);
        drop(callback);
        drop(cleanup);
    }

    #[test]
    fn test_recorder_component_dispatch_timer_registration_failures() {
        use crate::alloc::sync::Arc;
        use patina::component::service::uefi_services::event::{Event, EventError};
        use std::{cell::RefCell, rc::Rc};

        struct SchedulerMocks {
            events: Arc<MockEventServices>,
            timer: Arc<MockTimerEventServices>,
        }
        impl IntoService for SchedulerMocks {
            fn register_shared(self: Arc<Self>, storage: &mut Storage) {
                Self::register_service::<dyn EventServices>(storage, self.events.clone());
                Self::register_service::<dyn TimerEventServices>(storage, self.timer.clone());
            }
        }
        static TIMER_TEST: TestCase = TestCase { triggers: &[TestTrigger::Timer(500)], ..TEST };
        let timer_event = Event::from_raw(core::ptr::NonNull::<u8>::dangling().as_ptr().cast()).unwrap();
        let cleanup_event = Event::from_raw(core::ptr::NonNull::<u16>::dangling().as_ptr().cast()).unwrap();
        for stage in 0..3 {
            let closed = Rc::new(RefCell::new(alloc::vec::Vec::new()));
            let captured = closed.clone();
            let mut events = MockEventServices::new();
            events.expect_create_event_for_group().times(usize::from(stage != 0)).returning_st(move |_, _, _| {
                if stage == 1 { Err(EventError::InvalidParameter) } else { Ok(cleanup_event) }
            });
            events.expect_close_event().times(stage).returning_st(move |event| {
                captured.borrow_mut().push(event);
                Ok(())
            });
            let mut timer = MockTimerEventServices::new();
            timer
                .expect_create_timer_event()
                .times(1)
                .returning_st(move |_, _| if stage == 0 { Err(EventError::InvalidParameter) } else { Ok(timer_event) });
            timer.expect_set_timer().times(usize::from(stage == 2)).returning(|_, _| Err(EventError::InvalidParameter));
            let mut storage = Storage::new();
            let mut records = BTreeMap::new();
            records.insert(TIMER_TEST.name, TestRecord::new(false, &TIMER_TEST, None));
            Recorder::new(records).entry_point(Service::mock(Box::new(TestTpl)), &mut storage).unwrap();
            storage.add_service(SchedulerMocks { events: Arc::new(events), timer: Arc::new(timer) });
            let mut component = (TIMER_TEST.component)(&TIMER_TEST);
            assert!(component.initialize(&mut storage));
            assert_eq!(component.run(&mut storage), Err(patina::error::EfiError::InvalidParameter));
            let expected = match stage {
                0 => alloc::vec![],
                1 => alloc::vec![timer_event],
                _ => alloc::vec![timer_event, cleanup_event],
            };
            assert_eq!(*closed.borrow(), expected);
        }
    }

    #[test]
    fn test_recorder_component_dispatch_handles_mixed_triggers() {
        dispatch_triggered_test(&TRIGGERED_TEST, MockEventServices::new(), MockTimerEventServices::new());
    }

    #[test]
    fn test_recorder_component_dispatch_does_not_execute_deferred_triggers() {
        static DEFERRED_TEST: TestCase = TestCase {
            triggers: &[
                TestTrigger::Event(patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID),
                TestTrigger::Timer(500),
            ],
            ..TEST
        };
        dispatch_triggered_test(&DEFERRED_TEST, MockEventServices::new(), MockTimerEventServices::new());
    }

    #[test]
    fn test_recorder_result_and_report_formats() {
        let record = TestRecord::new(false, &TEST, None);
        let mut records = BTreeMap::new();
        records.insert(TEST.name, record);
        let tpl: Service<dyn TplServices> = Service::mock(Box::new(TestTpl));
        let recorder = Recorder::new(records);
        assert!(matches!(
            recorder.records.checked_lock(),
            Err(patina::uefi::tpl_mutex::TplMutexError::UninitializedController)
        ));
        let mut storage = Storage::new();
        recorder.entry_point(tpl, &mut storage).unwrap();
        let recorder = storage.get_service::<Recorder>().unwrap();
        assert!(format!("{}", *recorder).contains("test ... not triggered"));
        assert_eq!(
            format!("{}", recorder.json()),
            r#"[{"name":"test","pass":0,"fail":0,"skipped":false,"err_msg":null}]"#
        );
        assert!(recorder.get_unskipped("test").unwrap().is_some());
        recorder.record_result("test", Err("failure")).unwrap();
        assert!(format!("{}", *recorder).contains("test ... fail (1 fails, 0 passes): failure"));
        assert_eq!(
            format!("{}", recorder.json()),
            r#"[{"name":"test","pass":0,"fail":1,"skipped":false,"err_msg":"failure"}]"#
        );
    }

    #[test]
    fn test_recorder_report_lock_failure() {
        use core::fmt::Write;

        let recorder = Recorder::new(BTreeMap::new());
        let mut output = std::string::String::new();
        assert!(write!(&mut output, "{recorder}").is_err());
        assert!(write!(&mut output, "{}", recorder.json()).is_err());
        assert!(matches!(recorder.report(), Err(patina::error::EfiError::NotReady)));

        recorder.records.init(Service::mock(Box::new(TestTpl)));
        assert_eq!(format!("{}", recorder.json()), "[]");
        let _guard = recorder.records.checked_lock().unwrap();
        assert!(write!(&mut output, "{recorder}").is_err());
        assert!(write!(&mut output, "{}", recorder.json()).is_err());
        assert!(matches!(recorder.report(), Err(patina::error::EfiError::NotReady)));
    }

    #[test]
    fn test_recorder_accumulates_repeated_results() {
        let mut records = BTreeMap::new();
        records.insert(TEST.name, TestRecord::new(false, &TEST, None));
        let mut storage = Storage::new();
        Recorder::new(records).entry_point(Service::mock(Box::new(TestTpl)), &mut storage).unwrap();
        let recorder = storage.get_service::<Recorder>().unwrap();
        for result in [Ok(()), Err("first failure"), Err("latest failure"), Ok(())] {
            recorder.record_result(TEST.name, result).unwrap();
        }
        assert!(recorder.get_unskipped(TEST.name).unwrap().is_some());
        let records = recorder.records.checked_lock().unwrap();
        let record = records.get(TEST.name).unwrap();
        assert!(!record.is_not_triggered());
        assert_eq!((record.pass, record.fail, record.err_msg), (2, 2, Some("latest failure")));
        drop(records);
        let report = format!("{}", *recorder);
        assert!(report.contains("test ... fail (2 fails, 2 passes): latest failure"));
        assert!(report.contains("totals: 2 passes, 2 fails"));
        assert_eq!(
            format!("{}", recorder.json()),
            r#"[{"name":"test","pass":2,"fail":2,"skipped":false,"err_msg":"latest failure"}]"#
        );
    }

    #[test]
    fn test_recorder_reports_skipped_tests_without_scheduling() {
        let mut records = BTreeMap::new();
        records.insert(TEST.name, TestRecord::skipped(&TEST));
        let mut storage = Storage::new();
        Recorder::new(records).entry_point(Service::mock(Box::new(TestTpl)), &mut storage).unwrap();
        let recorder = storage.get_service::<Recorder>().unwrap();
        assert!(recorder.get_unskipped(TEST.name).unwrap().is_none());
        let records = recorder.records.checked_lock().unwrap();
        assert!(!records.get(TEST.name).unwrap().is_not_triggered());
        drop(records);
        assert!(format!("{}", *recorder).contains("test ... skipped"));
        assert_eq!(
            format!("{}", recorder.json()),
            r#"[{"name":"test","pass":0,"fail":0,"skipped":true,"err_msg":null}]"#
        );
    }
}
