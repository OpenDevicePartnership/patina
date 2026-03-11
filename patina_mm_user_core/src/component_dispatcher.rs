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
        match component.initialize(&mut self.storage) {
            true => self.components.insert(idx, component),
            false => self.rejected.push(component),
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
        for entry in hob.into_iter() {
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
            log::warn!("  {} — {}", metadata.name(), metadata.error_message().unwrap_or(Cow::from("")));
        }
    }
}
