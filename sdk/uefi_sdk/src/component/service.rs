//! A module for the [Service] param type and any common service traits.
//!
//! The [Service] [Param] is a wait for components to produce and consume services defined by an interface (`Trait`)
//! that is agnostic to the underlying concrete implementation. It also allows a single concrete type to be used as
//! multiple services by implementing multiple traits on the same type and specifying the services trait(s) in the
//! [IntoService] derive macro.
//!
//! To simplify the management of services, the underlying datum is *always* readonly. This means that only `&self`
//! interface methods will be available to consumers of the service. If a service needs to be mutable, it should use
//! interior mutability to achieve this.
//!
//! The backing data is maintained as a static untyped type, which allows for one service to use another service in
//! it's implementation without needing to know about the underlying type or worry about lifetimes of holding one
//! service inside of another service.
//!
//! Similar to other [Param] implementations, any component that consumes a service will not be dispatched until the
//! services is produced, which allows for ordered dispatch of components that depend on each other. As mentioned
//! above, if one service depends on another service, not only will the service be produced first, but that service
//! can also be consumed by the dependent service before being produced
//!
//! ## Protocol Backwards Compatability
//!
//! While not suggested, it is possible to publish a service as an EDKII compatible protocol for backwards
//! compatability with existing EDKII code, allowing for a rust service to be consumed by an EDKII driver. As mentioned
//! multiple times, this is **only** for backwards compatability and should be avoided if possible. Any rust to rust
//! component interactions should be done through the [Service] [Param] type. Please review the [IntoService] trait on
//! how to register a service as an EDKII protocol.
//!
//! ## Example
//!
//! ### Implementing a Service
//!
//! See [IntoService][uefi_sdk_macro::IntoService] macro for more information on how to implement a service. While the
//! macro does not have to be used, it is recommended to ensure the service is implemented correctly.
//!
//! ### Basic Service Usage
//!
//! ```rust
//! use uefi_sdk::{
//!    error::Result,
//!    component::{
//!        service::{IntoService, Service},
//!        Storage,
//!    }
//! };
//!
//! trait MyService {}
//!
//! #[derive(IntoService)]
//! #[service(dyn MyService)]
//! struct MyServiceImpl;
//!
//! impl MyService for MyServiceImpl {}
//!
//! // This component will not be dispatched until the `MyService` service is produced.
//! fn my_component(service: Service<dyn MyService>) -> Result<()> {
//!     Ok(())
//! }
//!
//! // This component will be dispatched before `my_component` as it produces the `MyService` service.
//! fn service_producer(storage: &mut Storage) -> Result<()> {
//!     let service = MyServiceImpl;
//!     storage.add_service(service);
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Services Consuming Other Services
//!
//! The below example shows how one service can consume another service when producing the service.
//!
//! ```rust
//! use uefi_sdk::{
//!   error::Result,
//!   component::{
//!     service::{IntoService, Service},
//!     Storage,
//!   }
//! };
//!
//! trait Service1 {}
//! trait Service2 {}
//!
//! #[derive(IntoService)]
//! #[service(dyn Service1)]
//! struct Service1Impl;
//!
//! impl Service1 for Service1Impl {}
//!
//! #[derive(IntoService)]
//! #[service(dyn Service2)]
//! struct Service2Impl {
//!    service1: Service<dyn Service1>,
//! }
//!
//! impl Service2 for Service2Impl {}
//!
//! fn service1_producer(storage: &mut Storage) -> Result<()> {
//!   let service = Service1Impl;
//!   storage.add_service(service);
//!   Ok(())
//! }
//!
//! fn service2_producer(storage: &mut Storage, service1: Service<dyn Service1>) -> Result<()> {
//!   let service = Service2Impl { service1 };
//!   storage.add_service(service);
//!   Ok(())
//! }
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
extern crate alloc;

use alloc::boxed::Box;
use core::{any::Any, marker::PhantomData, ops::Deref};

use crate::component::{
    metadata::MetaData,
    params::Param,
    storage::{Storage, UnsafeStorageCell},
};

pub mod memory;

#[doc(hidden)]
pub use r_efi::efi::Guid;

pub mod mm_communicator;
pub mod platform_mm_control;
pub mod sw_mmi_manager;
pub use uefi_sdk_macro::IntoService;

