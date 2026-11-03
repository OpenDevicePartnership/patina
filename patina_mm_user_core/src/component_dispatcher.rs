//! MM User Core subsystem for the Patina component dispatcher.
//!
//! This subsystem brings the Patina component model (dependency-injected
//! `#[component]` entry points) into the MM User Core, mirroring the DXE Core's
//! `component_dispatcher` module. It lets a platform register components,
//! configurations, and services that are then dispatched in dependency order
//! during MM User Core startup.
//!
//! The component [`Storage`], [`Component`], and parameter types are reused
//! directly from the Patina SDK ([`patina::component`]); only the dispatcher and
//! the platform-facing [`MmComponentInfo`] registration trait live here.
//!
//! ## Relationship to MM driver dispatch
//!
//! This is distinct from the FFS/HOB-based MM driver dispatch performed by
//! [`MmDispatcher`](crate::mm_dispatcher::MmDispatcher). Both can coexist:
//! components are Rust objects registered by the platform binary, while MM
//! drivers are separate modules discovered from HOBs.
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

/// A trait implemented by the platform to register components, configurations,
/// and services with the MM User Core.
///
/// This is the MM analogue of the DXE Core's `ComponentInfo` trait. The platform
/// MM binary implements it and passes the implementing type to
/// [`MmUserCore::entry_point_worker`](crate::MmUserCore::entry_point_worker),
/// which applies it during `StartUserCore`.
///
/// Allocations are available when these callbacks are invoked.
///
/// ## Example
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
    #[inline(always)]
    fn components(_add: Add<'_, Component>) {}

    /// A platform callback to register configurations with the MM User Core.
    #[inline(always)]
    fn configs(_add: Add<'_, Config>) {}

    /// A platform callback to register services with the MM User Core.
    #[inline(always)]
    fn services(_add: Add<'_, Service>) {}
}

/// A marker to limit [`Add`] methods to only adding [`Component`](patina::component::Component)s.
pub struct Component;
/// A marker to limit [`Add`] methods to only adding configurations.
pub struct Config;
/// A marker to limit [`Add`] methods to only adding [`Service`](patina::component::service::Service)s.
pub struct Service;

/// A struct used to allow controlled access to the MM User Core's component storage.
///
/// The type parameter `L` limits which `add` methods are available, matching the
/// callback in [`MmComponentInfo`] that produced it.
pub struct Add<'a, L> {
    /// The component dispatcher to add to.
    dispatcher: &'a mut MmComponentDispatcher,
    /// Marker to limit what methods are available on this struct.
    _limiter: core::marker::PhantomData<L>,
}

impl<L> Add<'_, L> {
    /// Creates a new [`Add`] struct.
    #[inline(always)]
    pub(crate) fn new(dispatcher: &mut MmComponentDispatcher) -> Add<'_, L> {
        Add { dispatcher, _limiter: core::marker::PhantomData }
    }
}

impl Add<'_, Component> {
    /// Adds a component to the MM User Core's component list.
    pub fn component<I>(&mut self, component: impl IntoComponent<I>) {
        let component = component.into_component();
        let idx = self.dispatcher.components.len();
        self.dispatcher.insert_component(idx, component);
    }
}

impl Add<'_, Config> {
    /// Adds a configuration value to the MM User Core's storage.
    #[inline(always)]
    pub fn config<C: Default + 'static>(&mut self, config: C) {
        self.dispatcher.storage.add_config::<C>(config);
    }
}

impl Add<'_, Service> {
    /// Adds a service to the MM User Core's storage.
    #[inline(always)]
    pub fn service(&mut self, service: impl IntoService + 'static) {
        self.dispatcher.storage.add_service(service);
    }
}

/// The MM User Core component dispatcher.
///
/// Owns the registered components and the component [`Storage`] used for
/// dependency injection, and drives dispatch to a fixed point.
pub struct MmComponentDispatcher {
    /// Components that successfully initialized and are ready for dispatch attempts.
    components: Vec<Box<dyn patina::component::Component>>,
    /// Components that failed to initialize and are not ready for dispatch attempts.
    rejected: Vec<Box<dyn patina::component::Component>>,
    /// Storage for components to use during execution.
    storage: Storage,
}

impl Default for MmComponentDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: The MmComponentDispatcher owns all data stored within it and does not
// share it. It is only accessed by the BSP during single-threaded MM User Core
// startup, guarded by the containing `spin::Mutex`.
unsafe impl Send for MmComponentDispatcher {}

impl MmComponentDispatcher {
    /// Creates a new, empty `MmComponentDispatcher`.
    #[inline(always)]
    pub const fn new() -> Self {
        Self { components: Vec::new(), rejected: Vec::new(), storage: Storage::new() }
    }

    /// Applies the component information provided by the given type implementing
    /// [`MmComponentInfo`].
    pub fn apply_component_info<C: MmComponentInfo>(&mut self) {
        C::configs(Add::new(self));
        C::services(Add::new(self));
        C::components(Add::new(self));
    }

    /// Inserts a component at the given index, initializing it against storage.
    ///
    /// Components that fail initialization are moved to the rejected list and
    /// will not be dispatched.
    pub fn insert_component(&mut self, idx: usize, mut component: Box<dyn patina::component::Component>) {
        if component.initialize(&mut self.storage) {
            self.components.insert(idx, component);
        } else {
            self.rejected.push(component);
        }
    }

    /// Adds a service to storage.
    #[inline(always)]
    pub fn add_service<S: IntoService + 'static>(&mut self, service: S) {
        self.storage.add_service(service);
    }

    /// Adds a configuration value to storage.
    #[inline(always)]
    pub fn add_config<C: Default + 'static>(&mut self, config: C) {
        self.storage.add_config::<C>(config);
    }

    /// Locks the configurations in storage, preventing further modifications.
    ///
    /// This enables components that request an immutable `Config<T>` to be
    /// dispatched, and prevents further `ConfigMut<T>` components from running.
    #[inline(always)]
    pub fn lock_configs(&mut self) {
        self.storage.lock_configs();
    }

    /// Parses the HOB list, producing a `Hob<T>` datum for each guided HOB that
    /// has a registered parser.
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

    /// Attempts to dispatch all pending components in a single pass.
    ///
    /// Returns `true` if at least one component was dispatched (successfully or
    /// with an error), indicating progress and that another pass may dispatch
    /// more.
    pub fn dispatch(&mut self) -> bool {
        let len = self.components.len();
        self.components.retain_mut(|component| {
            let name = component.metadata().name();
            log::trace!("MM Dispatch Start: Id = [{name:?}]");
            // Ok(true):  dispatchable and dispatched successfully -> remove.
            // Ok(false): not dispatchable at this time -> retain.
            // Err(e):    dispatchable and dispatched with failure -> remove.
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

    /// Repeatedly dispatches components until no further progress is made.
    pub fn dispatch_to_completion(&mut self) {
        while self.dispatch() {}
    }

    /// Logs all components that were not dispatched and why.
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
        let names: Vec<_> = dispatcher.components.iter().map(|c| c.metadata().name().to_string()).collect();
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
