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