/// A trait that allows the implementor to register a service with the underlying [Storage].
pub trait IntoService {
    /// Registers the service with the underlying [Storage], consuming self
    fn register(self, storage: &mut Storage);
    /// Helper function to register the service.
    ///
    /// ## Safety
    ///
    /// - Caller must ensure the registered service is a static dyn Any, where the underlying type is a Box\<dyn T\>
    ///   where T is the Service trait.
    ///
    /// ## Example
    ///
    /// ``` rust
    /// extern crate alloc;
    ///
    /// use alloc::boxed::Box;
    /// use uefi_sdk::component::{Storage, service::IntoService};
    ///
    /// struct MyStruct;
    ///
    /// trait Service {}
    ///
    /// impl Service for MyStruct {}
    ///
    /// impl IntoService for MyStruct {
    ///   fn register(self, storage: &mut Storage) {
    ///     let boxed: Box<dyn Service> = Box::new(self);
    ///     let leaked: &'static dyn core::any::Any = Box::leak(Box::new(boxed));
    ///     Self::register_service::<dyn Service>(storage, leaked);
    ///   }
    /// }
    ///
    /// ```
    fn register_service<S: ?Sized + 'static>(storage: &mut Storage, service: &'static dyn Any) {
        let id = storage.register_service::<S>();
        storage.insert_service(id, service);
    }
}

/// A service with a static lifetime that can be used as a parameter to a [Component](super::Component).
///
/// The underlying service that this object wraps can be either a concrete type such as a struct or enum, or a dyn
/// trait object. In nearly all cases, the service should be a dyn trait object so that consumers of the service can
/// rely on the service being the same regardless of the underlying implementation.
///
/// This type has a static lifetime, which means it can can be consumed during component execution, such as being used
/// as backing functionality for another service that is being produced by the component.
///
/// While implementing [IntoService] is possible, it is advised to use the [IntoService](uefi_sdk_macro::IntoService)
/// derive macro, which also provides more information.
pub struct Service<T: ?Sized + 'static> {
    value: &'static dyn Any,
    _marker: core::marker::PhantomData<T>,
}

impl<T: ?Sized + 'static> Service<T> {
    /// Creates an instance of Service by creating a Box\<dyn T\> and then leaking it to a static lifetime.
    ///
    /// This function is intended for testing purposes only. Dropping the returned value will cause a memory leak as
    /// the underlying (leaked) Box cannot be deallocated.
    ///
    /// ## Example
    ///
    /// ```rust
    /// use uefi_sdk::component::service::Service;
    ///
    /// trait Service1 {
    ///   fn do_something(&self) -> u32;
    /// }
    ///
    /// struct MockService;
    ///
    ///   impl Service1 for MockService {
    ///     fn do_something(&self) -> u32 {
    ///       42
    ///     }
    ///   }
    ///
    /// fn my_component_to_test(service: Service<dyn Service1>) {
    ///   let _ = service.do_something();
    /// }
    ///
    /// #[test]
    /// fn test_my_component() {
    ///   // Create a mock, maybe use mockall?
    ///   let service = Service::mock(Box::new(MockService));
    ///   my_component_to_test(service);
    /// }
    /// ```
    #[allow(clippy::test_attr_in_doctest)]
    pub fn mock(value: Box<T>) -> Self {
        let leaked: &'static dyn core::any::Any = Box::leak(Box::new(value));
        Self { value: leaked, _marker: PhantomData }
    }
}

impl<T: ?Sized + 'static> From<&'static dyn Any> for Service<T> {
    fn from(value: &'static dyn Any) -> Self {
        Self { value, _marker: PhantomData }
    }
}

impl<T: ?Sized + 'static> Deref for Service<T> {
    type Target = Box<T>;

    fn deref(&self) -> &Self::Target {
        self.value.downcast_ref().unwrap_or_else(|| panic!("Config should be of type {}", core::any::type_name::<T>()))
    }
}

impl<T: ?Sized + 'static> Clone for Service<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized + 'static> Copy for Service<T> {}

unsafe impl<T: ?Sized + 'static> Param for Service<T> {
    type State = usize;
    type Item<'storage, 'state> = Service<T>;

    unsafe fn get_param<'storage, 'state>(
        state: &'state Self::State,
        storage: UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state> {
        Service::from(storage.storage().get_raw_service(*state).unwrap_or_else(|| {
            panic!("Could not find Service value with id [{}] even though it was just validated.", *state)
        }))
    }

    fn validate(state: &Self::State, storage: UnsafeStorageCell) -> bool {
        unsafe { storage.storage() }.get_raw_service(*state).is_some()
    }

    fn init_state(storage: &mut Storage, _meta: &mut MetaData) -> Self::State {
        storage.register_service::<T>()
    }
}

