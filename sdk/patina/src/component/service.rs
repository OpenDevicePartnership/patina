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
//! See [IntoService][patina_macro::IntoService] macro for more information on how to implement a service. While the
//! macro does not have to be used, it is recommended to ensure the service is implemented correctly.
//!
//! ### Basic Service Usage
//!
//! ```rust
//! use patina::{
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
//! use patina::{
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
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use alloc::boxed::Box;
use core::{any::Any, cell::OnceCell, marker::PhantomData, ops::Deref};

use crate::component::{
    metadata::MetaData,
    params::Param,
    storage::{Storage, UnsafeStorageCell},
};

pub mod memory;
pub mod perf_timer;

pub use patina_macro::IntoService;

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
    /// use patina::component::{Storage, service::IntoService};
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
/// While implementing [IntoService] is possible, it is advised to use the [IntoService](patina_macro::IntoService)
/// derive macro, which also provides more information.
pub struct Service<T: ?Sized + 'static> {
    value: OnceCell<&'static dyn Any>,
    _marker: core::marker::PhantomData<T>,
}

impl<T: ?Sized + 'static> Service<T> {
    /// Creates a new instance of a service with an uninitialized value.
    ///
    /// Useful for const instantiation of a service, such as for static references or other types that require a
    /// static lifetime that is executed during compilation.
    ///
    /// If you use this function to create an uninitialized service, you **MUST** call [replace](Self::replace)
    /// before using the service, or else dereferencing the service will panic. If you cannot guarantee that the service
    /// will be initialized before use, consider using [map_or](Self::map_or), [map_or_else](Self::map_or_else), or
    /// [map_or_default](Self::map_or_default) for any access to the service.
    ///
    /// ## Example
    ///
    /// ```rust
    /// # use patina::{error::Result, component::service::Service};
    /// # trait MyService {}
    /// static MY_SERVICE: Service<dyn MyService> = Service::new_uninit();
    ///
    /// fn my_component(service: Service<dyn MyService>) -> Result<()> {
    ///     MY_SERVICE.replace(&service);
    ///     Ok(())
    /// }
    /// ```
    pub const fn new_uninit() -> Self {
        Self { value: OnceCell::new(), _marker: PhantomData }
    }

    /// Replaces the uninitialized service with the provided, initialized, service.
    ///
    /// ## Panics
    ///
    /// Panics if the service is already initialized or if the provided service is not initialized.
    pub fn replace(&self, service: &Service<T>) {
        let v = service.value.get().expect("Provided Service was not initialized!");
        self.value.set(*v).expect("Service was already initialized!");
    }

    /// Returns true if the service is initialized.
    pub fn is_init(&self) -> bool {
        self.value.get().is_some()
    }

    /// Returns the provided default result (if uninitalized), or applies a function to the contained value (if any).
    ///
    /// Arguments passed to map_or are eagerly evaluated; if you are passing the result of a function call, it is
    /// recommended to use map_or_else, which is lazily evaluated.
    ///
    /// ## Example
    ///
    /// ```rust
    /// use patina::component::service::Service;
    ///
    /// trait Example {
    ///   fn do_something(&self) -> u32;
    /// }
    ///
    /// let service: Service<dyn Example> = Service::new_uninit();
    /// assert_eq!(service.map_or(10, |s| s.do_something()), 10);
    /// ```
    pub fn map_or<U, F>(&self, default: U, f: F) -> U
    where
        F: FnOnce(&Service<T>) -> U,
    {
        if self.value.get().is_some() { f(self) } else { default }
    }

    /// Computes a default function result (if uninitialized), or applies a different function to the contained value (if any).
    ///
    /// ## Example
    ///
    /// ```rust
    /// use patina::component::service::Service;
    ///
    /// trait Example {
    ///   fn do_something(&self) -> u32;
    /// }
    /// let service: Service<dyn Example> = Service::new_uninit();
    /// assert_eq!(service.map_or_else(|| 10, |s| s.do_something()), 10);
    /// ```
    pub fn map_or_else<U, D, F>(&self, default: D, f: F) -> U
    where
        D: FnOnce() -> U,
        F: FnOnce(&Service<T>) -> U,
    {
        if self.value.get().is_some() { f(self) } else { default() }
    }

    /// Returns the default value of U (if uninitalized), or applies a function to the contained value (if any).
    ///
    /// ## Example
    ///
    /// ```rust
    /// use patina::component::service::Service;
    ///
    /// trait Example {
    ///  fn do_something(&self) -> u32;
    /// }
    ///
    /// let service: Service<dyn Example> = Service::new_uninit();
    /// assert_eq!(service.map_or_default(|s| s.do_something()), 0u32);
    /// ```
    pub fn map_or_default<U, F>(&self, f: F) -> U
    where
        U: Default,
        F: FnOnce(&Service<T>) -> U,
    {
        if self.value.get().is_some() { f(self) } else { U::default() }
    }

    /// Creates an instance of Service by creating a Box\<dyn T\> and then leaking it to a static lifetime.
    ///
    /// This function is intended for testing purposes only. Dropping the returned value will cause a memory leak as
    /// the underlying (leaked) Box cannot be deallocated.
    ///
    /// ## Example
    ///
    /// ```rust
    /// use patina::component::service::Service;
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
        let v: &'static T = Box::leak(value);
        let value = OnceCell::new();
        let leaked: &'static dyn core::any::Any = Box::leak(Box::new(v));
        value.set(leaked).expect("Once Cell was just created");
        Self { value, _marker: PhantomData }
    }
}

