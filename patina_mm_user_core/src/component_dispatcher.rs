//! MM User Core subsystem for the Patina component dispatcher.
//!
//! Brings the Patina component model (dependency-injected `#[component]` entry points)
//! into the MM User Core, like the DXE Core's `component_dispatcher` module. A platform
//! registers components, configurations, and services on the MM dispatch event, after AP
//! rendezvous.
//!
//! The component [`Storage`], [`Component`](patina::component::Component), and parameter
//! types come from the Patina SDK ([`patina::component`]); only the dispatcher and the
//! platform-facing [`MmComponentInfo`] trait live here.
//!
//! This is separate from MM driver dispatch in
//! [`MmDispatcher`](crate::mm_dispatcher::MmDispatcher), and the two coexist: components
//! are Rust objects registered by the platform binary, while MM drivers are separate
//! images discovered from HOBs.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;

use alloc::{borrow::Cow, boxed::Box, vec::Vec};

use patina::{
    component::{IntoComponent, Storage, service::IntoService},
    pi::hob::Hob,
};

/// Implemented by the platform to register components, configurations, and services with
/// the MM User Core.
///
/// The MM counterpart of the DXE Core's `ComponentInfo` trait. The platform MM binary
/// passes its implementing type to
/// [`MmUserCore::entry_point_worker`](crate::MmUserCore::entry_point_worker), which applies
/// it at the MM dispatch event. The heap is available in these callbacks.
///
/// ## Examples
///
/// ```rust,ignore
/// use patina_mm_user_core::component_dispatcher::{Add, Component, MmComponentInfo};
///
/// struct MyMmPlatform;
///
/// impl MmComponentInfo for MyMmPlatform {
///     fn components(mut add: Add<Component>) {
///         add.component(my_mm_component::MyComponent::default());
///     }
/// }
/// ```
pub trait MmComponentInfo: Sized {
    /// A platform callback to register components with the MM User Core.
    fn components(_add: Add<'_, Component>) {}

    /// A platform callback to register configurations with the MM User Core.
    fn configs(_add: Add<'_, Config>) {}

    /// A platform callback to register services with the MM User Core.
    fn services(_add: Add<'_, Service>) {}
}

/// Marker that limits [`Add`] to adding [`Component`](patina::component::Component)s.
pub struct Component;
/// Marker that limits [`Add`] to adding configurations.
pub struct Config;
/// Marker that limits [`Add`] to adding [`Service`](patina::component::service::Service)s.
pub struct Service;

/// Controlled access to the MM User Core's component storage.
///
/// The type parameter `L` selects which `add` method is available, matching the
/// [`MmComponentInfo`] callback that received it.
pub struct Add<'a, L> {
    /// The component dispatcher to add to.
    dispatcher: &'a mut MmComponentDispatcher,
    /// Selects which methods are available.
    _limiter: core::marker::PhantomData<L>,
}

impl<L> Add<'_, L> {
    /// Creates a new [`Add`] struct.
    pub(crate) fn new(dispatcher: &mut MmComponentDispatcher) -> Add<'_, L> {
        Add { dispatcher, _limiter: core::marker::PhantomData }
    }
}

impl Add<'_, Component> {
    /// Adds a component to the MM User Core's component list.
    pub fn component<I>(&mut self, component: impl IntoComponent<I>) {
        let component = component.into_component();
        let index = self.dispatcher.components.len();
        self.dispatcher.insert_component(index, component);
    }
}

impl Add<'_, Config> {
    /// Adds a configuration value to the MM User Core's storage.
    pub fn config<C: Default + 'static>(&mut self, config: C) {
        self.dispatcher.storage.add_config::<C>(config);
    }
}

impl Add<'_, Service> {
    /// Adds a service to the MM User Core's storage.
    pub fn service(&mut self, service: impl IntoService + 'static) {
        self.dispatcher.storage.add_service(service);
    }
}

/// The MM User Core component dispatcher.
///
/// Owns the registered components and the [`Storage`] used for dependency injection, and
/// dispatches components until no more can run.
pub struct MmComponentDispatcher {
    /// Components that initialized and are waiting to be dispatched.
    components: Vec<Box<dyn patina::component::Component>>,
    /// Components that failed to initialize and are never dispatched.
    rejected: Vec<Box<dyn patina::component::Component>>,
    /// Storage the components draw their parameters from.
    storage: Storage,
}