#[cfg(test)]
mod tests {
    use super::IntoService;
    use super::*;

    #[test]
    fn test_service_derive_service_macro() {
        use crate as uefi_sdk;

        trait MyService {
            fn do_something(&self) -> u32;
        }

        trait MyService2 {
            fn do_something2(&self) -> u32;
        }

        #[derive(IntoService)]
        #[service(dyn MyService)]
        struct MyServiceImpl;

        impl MyService for MyServiceImpl {
            fn do_something(&self) -> u32 {
                42
            }
        }

        #[derive(IntoService)]
        #[service(dyn MyService2)]
        struct MyService2Impl {
            inner: Service<dyn MyService>,
        }

        impl MyService2 for MyService2Impl {
            fn do_something2(&self) -> u32 {
                self.inner.do_something()
            }
        }

        let mut storage = Storage::new();
        storage.add_service(MyServiceImpl);

        let s = storage.get_service::<dyn MyService>().unwrap();
        assert_eq!(42, s.do_something());

        storage.add_service(MyService2Impl { inner: s });
        let s2 = storage.get_service::<dyn MyService2>().unwrap();
        assert_eq!(42, s2.do_something2());

        storage.add_service(MyServiceImpl);

        #[derive(IntoService)]
        #[service(Self)]
        struct SomeStruct {
            x: u32,
        }

        storage.add_service(SomeStruct { x: 1 });
        let s3 = storage.get_service::<SomeStruct>().unwrap();
        assert_eq!(1, s3.x)
    }

    #[test]
    fn test_available_service_validates_true() {
        use crate as uefi_sdk;

        trait MyService {
            fn do_something(&self) -> u32;
        }

        #[derive(IntoService)]
        #[service(dyn MyService)]
        struct MyServiceImpl;

        impl MyService for MyServiceImpl {
            fn do_something(&self) -> u32 {
                42
            }
        }

        let mut storage = Storage::default();
        let mut mock_metadata = MetaData::new::<i32>();

        let id = <Service<dyn MyService> as Param>::init_state(&mut storage, &mut mock_metadata);

        storage.add_service(MyServiceImpl);

        assert!(<Service<dyn MyService> as Param>::try_validate(&id, (&storage).into()).is_ok());
        let service = unsafe { <Service<dyn MyService> as Param>::get_param(&id, (&storage).into()) };
        assert_eq!(42, service.do_something());
    }

    #[test]
    fn test_missing_service_validates_false() {
        trait MyService {
            #[allow(dead_code)]
            fn do_something(&self) -> u32;
        }

        let mut storage = Storage::default();
        let mut mock_metadata = MetaData::new::<i32>();

        let id = <Service<dyn MyService> as Param>::init_state(&mut storage, &mut mock_metadata);
        assert!(<Service<dyn MyService> as Param>::try_validate(&id, (&storage).into()).is_err());
    }

    #[test]
    #[should_panic]
    fn test_get_param_without_validate_should_panic_when_missing() {
        trait MyService {
            #[allow(dead_code)]
            fn do_something(&self) -> u32;
        }

        let storage = Storage::default();
        let _service =
            unsafe { <Service<dyn MyService> as Param>::get_param(&0, UnsafeStorageCell::new_readonly(&storage)) };
    }

    #[test]
    fn test_mocking_works() {
        trait MyService {
            fn do_something(&self) -> u32;
        }

        struct MockService;

        impl MyService for MockService {
            fn do_something(&self) -> u32 {
                42
            }
        }

        let service = Service::mock(Box::new(MockService));
        assert_eq!(42, service.do_something());
    }

    #[test]
    fn test_services_can_be_copied() {
        trait MyService {
            fn do_something(&self) -> u32;
        }

        struct MockService;

        impl MyService for MockService {
            fn do_something(&self) -> u32 {
                42
            }
        }

        fn consume_service(service: Service<dyn MyService>) {
            assert_eq!(42, service.do_something());
        }

        let service: Service<dyn MyService> = Service::mock(Box::new(MockService));
        consume_service(service);
        consume_service(service); // This should work as well, since Service is Copy
    }
}