impl<T: ?Sized + 'static> From<&'static dyn Any> for Service<T> {
    fn from(value: &'static dyn Any) -> Self {
        let s = Self::new_uninit();
        s.value.set(value).expect("Once Cell was just created");
        s
    }
}

impl<T: ?Sized + 'static> Deref for Service<T> {
    type Target = &'static T;

    fn deref(&self) -> &Self::Target {
        if let Some(service) = self.value.get() {
            if let Some(service) = service.downcast_ref() {
                service
            } else {
                // Using core::hint::unreachable_unchecked() here results in the compiler optimizing away this entire
                // code path, and will result in UB if the path is reached. This code path truly is unreachable as we
                // (as patina developers) control all ways of instantiating a Service type.
                //
                // The performance impact of using this over unreachable! or panic! is about 25% improved performance
                // in benchmarks.
                //
                // # SAFETY
                // - The `Service` type tightly couples the underlying type to the `dyn Any` type for downcasting.
                // - All ways of instantiating a `Service` type are tightly controlled to ensure that this downcast is
                //   valid and will never fail including:
                //   - The `mock` function, which requires a `Box<dyn T>` which is then manually leaked, ensuring the
                //     underlying type is always available and the correct type.
                //   - The `from` function, which is passed data from `Storage`, that is guaranteed to be the correct
                //     type as it is generated via the `IntoService` macro and out of the hands of the user.
                //   - The `initialize` function, which consumes another `Service` type, which has the same guarantees
                //     as above.
                // - If the Service is uninitialized, it will panic at the normal unreachable! macro call below.
                // SAFETY: Service value was validated to be initialized in the Some(v) match arm.
                // unreachable_unchecked provides optimizer hints for the impossible None case.
                unsafe { core::hint::unreachable_unchecked() }
            }
        } else {
            // We use unreachable! here instead of panic! as this provides compiler hints to the optimizer. We cannot
            // use core::hint::unreachable_unchecked() here as we cannot guarantee that the service is initialized
            unreachable!("Service should be initialized first!");
        }
    }
}

impl<T: ?Sized + 'static> Clone for Service<T> {
    fn clone(&self) -> Self {
        Service { value: self.value.clone(), _marker: PhantomData }
    }
}

// SAFETY: Service<T> wraps a static reference with PhantomData.
// All access is through immutable Deref. Static lifetime and immutable access make it Send+Sync safe.
unsafe impl<T: ?Sized + 'static> Send for Service<T> {}
// SAFETY: Service<T> wraps a static reference with PhantomData.
// All access is through immutable Deref. Static lifetime and immutable access make it Send+Sync safe.
unsafe impl<T: ?Sized + 'static> Sync for Service<T> {}

