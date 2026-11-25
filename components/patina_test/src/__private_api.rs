//! Implementation details used by the `patina_test` macro.
//!
//! Copyright (c) Microsoft Corporation.
//! SPDX-License-Identifier: Apache-2.0

use crate::{
    alloc::{boxed::Box, rc::Rc},
    component::Filter,
};
use core::{marker::PhantomData, ptr::NonNull, time::Duration};
use patina::{
    BinaryGuid,
    component::{
        Component, MetaData, Storage, UnsafeStorageCell,
        params::{Param, ParamFunction},
        service::{
            Service,
            uefi_services::{
                event::{Event, EventError, EventServices, EventServicesExt, Tpl},
                timer_event::{TimerEventServices, TimerEventServicesExt, TimerType},
            },
        },
    },
};

/// Where all the test cases marked with `#[patina_test]` are collated to.
#[cfg(feature = "test-runner")]
#[linkme::distributed_slice]
pub static TEST_CASES: [TestCase];

/// Returns the test cases to run.
///
/// Tests are only collected when the `test-runner` feature is
/// explicitly enabled. This feature is opt-in and explicit because external
/// consumers of `patina` who do not register at least one test case with
/// the `#[patina_test]` attribute may encounter a surprising linker crash (not
/// just a linker failure), due to the testing infrastructure relying on the
/// `linkme` crate.
///
/// If the `test-runner` feature is not enabled, this function will return an empty slice.
pub fn test_cases() -> &'static [TestCase] {
    #[cfg(feature = "test-runner")]
    {
        &TEST_CASES
    }
    #[cfg(not(feature = "test-runner"))]
    {
        &[]
    }
}

/// An enum to describe how / when a unit test should be executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestTrigger {
    /// Run once when dependencies are available.
    Immediate,
    /// The test case should be executed when the specified event triggers.
    Event(BinaryGuid),
    /// The test case should be executed after the specified units of 100ns have elapsed.
    Timer(u64),
}

/// Internal struct to hold the test case information.
#[derive(Debug, Clone, Copy)]
pub struct TestCase {
    pub name: &'static str,
    pub triggers: &'static [TestTrigger],
    pub skip: bool,
    pub should_fail: bool,
    pub fail_msg: Option<&'static str>,
    pub component: fn(&'static TestCase) -> Box<dyn Component>,
}

impl TestCase {
    pub(crate) fn should_run(&self, filters: &[Filter]) -> bool {
        if self.skip {
            return false;
        }
        let mut includes = false;
        let mut matched = false;

        for filter in filters {
            match filter {
                Filter::Exclude(pattern) if self.name.contains(pattern) => return false,
                Filter::Predicate(predicate) if !predicate(self.name) => return false,
                Filter::Exclude(_) | Filter::Predicate(_) => {}
                Filter::Include(pattern) => {
                    includes = true;
                    matched |= self.name.contains(pattern);
                }
            }
        }

        !includes || matched
    }

