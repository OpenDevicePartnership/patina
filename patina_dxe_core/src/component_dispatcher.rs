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
use patina::{
    boot_services::StandardBootServices,
    component::{IntoComponent, Storage, service::IntoService},
    pi::hob::HobList,
    runtime_services::StandardRuntimeServices,
};
use r_efi::efi;

use alloc::{boxed::Box, vec::Vec};

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
    pub fn component<I>(&mut self, component: impl IntoComponent<I>) {
        let component = component.into_component();
        self.dispatcher.insert_component(self.dispatcher.components.len(), component);
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
    components: Vec<Box<dyn patina::component::Component>>,
    storage: Storage,
}

impl Default for ComponentDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Safety: The ComponentDispatcher is `Send` as all data stored within this structure is owned by it, and not shared.
unsafe impl Send for ComponentDispatcher {}

impl ComponentDispatcher {
    /// Creates a new locked ComponentDispatcher.
    #[inline(always)]
    pub(crate) const fn new_locked() -> TplMutex<Self> {
        TplMutex::new(efi::TPL_NOTIFY, Self::new(), "ComponentDispatcher")
    }

    /// Creates a new ComponentDispatcher.
    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self { components: Vec::new(), storage: Storage::new() }
    }

    /// Applies the component information provided by the given type implementing [ComponentInfo].
    pub(crate) fn apply_component_info<C: ComponentInfo>(&mut self) {
        C::configs(Add::new(self));
        C::services(Add::new(self));
        C::components(Add::new(self));
    }

    /// Inserts a component at the given index.
    pub(crate) fn insert_component(&mut self, idx: usize, mut component: Box<dyn patina::component::Component>) {
        component.initialize(&mut self.storage);
        self.components.insert(idx, component);
    }

    /// Adds a service to storage.
    #[coverage(off)]
    #[inline(always)]
    pub(crate) fn add_service<S: IntoService + 'static>(&mut self, service: S) {
        self.storage.add_service(service);
    }

    /// Locks the configurations in storage, preventing further modifications.
    #[coverage(off)]
    #[inline(always)]
    pub(crate) fn lock_configs(&mut self) {
        self.storage.lock_configs();
    }

    /// Sets the Boot Services table in storage.
    #[coverage(off)]
    #[inline(always)]
    pub(crate) fn set_boot_services(&mut self, bs: StandardBootServices) {
        self.storage.set_boot_services(bs);
    }

    /// Sets the Runtime Services table in storage.
    #[coverage(off)]
    #[inline(always)]
    pub(crate) fn set_runtime_services(&mut self, rs: StandardRuntimeServices) {
        self.storage.set_runtime_services(rs);
    }

    /// Parses the HOB list producing a `Hob\<T\>` struct for each guided HOB found with a registered parser.
    pub(crate) fn insert_hobs(&mut self, hob_list: &HobList<'_>) {
        for hob in hob_list.iter() {
            if let patina::pi::hob::Hob::GuidHob(guid, data) = hob {
                let parser_funcs = self.storage.get_hob_parsers(&patina::OwnedGuid::from(guid.name));
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
        let len = self.components.len();
        self.components.retain_mut(|component| {
            // Ok(true): Dispatchable and dispatched returning success
            // Ok(false): Not dispatchable at this time.
            // Err(e): Dispatchable and dispatched returning failure
            let name = component.metadata().name();
            log::trace!("Dispatch Start: Id = [{name:?}]");
            !match component.run(&mut self.storage) {
                Ok(true) => {
                    log::info!("Dispatched: Id = [{name:?}] Status = [Success]");
                    true
                }
                Ok(false) => false,
                Err(err) => {
                    log::error!("Dispatched: Id = [{name:?}] Status = [Failed] Error = [{err:?}]");
                    debug_assert!(false);
                    true // Component dispatched, even if it did fail, so remove from self.components to avoid re-dispatch.
                }
            }
        });
        len != self.components.len()
    }

    /// Logs all components that were not dispatched, and the parameter that was not satisfied that prevented dispatch.
    #[coverage(off)]
    pub(crate) fn display_not_dispatched(&self) {
        if !self.components.is_empty() {
            let name_len = "name".len();
            let param_len = "failed_param".len();

            let max_name_len = self.components.iter().map(|c| c.metadata().name().len()).max().unwrap_or(name_len);
            let max_param_len = self
                .components
                .iter()
                .map(|c| c.metadata().failed_param().map(|s| s.len()).unwrap_or(0))
                .max()
                .unwrap_or(param_len);

            log::warn!("Components not dispatched:");
            log::warn!("{:-<max_name_len$} {:-<max_param_len$}", "", "");
            log::warn!("{:<max_name_len$} {:<max_param_len$}", "name", "failed_param");

            for component in &self.components {
                let metadata = component.metadata();
                log::warn!(
                    "{:<max_name_len$} {:<max_param_len$}",
                    metadata.name(),
                    metadata.failed_param().unwrap_or("")
                );
            }
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use patina::pi::hob::GuidHob;

    use crate::test_support::with_global_lock;

    use super::*;

    #[test]
    fn test_component_info_trait_is_implemented() {
        struct Test;

        impl ComponentInfo for Test {}

        let mut dispatcher = ComponentDispatcher::new();
        dispatcher.apply_component_info::<Test>();
        assert!(dispatcher.components.is_empty());
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

        #[derive(patina::component::IntoComponent)]
        struct TestComponent;

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
        assert!(dispatcher.components.is_empty());

        dispatcher.apply_component_info::<Test>();

        assert!(dispatcher.storage.get_config::<u32>().is_some());
        assert!(dispatcher.storage.get_service::<dyn TestService>().is_some());
        assert_eq!(dispatcher.components.len(), 1);
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
            header: patina::pi::hob::header::Hob {
                r#type: patina::pi::hob::GUID_EXTENSION,
                length: core::mem::size_of::<TestHob1>() as u16,
                reserved: 0,
            },
            name: *GUID1,
        };

        let guid_hob2 = GuidHob {
            header: patina::pi::hob::header::Hob {
                r#type: patina::pi::hob::GUID_EXTENSION,
                length: core::mem::size_of::<TestHob2>() as u16,
                reserved: 0,
            },
            name: *GUID2,
        };

        let guid_hob3 = GuidHob {
            header: patina::pi::hob::header::Hob {
                r#type: patina::pi::hob::GUID_EXTENSION,
                length: core::mem::size_of::<TestHob3>() as u16,
                reserved: 0,
            },
            name: *GUID3,
        };

        hob_list.push(patina::pi::hob::Hob::GuidHob(&guid_hob1, hob1_bytes));
        hob_list.push(patina::pi::hob::Hob::GuidHob(&guid_hob2, hob2_bytes));
        hob_list.push(patina::pi::hob::Hob::GuidHob(&guid_hob3, hob3_bytes));
        hob_list.push(patina::pi::hob::Hob::Misc(30)); // Non-guid HOB to ensure it's ignored.

        #[derive(patina::component::IntoComponent)]
        struct TestComponent;

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
        dispatcher.insert_component(0, TestComponent.into_component());
        dispatcher.insert_hobs(&hob_list);

        assert!(dispatcher.dispatch());
    }

    #[test]
    fn test_reentrant_lock_correctly_displays_name() {
        assert!(
            with_global_lock(|| {
                let dispatcher = ComponentDispatcher::new_locked();
                let _lock = dispatcher.lock();
                dispatcher.lock();
            })
            .is_err_and(|e| {
                e.downcast::<String>().unwrap().contains("Re-entrant locks for \"ComponentDispatcher\" not permitted.")
            })
        );
    }

    #[test]
    fn test_dispatch_missing_shows_not_dispatched() {
        #[derive(patina::component::IntoComponent)]
        struct TestComponent;

        trait TestService {}

        impl TestComponent {
            fn entry_point(self, _: patina::component::service::Service<dyn TestService>) -> patina::error::Result<()> {
                Ok(())
            }
        }

        let mut dispatcher = ComponentDispatcher::default();
        dispatcher.insert_component(0, TestComponent.into_component());
        assert!(!dispatcher.dispatch());
        dispatcher.display_not_dispatched();
    }

    #[test]
    #[should_panic]
    fn test_error_in_component_debug_asserts() {
        #[derive(patina::component::IntoComponent)]
        struct TestComponent;

        impl TestComponent {
            fn entry_point(self) -> patina::error::Result<()> {
                Err(patina::error::EfiError::Unsupported)
            }
        }

        let mut dispatcher = ComponentDispatcher::default();
        dispatcher.insert_component(0, TestComponent.into_component());
        dispatcher.dispatch();
    }
}