// SAFETY: Service<T> parameter provides access to registered services.
// State tracks the service ID. Validates service availability before access.
unsafe impl<T: ?Sized + 'static> Param for Service<T> {
    type State = usize;
    type Item<'storage, 'state> = Service<T>;

    unsafe fn get_param<'storage, 'state>(
        state: &'state Self::State,
        storage: UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state> {
        // SAFETY: State was validated by validate() to contain a valid service ID.
        // UnsafeStorageCell provides exclusive access to storage under Param protocol.
        Service::from(unsafe {
            storage.storage().get_raw_service(*state).unwrap_or_else(|| {
                panic!("Could not find Service value with id [{}] even though it was just validated.", *state)
            })
        })
    }

    fn validate(state: &Self::State, storage: UnsafeStorageCell) -> bool {
        // SAFETY: Storage access is controlled by UnsafeStorageCell. Just checking service existence.
        unsafe { storage.storage() }.get_raw_service(*state).is_some()
    }

    fn init_state(storage: &mut Storage, _meta: &mut MetaData) -> Self::State {
        storage.register_service::<T>()
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::{IntoService, *};

    #[test]
    fn test_service_derive_service_macro() {
        use crate as patina;

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
        #[service(SomeStruct)]
        struct SomeStruct {
            x: u32,
        }

        storage.add_service(SomeStruct { x: 1 });
        let s3 = storage.get_service::<SomeStruct>().unwrap();
        assert_eq!(1, s3.x)
    }

    #[test]
    fn test_available_service_validates_true() {
        use crate as patina;

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
        // SAFETY: Test code - Service<dyn MyService> has been validated and is available in storage.
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
        // SAFETY: Test code - intentionally calling get_param without validation to test panic behavior.
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
        let service: Service<dyn MyService> = Service::mock(Box::new(MockService));
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
        consume_service(service.clone());
        consume_service(service); // This should work as well, since Service is Copy
    }

    #[test]
    fn test_basic_static_support() {
        use crate as patina;
        trait MyService {
            fn do_something(&self) -> u32;
        }

        #[derive(IntoService)]
        #[service(dyn MyService)]
        struct MockService {
            a: u32,
        }

        impl MockService {
            const fn new(a: u32) -> Self {
                Self { a }
            }
        }

        impl MyService for MockService {
            fn do_something(&self) -> u32 {
                self.a
            }
        }

        static MY_SERVICE: MockService = MockService::new(42);

        let mut storage = Storage::default();
        storage.add_service(&MY_SERVICE);

        let service = storage.get_service::<dyn MyService>().unwrap();
        assert_eq!(42, service.do_something());
    }

    #[test]
    fn test_replace_service_works() {
        trait MyService {
            fn do_something(&self) -> u32;
        }
        struct MockService;
        impl MyService for MockService {
            fn do_something(&self) -> u32 {
                42
            }
        }

        let service1: Service<dyn MyService> = Service::mock(Box::new(MockService));
        let service2: Service<dyn MyService> = Service::new_uninit();

        assert_eq!(42, service1.do_something());
        service2.replace(&service1);
        assert_eq!(42, service2.do_something());
        assert_eq!(42, service1.do_something()); // service1 should still work
    }

    #[test]
    #[should_panic = "Service should be initialized first!"]
    fn test_uninitialized_service_panics_instead_of_ub() {
        trait MyService {
            fn do_something(&self) -> u32;
        }

        let service: Service<dyn MyService> = Service::new_uninit();
        service.do_something(); // This should panic
    }

    #[test]
    fn test_map_function_on_uninit() {
        trait TestService {
            fn get_value(&self) -> u32;
        }

        let service: Service<dyn TestService> = Service::new_uninit();
        assert!(!service.is_init());

        assert_eq!(service.map_or(100, |s| s.get_value()), 100);
        assert_eq!(service.map_or_else(|| 200, |s| s.get_value()), 200);
        assert_eq!(service.map_or_default(|s| s.get_value()), 0);
    }

    #[test]
    fn test_map_functions_on_init() {
        trait TestService {
            fn get_value(&self) -> u32;
        }

        struct TestServiceImpl;

        impl TestService for TestServiceImpl {
            fn get_value(&self) -> u32 {
                42
            }
        }

        let service: Service<dyn TestService> = Service::mock(Box::new(TestServiceImpl));
        assert!(service.is_init());

        assert_eq!(service.map_or(100, |s| s.get_value()), 42);
        assert_eq!(service.map_or_else(|| 200, |s| s.get_value()), 42);
        assert_eq!(service.map_or_default(|s| s.get_value()), 42);
    }
}