    pub(crate) fn evaluate(&self, result: crate::error::Result) -> crate::error::Result {
        match (self.should_fail, result) {
            (true, Ok(())) => Err("Test passed when it should have failed"),
            (true, Err(msg)) if self.fail_msg.is_none() || self.fail_msg == Some(msg) => Ok(()),
            (_, result) => result,
        }
    }
}

/// Creates the typed component used to execute a test, copying the callable into each deferred callback.
pub fn test_component<Marker, Func>(test: &'static TestCase, func: Func) -> Box<dyn Component>
where
    Marker: 'static,
    Func: ParamFunction<Marker, In = (), Out = crate::error::Result> + Copy,
{
    Box::new(TestComponent { test, func, state: None, metadata: MetaData::new::<Func>(), marker: PhantomData })
}

struct TestComponent<Marker, Func: ParamFunction<Marker>> {
    test: &'static TestCase,
    func: Func,
    state: Option<Rc<TestState<Func::Param>>>,
    metadata: MetaData,
    marker: PhantomData<fn() -> Marker>,
}

type InfrastructureServices =
    (Service<crate::service::Recorder>, Service<dyn EventServices>, Service<dyn TimerEventServices>);
type TestState<P> = <(InfrastructureServices, P) as Param>::State;

impl<Marker, Func> TestComponent<Marker, Func>
where
    Marker: 'static,
    Func: ParamFunction<Marker, In = (), Out = crate::error::Result> + Copy,
{
    /// Runs an immediate trigger with registered and validated parameter accesses.
    unsafe fn handle_immediate(
        &mut self,
        state: &TestState<Func::Param>,
        storage: UnsafeStorageCell,
    ) -> patina::error::Result<()> {
        // SAFETY: The caller guarantees registered, validated, non-conflicting access.
        let recorder = unsafe { Service::<crate::service::Recorder>::get_param(&state.0.0, storage) };
        // SAFETY: The caller guarantees registered, validated, non-conflicting access.
        unsafe { Self::execute(self.test, &mut self.func, &state.1, &recorder, storage) }
    }

    /// Registers an event trigger with registered and validated parameter accesses.
    unsafe fn handle_event(
        &self,
        guid: BinaryGuid,
        state: &Rc<TestState<Func::Param>>,
        storage: UnsafeStorageCell,
    ) -> patina::error::Result<()> {
        // SAFETY: The caller guarantees registered, validated, non-conflicting access.
        let (recorder, events, _) = unsafe { InfrastructureServices::get_param(&state.0, storage) };
        // SAFETY: The caller guarantees registered, non-conflicting access.
        let storage = NonNull::from(unsafe { storage.storage() });
        events
            .on_event_group(guid, Tpl::Callback, self.create_test_callback(state, recorder, storage))
            .map(|_| ())
            .map_err(|error| {
                log::error!("Failed to register event trigger for patina_test {}: {error:?}", self.test.name);
                error.into()
            })
    }

    /// Registers a periodic timer and a Ready to Boot cleanup event.
    unsafe fn handle_timer(
        &self,
        interval: u64,
        state: &Rc<TestState<Func::Param>>,
        storage: UnsafeStorageCell,
    ) -> patina::error::Result<()> {
        // SAFETY: The caller guarantees registered, validated, non-conflicting access.
        let (recorder, events, timer) = unsafe { InfrastructureServices::get_param(&state.0, storage) };
        // SAFETY: The caller guarantees registered, non-conflicting access.
        let storage = NonNull::from(unsafe { storage.storage() });
        let name = self.test.name;
        let timer_event = timer
            .on_timer_event(Tpl::Callback, self.create_test_callback(state, recorder, storage))
            .map_err(|error| {
                log::error!("Failed to create timer trigger for patina_test {name}: {error:?}");
                patina::error::EfiError::from(error)
            })?;
        let cleanup_event = match self.register_timer_cleanup(&events, &timer, timer_event) {
            Ok(event) => event,
            Err(error) => {
                log::error!("Failed to register timer cleanup for patina_test {name}: {error:?}");
                Self::close_timer_events(name, &*events, &[timer_event]);
                return Err(error.into());
            }
        };
        let duration = Duration::from_nanos(interval.saturating_mul(100));
        if let Err(error) = timer.set_timer(timer_event, TimerType::Periodic(duration)) {
            log::error!("Failed to arm timer trigger for patina_test {name}: {error:?}");
            Self::close_timer_events(name, &*events, &[timer_event, cleanup_event]);
            return Err(error.into());
        }
        Ok(())
    }

    /// Registers a Ready to Boot callback that cancels and closes the timer and its cleanup event.
    fn register_timer_cleanup(
        &self,
        events: &Service<dyn EventServices>,
        timer: &Service<dyn TimerEventServices>,
        timer_event: Event,
    ) -> Result<Event, EventError> {
        let name = self.test.name;
        let cleanup_timer = timer.clone();
        let cleanup_events = events.clone();
        events.create_event_for_group(
            patina::uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID,
            Tpl::Callback,
            Box::new(move |event| {
                if let Err(error) = cleanup_timer.set_timer(timer_event, TimerType::Cancel) {
                    log::error!("Failed to cancel timer trigger for patina_test {name}: {error:?}");
                }
                Self::close_timer_events(name, &*cleanup_events, &[timer_event, event]);
            }),
        )
    }

    /// Closes each event, logging failures without preventing the remaining events from being closed.
    fn close_timer_events(name: &str, events: &dyn EventServices, handles: &[Event]) {
        for &event in handles {
            if let Err(error) = events.close_event(event) {
                log::error!("Failed to close timer event {event:?} for patina_test {name}: {error:?}");
            }
        }
    }

    /// Creates a callback that revalidates the test parameters and executes the test when available.
    ///
    /// Deferred triggers remain disabled until storage lifetime and access requirements are enforced.
    fn create_test_callback(
        &self,
        state: &Rc<TestState<Func::Param>>,
        recorder: Service<crate::service::Recorder>,
        storage: NonNull<Storage>,
    ) -> impl FnMut() + 'static {
        let test = self.test;
        let mut func = self.func;
        let state = state.clone();
        move || {
            // SAFETY: Deferred tests remain filtered out by TestRegistration::build_components.
            // Before enabling them, storage must remain at a stable address and outlive the
            // callback, with no conflicting accesses during callback execution.
            let storage = UnsafeStorageCell::from(unsafe { storage.as_ref() });
            if !Func::Param::validate(&state.1, storage) {
                return;
            }
            // SAFETY: Parameters are registered and validated. This path remains disabled
            // until the storage lifetime and non-conflicting access requirements above hold.
            if let Err(error) = unsafe { Self::execute(test, &mut func, &state.1, &recorder, storage) } {
                log::error!("Failed to execute patina_test {}: {error:?}", test.name);
            }
        }
    }

    /// Executes with registered, validated, non-conflicting parameter access.
    unsafe fn execute(
        test: &TestCase,
        func: &mut Func,
        state: &<Func::Param as Param>::State,
        recorder: &crate::service::Recorder,
        storage: UnsafeStorageCell,
    ) -> patina::error::Result<()> {
        let Some(record) = recorder.get_unskipped(test.name)? else {
            return Ok(());
        };
        // SAFETY: The caller guarantees parameter registration, validation, and non-conflicting access.
        let params = unsafe { Func::Param::get_param(state, storage) };
        let result = if record.debug_mode() {
            log::debug!("#### {} Test Output Start ####", test.name);
            let result = func.run(&mut Some(()), params);
            log::debug!("####  {} Test Output End  ####", test.name);
            result
        } else {
            let level = log::max_level();
            log::set_max_level(log::LevelFilter::Off);
            let result = func.run(&mut Some(()), params);
            log::set_max_level(level);
            result
        };
        let result = test.evaluate(result);
        recorder.record_result(test.name, result)?;
        if let (Err(message), Some(callback)) = (result, record.callback()) {
            callback(test.name, message);
        }
        Ok(())
    }
}

impl<Marker, Func> Component for TestComponent<Marker, Func>
where
    Marker: 'static,
    Func: ParamFunction<Marker, In = (), Out = crate::error::Result> + Copy,
{
    fn initialize(&mut self, storage: &mut Storage) -> bool {
        match <(InfrastructureServices, Func::Param) as Param>::init_state(storage, &mut self.metadata) {
            Ok(state) => {
                self.state = Some(Rc::new(state));
                true
            }
            Err(error) => {
                self.metadata.set_error_message(error);
                false
            }
        }
    }

    fn metadata(&self) -> &MetaData {
        &self.metadata
    }

    unsafe fn run_unsafe(&mut self, storage: UnsafeStorageCell) -> patina::error::Result<bool> {
        let state = self.state.as_ref().expect("test component initialized").clone();
        if let Err(error) = <(InfrastructureServices, Func::Param)>::try_validate(&state, storage) {
            self.metadata.set_error_message(error);
            return Ok(false);
        }
        for trigger in self.test.triggers {
            match trigger {
                // SAFETY: All parameter accesses were registered and validated above.
                TestTrigger::Immediate => unsafe { self.handle_immediate(&state, storage) },
                // SAFETY: All parameter accesses were registered and validated above.
                TestTrigger::Event(guid) => unsafe { self.handle_event(*guid, &state, storage) },
                // SAFETY: All parameter accesses were registered and validated above.
                TestTrigger::Timer(interval) => unsafe { self.handle_timer(*interval, &state, storage) },
            }?;
        }
        Ok(true)
    }
}
