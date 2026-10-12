//! DXE Core subsystem for the Patina component dispatcher.
//!
//! This subsystem is responsible for managing the lifecycle of Patina components, their data, and their execution.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use crate::tpl_mutex::TplMutex;
use core::any::TypeId;
use patina::standard::efi;
use patina::{
    component::{Entity, IntoComponent, Query, QueryIter, Storage, service::IntoService},
    log_debug_assert,
    pi::hob::HobList,
    uefi::runtime_services::StandardRuntimeServices,
};
use patina_test::__private_api::DispatchInfo;

use alloc::{borrow::Cow, boxed::Box, vec::Vec};

/// A private wrapper around a Patina component to prevent a non-patina_dxe_core consumer from
/// access to the underlying component via Query parameters.
pub(crate) struct StoredComponent(Option<Box<dyn patina::component::Component>>);

impl StoredComponent {
    fn component(&self) -> &dyn patina::component::Component {
        self.0.as_deref().expect("stored component must be present outside its execution")
    }
}

type ComponentQueryData = (Entity, &'static mut StoredComponent, &'static mut DispatchInfo);
type ComponentQuery = Query<ComponentQueryData>;
type ComponentQueryIter<'storage> = QueryIter<'storage, ComponentQueryData>;

/// A trait to be implemented by the platform to register additional components, configurations, and services.
///
/// Allocations are available when these callbacks are invoked.
///
/// ## Example
///
/// ```rust
/// use patina_dxe_core::*;
/// struct MyPlatform;
///
/// impl ComponentInfo for MyPlatform {
///   fn configs(mut add: Add<Config>) {
///     add.config(32u32);
///     add.config(true);
///   }
/// }
/// ```
#[cfg_attr(test, mockall::automock)]
#[allow(clippy::used_underscore_binding)] // Allow underscore bindings to avoid unused variable warnings.
pub trait ComponentInfo: Sized {
    /// A platform callback to register components with the core.
    #[inline(always)]
    fn components<'a>(_add: Add<'a, Component>) {}

    /// A platform callback to register configurations with the core.
    #[inline(always)]
    fn configs<'a>(_add: Add<'a, Config>) {}

    /// A platform callback to register services with the core.
    #[inline(always)]
    fn services<'a>(_add: Add<'a, Service>) {}
}

/// A marker to limit [Add] methods to only adding [Component](patina::component::Component)s
pub struct Component;
/// A marker to limit [Add] methods to only adding Configs.
pub struct Config;
/// A marker to limit [Add] methods to only adding [Service](patina::component::service::Service)s
pub struct Service;

/// A struct used to allow controlled access to the Core's storage.
pub struct Add<'a, L> {
    /// The component dispatcher to add to.
    dispatcher: &'a mut ComponentDispatcher,
    /// Marker to limit what methods are available on this struct.
    _limiter: core::marker::PhantomData<L>,
}

impl<L> Add<'_, L> {
    /// Creates a new [Add] struct.
    #[inline(always)]
    pub(crate) fn new<'a>(dispatcher: &'a mut ComponentDispatcher) -> Add<'a, L> {
        Add { dispatcher, _limiter: core::marker::PhantomData }
    }
}

impl Add<'_, Component> {
    /// Adds a component to the core's component list.
    pub fn component<I, C>(&mut self, component: C)
    where
        C: IntoComponent<I> + 'static,
    {
        self.dispatcher.insert_component(component);
    }
}

impl Add<'_, Config> {
    /// Adds a configuration value to the core's storage.
    #[inline(always)]
    pub fn config<C: Default + 'static>(&mut self, config: C) {
        self.dispatcher.storage.add_config::<C>(config);
    }
}

impl Add<'_, Service> {
    /// Adds a service to the core's storage.
    #[inline(always)]
    pub fn service(&mut self, service: impl IntoService + 'static) {
        self.dispatcher.storage.add_service(service);
    }
}

pub(crate) struct ComponentDispatcher {
    /// Storage for components to use during execution.
    storage: Storage,
    /// Lazily validated query over registered component rows.
    component_query: Option<ComponentQuery>,
}

impl Default for ComponentDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// SAFETY: The `ComponentDispatcher` is `Send` as all data stored within this structure is owned by it, and not shared.
unsafe impl Send for ComponentDispatcher {}