impl Default for MmComponentDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: The dispatcher owns everything stored in it and does not share it. Only the BSP
// uses it, during deferred component startup, behind the containing `spin::Mutex`.
unsafe impl Send for MmComponentDispatcher {}

impl MmComponentDispatcher {
    /// Creates a new, empty `MmComponentDispatcher`.
    pub const fn new() -> Self {
        Self { components: Vec::new(), rejected: Vec::new(), storage: Storage::new() }
    }

    /// Applies the configs, services, and components registered by `C`.
    pub fn apply_component_info<C: MmComponentInfo>(&mut self) {
        C::configs(Add::new(self));
        C::services(Add::new(self));
        C::components(Add::new(self));
    }

    /// Initializes a component against storage and inserts it at `idx`.
    ///
    /// A component that fails to initialize goes to the rejected list and is never
    /// dispatched.
    pub fn insert_component(&mut self, index: usize, mut component: Box<dyn patina::component::Component>) {
        if component.initialize(&mut self.storage) {
            self.components.insert(index, component);
        } else {
            self.rejected.push(component);
        }
    }

    /// Adds a service to storage.
    pub fn add_service<S: IntoService + 'static>(&mut self, service: S) {
        self.storage.add_service(service);
    }

    /// Adds a configuration value to storage.
    pub fn add_config<C: Default + 'static>(&mut self, config: C) {
        self.storage.add_config::<C>(config);
    }

    /// Locks the configurations in storage.
    ///
    /// Components that take `Config<T>` can then run, and `ConfigMut<T>` components can no
    /// longer run.
    pub fn lock_configs(&mut self) {
        self.storage.lock_configs();
    }

    /// Runs the registered parsers on each guided HOB, producing the `Hob<T>` values
    /// components can request.
    pub fn insert_hobs(&mut self, hob: &Hob<'_>) {
        for entry in hob {
            if let Hob::GuidHob(guid, data) = entry {
                let parser_funcs = self.storage.get_hob_parsers(&guid.name);
                if parser_funcs.is_empty() {
                    continue;
                }
                for parser_func in parser_funcs {
                    parser_func(data, &mut self.storage);
                }
            }
        }
    }

    /// Makes one dispatch pass over the pending components.
    ///
    /// Returns `true` if any component ran (successfully or not), meaning another pass may
    /// dispatch more.
    pub fn dispatch(&mut self) -> bool {
        let len = self.components.len();
        self.components.retain_mut(|component| {
            let name = component.metadata().name();
            log::trace!("MM Dispatch Start: Id = [{name:?}]");
            // Ok(true): ran successfully, so drop it.
            // Ok(false): not ready yet, so keep it.
            // Err(e): ran and failed, so drop it.
            !match component.run(&mut self.storage) {
                Ok(true) => true,
                Ok(false) => false,
                Err(err) => {
                    log::error!("MM Component dispatched: Id = [{name:?}] Status = [Failed] Error = [{err:?}]");
                    true
                }
            }
        });
        len != self.components.len()
    }

    /// Dispatches until a pass makes no progress.
    pub fn dispatch_to_completion(&mut self) {
        while self.dispatch() {}
    }

    /// Logs every component that was not dispatched, with the reason when known.
    pub fn display_not_dispatched(&self) {
        if self.components.is_empty() && self.rejected.is_empty() {
            return;
        }

        log::warn!("MM components not dispatched:");
        for component in self.components.iter().chain(&self.rejected) {
            let metadata = component.metadata();
            log::warn!("  {} - {}", metadata.name(), metadata.error_message().unwrap_or(Cow::from("")));
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use patina::{
        BinaryGuid,
        component::{
            component,
            hob::{FromHob, Hob, HobParseError},
            params::{Config as ConfigParam, ConfigMut},
            service::{IntoService as IntoServiceDerive, Service as ServiceParam},
        },
        pi::hob::{END_OF_HOB_LIST, GUID_EXTENSION, GuidHob, Hob as PiHob, HobHeader},
    };

    const COUNTER_HOB_GUID: BinaryGuid = BinaryGuid::from_string("00000000-0000-0000-0000-0000000000a1");
    const UNPARSED_HOB_GUID: BinaryGuid = BinaryGuid::from_string("00000000-0000-0000-0000-0000000000a2");

    static RUNS: AtomicUsize = AtomicUsize::new(0);
    static OBSERVED: AtomicU64 = AtomicU64::new(0);

    trait Greeter {
        fn greet(&self) -> u64;
    }

    #[derive(IntoServiceDerive)]
    #[service(dyn Greeter)]
    struct GreeterImpl;

    impl Greeter for GreeterImpl {
        fn greet(&self) -> u64 {
            7
        }
    }

    struct CountingComponent;

    #[component]
    impl CountingComponent {
        fn entry_point(self) -> patina::error::Result<()> {
            RUNS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct SecondComponent;

    #[component]
    impl SecondComponent {
        fn entry_point(self) -> patina::error::Result<()> {
            RUNS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FailingComponent;

    #[component]
    impl FailingComponent {
        fn entry_point(self) -> patina::error::Result<()> {
            Err(patina::error::EfiError::Unsupported)
        }
    }

    struct NeedsGreeter;

    #[component]
    impl NeedsGreeter {
        fn entry_point(self, greeter: ServiceParam<dyn Greeter>) -> patina::error::Result<()> {
            OBSERVED.store(greeter.greet(), Ordering::SeqCst);
            Ok(())
        }
    }

    struct NeedsLockedConfig;

    #[component]
    impl NeedsLockedConfig {
        fn entry_point(self, value: ConfigParam<u64>) -> patina::error::Result<()> {
            OBSERVED.store(*value, Ordering::SeqCst);
            Ok(())
        }
    }

    struct WritesConfig;

    #[component]
    impl WritesConfig {
        fn entry_point(self, mut value: ConfigMut<u64>) -> patina::error::Result<()> {
            *value += 1;
            Ok(())
        }
    }

    /// Requests the same configuration both mutably and immutably, which storage rejects
    /// during initialization.
    struct ConflictingComponent;

    #[component]
    impl ConflictingComponent {
        fn entry_point(self, _write: ConfigMut<u64>, _read: ConfigParam<u64>) -> patina::error::Result<()> {
            Ok(())
        }
    }

    struct CounterHob {
        value: u64,
    }

    impl FromHob for CounterHob {
        const HOB_GUID: BinaryGuid = COUNTER_HOB_GUID;

        fn parse(bytes: &[u8]) -> Result<Self, HobParseError> {
            Ok(Self { value: u64::from_le_bytes(bytes[..8].try_into().expect("payload holds a u64")) })
        }
    }

    struct NeedsCounterHob;

    #[component]
    impl NeedsCounterHob {
        fn entry_point(self, hob: Hob<CounterHob>) -> patina::error::Result<()> {
            OBSERVED.store(hob.value, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Builds a contiguous PI HOB list in heap memory.
    ///
    /// The HOB iterator walks raw memory from the first header to the end-of-list marker, so
    /// the entries must be laid out consecutively and every length kept a multiple of eight to
    /// keep the following header aligned.
    struct HobListBuffer {
        bytes: Vec<u8>,
        aligned: Vec<u64>,
    }

    impl HobListBuffer {
        fn new() -> Self {
            Self { bytes: Vec::new(), aligned: Vec::new() }
        }

        fn guid_hob(mut self, guid: BinaryGuid, payload: &[u8]) -> Self {
            let length = (size_of::<GuidHob>() + payload.len()).next_multiple_of(8);
            let header = GuidHob {
                header: HobHeader { r#type: GUID_EXTENSION, length: length as u16, reserved: 0 },
                name: guid,
            };

            // SAFETY: `GuidHob` is `repr(C)` and holds only integers and a GUID, so every byte
            // of it is initialized and safe to copy out.
            self.bytes.extend_from_slice(unsafe {
                core::slice::from_raw_parts(core::ptr::from_ref(&header).cast::<u8>(), size_of::<GuidHob>())
            });
            self.bytes.extend_from_slice(payload);
            self.bytes.resize(length + self.bytes.len() - (size_of::<GuidHob>() + payload.len()), 0);
            self
        }

        fn misc_hob(mut self, hob_type: u16) -> Self {
            let header = HobHeader { r#type: hob_type, length: size_of::<HobHeader>() as u16, reserved: 0 };
            // SAFETY: `HobHeader` is `repr(C)` and fully initialized.
            self.bytes.extend_from_slice(unsafe {
                core::slice::from_raw_parts(core::ptr::from_ref(&header).cast::<u8>(), size_of::<HobHeader>())
            });
            self
        }

        fn build(mut self) -> Self {
            self = self.misc_hob(END_OF_HOB_LIST);
            self.aligned = vec![0u64; self.bytes.len().div_ceil(8)];
            // SAFETY: `aligned` owns at least `bytes.len()` bytes and is 8-byte aligned because
            // it is allocated as `u64`.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.bytes.as_ptr(),
                    self.aligned.as_mut_ptr().cast::<u8>(),
                    self.bytes.len(),
                );
            }
            self
        }

        /// Returns the first entry, which the dispatcher iterates from.
        fn first(&self) -> PiHob<'_> {
            let ptr = self.aligned.as_ptr().cast::<GuidHob>();
            // SAFETY: `build` copied a well-formed GUID HOB to the start of the aligned buffer,
            // and its payload follows immediately after the header.
            unsafe {
                let header = &*ptr;
                let payload_len = header.header.length as usize - size_of::<GuidHob>();
                PiHob::GuidHob(header, core::slice::from_raw_parts(ptr.add(1).cast::<u8>(), payload_len))
            }
        }
    }

    #[test]
    fn test_component_dispatcher_starts_empty() {
        let dispatcher = MmComponentDispatcher::default();

        assert!(dispatcher.components.is_empty());
        assert!(dispatcher.rejected.is_empty());
    }

    #[test]
    fn test_component_info_defaults_register_nothing() {
        struct BarePlatform;
        impl MmComponentInfo for BarePlatform {}

        let mut dispatcher = MmComponentDispatcher::new();
        dispatcher.apply_component_info::<BarePlatform>();

        assert!(dispatcher.components.is_empty());
        assert!(dispatcher.rejected.is_empty());
        assert!(!dispatcher.dispatch(), "an empty dispatcher makes no progress");
    }

    #[test]
    fn test_component_info_registers_configs_services_and_components() {
        struct Platform;

        impl MmComponentInfo for Platform {
            fn configs(mut add: Add<'_, Config>) {
                add.config(42u64);
            }

            fn services(mut add: Add<'_, Service>) {
                add.service(GreeterImpl);
            }

            fn components(mut add: Add<'_, Component>) {
                add.component(CountingComponent);
                add.component(SecondComponent);
            }
        }

        let mut dispatcher = MmComponentDispatcher::new();
        assert!(dispatcher.storage.get_config::<u64>().is_none());
        assert!(dispatcher.storage.get_service::<dyn Greeter>().is_none());

        dispatcher.apply_component_info::<Platform>();

        assert!(dispatcher.storage.get_config::<u64>().is_some());
        assert!(dispatcher.storage.get_service::<dyn Greeter>().is_some());
        // `Add::component` appends, so registration order is preserved.
        let names: Vec<_> =
            dispatcher.components.iter().map(|component| component.metadata().name().to_string()).collect();
        assert_eq!(names.len(), 2);
        assert!(names[0].contains("CountingComponent"), "unexpected first component: {names:?}");
        assert!(names[1].contains("SecondComponent"), "unexpected second component: {names:?}");
    }

    #[test]
    fn test_insert_component_rejects_a_component_that_fails_to_initialize() {
        let mut dispatcher = MmComponentDispatcher::new();

        dispatcher.insert_component(0, ConflictingComponent.into_component());

        assert!(dispatcher.components.is_empty(), "a component that cannot initialize is never dispatchable");
        assert_eq!(dispatcher.rejected.len(), 1);
    }

    #[test]
    fn test_dispatch_runs_a_component_once_and_reports_progress() {
        let mut dispatcher = MmComponentDispatcher::new();
        dispatcher.insert_component(0, CountingComponent.into_component());

        assert!(dispatcher.dispatch());
        assert_eq!(RUNS.load(Ordering::SeqCst), 1);
        // The component was consumed, so a second pass has nothing left to do.
        assert!(!dispatcher.dispatch());
        assert_eq!(RUNS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_dispatch_retains_a_component_whose_dependency_is_missing() {
        let mut dispatcher = MmComponentDispatcher::new();
        dispatcher.insert_component(0, NeedsGreeter.into_component());

        assert!(!dispatcher.dispatch(), "no progress while the service is absent");
        assert_eq!(dispatcher.components.len(), 1);

        dispatcher.add_service(GreeterImpl);

        assert!(dispatcher.dispatch());
        assert!(dispatcher.components.is_empty());
        assert_eq!(OBSERVED.load(Ordering::SeqCst), 7);
    }

    #[test]
    fn test_dispatch_consumes_a_component_that_returns_an_error() {
        let mut dispatcher = MmComponentDispatcher::new();
        dispatcher.insert_component(0, FailingComponent.into_component());

        // A failed entry point still counts as dispatched; it must not be retried forever.
        assert!(dispatcher.dispatch());
        assert!(dispatcher.components.is_empty());
    }

    #[test]
    fn test_locking_configs_releases_components_waiting_to_read() {
        let mut dispatcher = MmComponentDispatcher::new();
        dispatcher.add_config(99u64);
        // Registering a `ConfigMut` component unlocks the datum, which holds readers back until
        // every writer has had its turn.
        dispatcher.insert_component(0, WritesConfig.into_component());
        dispatcher.insert_component(1, NeedsLockedConfig.into_component());

        assert!(dispatcher.dispatch(), "the writer runs while the config is unlocked");
        assert_eq!(dispatcher.components.len(), 1, "the reader is still waiting on the lock");

        dispatcher.lock_configs();

        assert!(dispatcher.dispatch());
        assert_eq!(OBSERVED.load(Ordering::SeqCst), 100, "the reader sees the writer's update");
    }

    #[test]
    fn test_dispatch_to_completion_drains_every_ready_component() {
        let mut dispatcher = MmComponentDispatcher::new();
        dispatcher.insert_component(0, CountingComponent.into_component());
        dispatcher.insert_component(1, SecondComponent.into_component());
        dispatcher.insert_component(2, NeedsGreeter.into_component());

        dispatcher.dispatch_to_completion();

        assert_eq!(RUNS.load(Ordering::SeqCst), 2);
        // `NeedsGreeter` is still waiting on a service that was never registered.
        assert_eq!(dispatcher.components.len(), 1);
    }

    #[test]
    fn test_display_not_dispatched_is_quiet_when_nothing_is_pending() {
        let dispatcher = MmComponentDispatcher::new();
        dispatcher.display_not_dispatched();
    }

    #[test]
    fn test_display_not_dispatched_reports_pending_and_rejected_components() {
        crate::test_support::init_test_logger();
        let mut dispatcher = MmComponentDispatcher::new();
        dispatcher.insert_component(0, NeedsGreeter.into_component());
        dispatcher.insert_component(1, ConflictingComponent.into_component());

        assert!(!dispatcher.dispatch());
        assert_eq!(dispatcher.components.len(), 1);
        assert_eq!(dispatcher.rejected.len(), 1);
        dispatcher.display_not_dispatched();
    }

    #[test]
    fn test_insert_hobs_parses_guided_hobs_with_a_registered_parser() {
        let hobs = HobListBuffer::new()
            .guid_hob(COUNTER_HOB_GUID, &1234u64.to_le_bytes())
            // A guided HOB nobody registered a parser for, and a non-guided HOB, are both skipped.
            .guid_hob(UNPARSED_HOB_GUID, &7u64.to_le_bytes())
            .misc_hob(patina::pi::hob::CPU)
            .build();

        let mut dispatcher = MmComponentDispatcher::new();
        dispatcher.insert_component(0, NeedsCounterHob.into_component());
        dispatcher.insert_hobs(&hobs.first());

        assert!(dispatcher.dispatch());
        assert_eq!(OBSERVED.load(Ordering::SeqCst), 1234);
    }

    #[test]
    fn test_insert_hobs_without_a_matching_parser_leaves_storage_untouched() {
        let hobs = HobListBuffer::new().guid_hob(UNPARSED_HOB_GUID, &7u64.to_le_bytes()).build();

        let mut dispatcher = MmComponentDispatcher::new();
        dispatcher.insert_component(0, NeedsCounterHob.into_component());
        dispatcher.insert_hobs(&hobs.first());

        assert!(!dispatcher.dispatch(), "the awaited HOB was never produced");
    }
}