impl ComponentDispatcher {
    /// Creates a new locked `ComponentDispatcher`.
    ///
    /// Uses `TPL_APPLICATION` so that component entry points can use boot services
    /// that are restricted at higher TPL levels.
    #[inline(always)]
    pub(crate) const fn new_locked() -> TplMutex<Self> {
        TplMutex::new(efi::TPL_APPLICATION, Self::new(), "ComponentDispatcher")
    }

    /// Creates a new `ComponentDispatcher`.
    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self { storage: Storage::new(), component_query: None }
    }

    /// Applies the component information provided by the given type implementing [`ComponentInfo`].
    pub(crate) fn apply_component_info<C: ComponentInfo>(&mut self) {
        C::configs(Add::new(self));
        C::services(Add::new(self));
        C::components(Add::new(self));
    }

    /// Inserts a component.
    pub(crate) fn insert_component<I, C>(&mut self, component: C)
    where
        C: IntoComponent<I> + 'static,
    {
        let mut component = component.into_component();
        let mut dispatch_info = DispatchInfo::new(TypeId::of::<C>());
        if !component.initialize(&mut self.storage) {
            dispatch_info.set_rejected();
        }
        self.storage
            .create_entity()
            .add_fields((dispatch_info, StoredComponent(Some(component))))
            .expect("new component entity must accept its initial fields");
    }

    /// Returns all registered components and their dispatch state.
    pub(crate) fn components(&mut self) -> ComponentQueryIter<'_> {
        let Self { storage, component_query } = self;
        component_query.get_or_insert_with(Query::new).query_mut(storage)
    }

    /// Adds a configuration value to storage.
    #[cfg_attr(coverage, coverage(off))]
    pub(crate) fn add_config<C: Default + 'static>(&mut self, config: C) {
        self.storage.add_config(config);
    }

    /// Adds a service to storage.
    #[cfg_attr(coverage, coverage(off))]
    #[inline(always)]
    pub(crate) fn add_service<S: IntoService + 'static>(&mut self, service: S) {
        self.storage.add_service(service);
    }

    /// Gets a service from storage.
    #[cfg_attr(coverage, coverage(off))]
    #[inline(always)]
    pub(crate) fn get_service<S: IntoService + 'static>(&mut self) -> Option<patina::component::service::Service<S>> {
        self.storage.get_service::<S>()
    }

    /// Locks the configurations in storage, preventing further modifications.
    #[cfg_attr(coverage, coverage(off))]
    #[inline(always)]
    pub(crate) fn lock_configs(&mut self) {
        self.storage.lock_configs();
    }

    /// Sets the Runtime Services table in storage.
    #[cfg_attr(coverage, coverage(off))]
    #[inline(always)]
    pub(crate) fn set_runtime_services(&mut self, rs: StandardRuntimeServices) {
        self.storage.set_runtime_services(rs);
    }

    /// Sets the core Image Handle in storage.
    #[cfg_attr(coverage, coverage(off))]
    #[inline(always)]
    pub(crate) fn set_image_handle(&mut self, handle: efi::Handle) {
        self.storage.set_image_handle(handle);
    }

    /// Parses the HOB list producing a `Hob\<T\>` struct for each guided HOB found with a registered parser.
    pub(crate) fn insert_hobs(&mut self, hob_list: &HobList<'_>) {
        for hob in hob_list.iter() {
            if let patina::pi::hob::Hob::GuidHob(guid, data) = hob {
                let parser_funcs = self.storage.get_hob_parsers(&guid.name);
                if parser_funcs.is_empty() {
                    let (f0, f1, f2, f3, f4, &[f5, f6, f7, f8, f9, f10]) = guid.name.as_fields();
                    let name = alloc::format!(
                        "{f0:08x}-{f1:04x}-{f2:04x}-{f3:02x}{f4:02x}-{f5:02x}{f6:02x}{f7:02x}{f8:02x}{f9:02x}{f10:02x}"
                    );
                    log::warn!(
                        "No parser registered for HOB: GuidHob {{ {:?}, name: Guid {{ {} }} }}",
                        guid.header,
                        name
                    );
                } else {
                    for parser_func in parser_funcs {
                        parser_func(data, &mut self.storage);
                    }
                }
            }
        }
    }

    /// Attempts to dispatch all components.
    ///
    /// This method will perform a single pass over all registered components, attempting to run each one.
    ///
    /// Returns `true` if at least one component was successfully dispatched, `false` otherwise.
    pub(crate) fn dispatch(&mut self) -> bool {
        let pending = self
            .components()
            .filter_map(|(entity, _, dispatch_info)| dispatch_info.is_pending().then_some(entity))
            .collect::<Vec<_>>();

        let mut dispatched = false;
        for entity in pending {
            let mut component = {
                let (_, stored, _) = self.components().get(&entity).expect("pending entity must contain its component");
                stored.0.take().expect("pending component must be present")
            };
            let name = component.metadata().name();
            log::trace!("Dispatch Start: Id = [{name:?}]");
            let result = component.run(&mut self.storage);

            let (_, stored, dispatch_info) =
                self.components().get(&entity).expect("dispatched entity must retain its component fields");
            stored.0 = Some(component);
            match result {
                Ok(true) => {
                    dispatch_info.set_succeeded();
                    dispatched = true;
                }
                Ok(false) => {}
                Err(error) => {
                    dispatch_info.set_failed();
                    log_debug_assert!("Dispatched: Id = [{name:?}] Status = [Failed] Error = [{error}]");
                }
            }
        }
        dispatched
    }

    /// Logs all components that were not dispatched, and the parameter that was not satisfied that prevented dispatch.
    #[cfg_attr(coverage, coverage(off))]
    pub(crate) fn display_not_dispatched(&mut self) {
        let not_dispatched = self
            .components()
            .filter(|(_, _, dispatch_info)| dispatch_info.is_pending() || dispatch_info.is_rejected())
            .map(|(_, component, _)| {
                let metadata = component.component().metadata();
                (metadata.name(), metadata.error_message().unwrap_or(Cow::from("")))
            })
            .collect::<Vec<_>>();

        if !not_dispatched.is_empty() {
            let name_len = "name".len();
            let param_len = "error message".len();
            let max_name_len = not_dispatched.iter().map(|(name, _)| name.len()).max().unwrap_or(name_len);
            let max_param_len = not_dispatched.iter().map(|(_, error)| error.len()).max().unwrap_or(param_len);

            log::warn!("Components not dispatched:");
            log::warn!("{:-<max_name_len$} {:-<max_param_len$}", "", "");
            log::warn!("{:<max_name_len$} {:<max_param_len$}", "name", "error message");

            for (name, error) in not_dispatched {
                log::warn!("{name:<max_name_len$} {error:<max_param_len$}");
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use patina::{component::component, pi::hob::GuidHob};

    use crate::test_support::with_global_lock;

    use super::*;

    fn dispatch_info_of<C: 'static>(dispatcher: &mut ComponentDispatcher) -> Option<DispatchInfo> {
        dispatcher.components().find_map(|(_, _, dispatch_info)| {
            (dispatch_info.component_type_id() == TypeId::of::<C>()).then_some(*dispatch_info)
        })
    }

    #[test]
    fn test_component_info_trait_is_implemented() {
        struct Test;

        impl ComponentInfo for Test {}

        let mut dispatcher = ComponentDispatcher::new();
        assert!(dispatcher.component_query.is_none());
        dispatcher.apply_component_info::<Test>();
        assert_eq!(dispatcher.components().count(), 0);
        assert!(dispatcher.component_query.is_some());
        assert_eq!(dispatcher.components().count(), 0);
    }

    #[test]
    fn test_add_struct_correctly_applies_changes_to_storage() {
        struct Test;

        /// A test service and implementation so we can add it via the Add<Service> struct
        /// and verify it was added to storage.
        trait TestService {}

        #[derive(patina::component::service::IntoService)]
        #[service(dyn TestService)]
        struct TestServiceImpl;

        impl TestService for TestServiceImpl {}

        struct TestComponent;

        #[component]
        impl TestComponent {
            fn entry_point(self) -> patina::error::Result<()> {
                Ok(())
            }
        }

        impl ComponentInfo for Test {
            fn configs(mut add: Add<Config>) {
                add.config(42u32);
            }

            fn services(mut add: Add<Service>) {
                add.service(TestServiceImpl);
            }

            fn components(mut add: Add<Component>) {
                add.component(TestComponent);
            }
        }

        let mut dispatcher = ComponentDispatcher::new();
        assert!(dispatcher.storage.get_config::<u32>().is_none());
        assert!(dispatcher.storage.get_service::<dyn TestService>().is_none());
        assert_eq!(dispatcher.components().count(), 0);

        dispatcher.apply_component_info::<Test>();

        assert!(dispatcher.storage.get_config::<u32>().is_some());
        assert!(dispatcher.storage.get_service::<dyn TestService>().is_some());
        assert_eq!(dispatcher.components().count(), 1);
    }

    #[test]
    fn test_platform_components_append_after_core_components() {
        struct CoreComponent;

        #[component]
        impl CoreComponent {
            fn entry_point(self) -> patina::error::Result<()> {
                Ok(())
            }
        }

        struct PlatformComponent;

        #[component]
        impl PlatformComponent {
            fn entry_point(self) -> patina::error::Result<()> {
                Ok(())
            }
        }

        struct Platform;

        impl ComponentInfo for Platform {
            fn components(mut add: Add<Component>) {
                add.component(PlatformComponent);
            }
        }

        let mut dispatcher = ComponentDispatcher::new();
        dispatcher.insert_component(CoreComponent);
        dispatcher.apply_component_info::<Platform>();

        let component_types =
            dispatcher.components().map(|(_, _, dispatch_info)| dispatch_info.component_type_id()).collect::<Vec<_>>();
        assert_eq!(component_types, vec![TypeId::of::<CoreComponent>(), TypeId::of::<PlatformComponent>()]);
    }

    #[test]
    fn test_component_with_unregistered_service_is_deferred_until_service_added() {
        trait TestService {
            fn value(&self) -> u32;
        }

        #[derive(patina::component::service::IntoService)]
        #[service(dyn TestService)]
        struct TestServiceImpl;

        impl TestService for TestServiceImpl {
            fn value(&self) -> u32 {
                42
            }
        }

        struct TestComponent;

        #[component]
        impl TestComponent {
            fn entry_point(
                self,
                service: patina::component::service::Service<dyn TestService>,
            ) -> patina::error::Result<()> {
                assert_eq!(service.value(), 42);
                Ok(())
            }
        }

        let mut dispatcher = ComponentDispatcher::default();
        dispatcher.insert_component(TestComponent);
        assert!(dispatch_info_of::<TestComponent>(&mut dispatcher).unwrap().is_pending());

        // The service the component depends on hasn't been added yet, so the component must be deferred for
        // a later retry, not permanently failed.
        assert!(!dispatcher.dispatch());
        assert!(dispatch_info_of::<TestComponent>(&mut dispatcher).unwrap().is_pending());

        dispatcher.add_service(TestServiceImpl);

        // Now that the service is present, the previously-deferred component should dispatch successfully.
        assert!(dispatcher.dispatch());
        assert!(dispatch_info_of::<TestComponent>(&mut dispatcher).unwrap().is_succeeded());
        assert_eq!(dispatcher.components().count(), 1, "completed component rows remain queryable");
        assert!(!dispatcher.dispatch(), "completed components are not dispatched again");
    }

    #[test]
    fn test_component_service_becomes_available_after_dispatch() {
        trait TestService {}

        #[derive(patina::component::service::IntoService)]
        #[service(dyn TestService)]
        struct TestServiceImpl;

        impl TestService for TestServiceImpl {}

        struct TestComponent;

        #[component]
        impl TestComponent {
            fn entry_point(self, storage: &mut Storage) -> patina::error::Result<()> {
                storage.add_service(TestServiceImpl);
                Ok(())
            }
        }

        let mut dispatcher = ComponentDispatcher::default();
        dispatcher.insert_component(TestComponent);
        assert!(dispatcher.storage.get_service::<dyn TestService>().is_none());
        assert!(dispatcher.dispatch());
        assert!(dispatcher.storage.get_service::<dyn TestService>().is_some());
    }

    #[test]
    fn test_parse_hob_list_into_storage() {
        use zerocopy::IntoBytes;
        use zerocopy_derive::*;

        const GUID_STR1: &str = "00000000-0000-0000-0000-000000000001";
        const GUID_STR2: &str = "00000000-0000-0000-0000-000000000002";
        const GUID_STR3: &str = "00000000-0000-0000-0000-000000000003";
        const GUID1: patina::BinaryGuid = patina::BinaryGuid::from_string(GUID_STR1);
        const GUID2: patina::BinaryGuid = patina::BinaryGuid::from_string(GUID_STR2);
        const GUID3: patina::BinaryGuid = patina::BinaryGuid::from_string(GUID_STR3);
        const HOB1_VALUE: u32 = 1234;
        const HOB2_VALUE: u64 = 56789;

        let mut hob_list = HobList::new();

        #[derive(FromBytes, IntoBytes, Immutable, PartialEq, patina::component::hob::FromHob)]
        #[hob = "00000000-0000-0000-0000-000000000001"]
        struct TestHob1 {
            value: u32,
        }

        #[derive(FromBytes, IntoBytes, Immutable, PartialEq, patina::component::hob::FromHob)]
        #[hob = "00000000-0000-0000-0000-000000000002"]
        struct TestHob2 {
            value: u64,
        }

        #[derive(FromBytes, IntoBytes, Immutable, PartialEq, patina::component::hob::FromHob)]
        #[hob = "00000000-0000-0000-0000-000000000003"]
        struct TestHob3 {
            value: u128,
        }

        let hob1 = TestHob1 { value: HOB1_VALUE };
        let hob2 = TestHob2 { value: HOB2_VALUE };
        let hob3 = TestHob3 { value: 9876543210 };
        let hob1_bytes = &hob1.as_bytes();
        let hob2_bytes = &hob2.as_bytes();
        let hob3_bytes = &hob3.as_bytes();

        let guid_hob1 = GuidHob {
            header: patina::pi::hob::HobHeader {
                r#type: patina::pi::hob::GUID_EXTENSION,
                length: core::mem::size_of::<TestHob1>() as u16,
                reserved: 0,
            },
            name: GUID1,
        };

        let guid_hob2 = GuidHob {
            header: patina::pi::hob::HobHeader {
                r#type: patina::pi::hob::GUID_EXTENSION,
                length: core::mem::size_of::<TestHob2>() as u16,
                reserved: 0,
            },
            name: GUID2,
        };

        let guid_hob3 = GuidHob {
            header: patina::pi::hob::HobHeader {
                r#type: patina::pi::hob::GUID_EXTENSION,
                length: core::mem::size_of::<TestHob3>() as u16,
                reserved: 0,
            },
            name: GUID3,
        };

        hob_list.push(patina::pi::hob::Hob::GuidHob(&guid_hob1, hob1_bytes));
        hob_list.push(patina::pi::hob::Hob::GuidHob(&guid_hob2, hob2_bytes));
        hob_list.push(patina::pi::hob::Hob::GuidHob(&guid_hob3, hob3_bytes));
        hob_list.push(patina::pi::hob::Hob::Misc(30)); // Non-guid HOB to ensure it's ignored.

        struct TestComponent;

        #[component]
        impl TestComponent {
            fn entry_point(
                self,
                hob1: patina::component::hob::Hob<TestHob1>,
                hob2: patina::component::hob::Hob<TestHob2>,
            ) -> patina::error::Result<()> {
                assert_eq!(hob1.value, HOB1_VALUE);
                assert_eq!(hob2.value, HOB2_VALUE);
                Ok(())
            }
        }

        let mut dispatcher = ComponentDispatcher::default();
        dispatcher.insert_component(TestComponent);
        dispatcher.insert_hobs(&hob_list);

        assert!(dispatcher.dispatch());
    }

    #[test]
    fn test_reentrant_lock_correctly_displays_name() {
        assert!(
            with_global_lock(|| {
                let dispatcher = ComponentDispatcher::new_locked();
                let _lock = dispatcher.lock();
                let _reentrant_lock = dispatcher.lock();
            })
            .is_err_and(|e| {
                e.downcast::<String>().unwrap().contains("Re-entrant locks for \"ComponentDispatcher\" not permitted.")
            })
        );
    }

    #[test]
    fn test_dispatch_missing_shows_not_dispatched() {
        struct TestComponent;

        trait TestService {}

        #[component]
        impl TestComponent {
            fn entry_point(self, _: patina::component::service::Service<dyn TestService>) -> patina::error::Result<()> {
                Ok(())
            }
        }

        let mut dispatcher = ComponentDispatcher::default();
        dispatcher.insert_component(TestComponent);
        assert!(!dispatcher.dispatch());
        dispatcher.display_not_dispatched();
    }

    #[test]
    fn test_dispatch_records_failed_component_without_reporting_success() {
        struct TestComponent;

        #[component]
        impl TestComponent {
            fn entry_point(self) -> patina::error::Result<()> {
                Err(patina::error::EfiError::Unsupported)
            }
        }

        let mut dispatcher = ComponentDispatcher::default();
        dispatcher.insert_component(TestComponent);
        assert!(!dispatcher.dispatch(), "A failed component is not a successful dispatch");
        assert!(dispatch_info_of::<TestComponent>(&mut dispatcher).unwrap().is_failed());
        assert!(!dispatcher.dispatch(), "failed components are not dispatched again");
    }
}
