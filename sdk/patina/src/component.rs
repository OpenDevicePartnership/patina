//! Module for creating UEFI components.
//!
//! This module provides a way to create UEFI components by allowing each component to define its own dependencies. The
//! component executor will automatically resolve the dependencies and execute the component.
//!
//! This module takes its inspiration from the [Entity Component System](https://en.wikipedia.org/wiki/Entity_component_system)
//! architectural pattern, while only using a subset of its described characteristics. This module's implementation is
//! heavily inspired by the [bevy_ecs](https://crates.io/crates/bevy_ecs) crate, which was created by the [Bevy](https://bevyengine.org/)
//! engine team.
//!
//! This module comes from the need to design a highly generic and extensible user interface for UEFI driver
//! development. As such, we only need a subset of the features offered by `bevy_ecs`, and thus we pulled out the core
//! functionality from `bevy_ecs` that is applicable to our needs, modified it to fit our use case, and expanded on it.
//!
//! ## Features
//!
//! This module has two main use cases: (1) for end users to write their own components and (2) for the DxeCore to manage
//! these components and their dependencies. (1) is always available, however (2) is only available when the `core`
//! feature flag is enabled.
//!
//! - `core`: Exposes additional items necessary to manage and execute components and their dependencies.
//!
//! ## Creating a Component
//!
//! The only requirement for a component is that it implements the [Component] trait. This trait defines the methods
//! necessary for a component to be executed by the DxeCore. The [Component] trait is public, so it can be implemented
//! by any user-defined type. So long as it implements [IntoComponent], it can be registered with and executed by the
//! DxeCore.
//!
//! ### `StructComponent`
//!
//! This crate provides a single component implementation, [StructComponent], which is a component that allows for
//! private internal configuration. To enable a struct or enum to be transformed into a [StructComponent], a derive
//! macro, [IntoComponent] is provided to implement the corresponding trait automatically. By default, the macro
//! expects the struct or enum to have a method named `entry_point` with the appropriate function signature, however
//! this can be overridden with the `#[entry_point = path::to::function]` attribute.
//!
//! It is important to note that the function's first parameter must be `self` or `mut self`, **NOT** `&self` or
//! `&mut self`. This design choice was made as components are only expected to be executed once, and by consuming
//! `self`, you are able to pass ownership of the entire struct (or items within the struct) to other "things" (for
//! lack of a better term) without the need for cloning or borrowing. The rest of the parameters must implement the
//! [Param](params::Param) trait, which is described in more detail below. If not all parameters implement
//! [Param](params::Param), the macro will succeed, but the underlying implementation will report a diagnostic error
//! at compile time.
//!
//! Note: there is an arbitrary parameter count limit of 5, but this can be changed in the future if needed. See the
//! [params] module for more information.
//!
//! Note: Certain combinations of parameters may lead to undefined behavior as they can allow multiple mutable
//! accesses to the same data. Each parameter type checks for conflicts with previously registered accesses, but
//! **ONLY** on debug builds. In release builds, these checks are disabled for performance and size reasons.
//!
//! ### `Param` types
//!
//! Below is a list of all types that implement the [Param](params::Param) trait, within this module. Other
//! implementations may still exist.
//!
//! | Param                        | Description                                                                                                                                                           |
//! |------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------|
//! | Hob\<T\>                     | A parsed, immutable, guid HOB (Hand-Off Block) that is automatically parsed and registered. See the [hob] module for more info.                                       |
//! | Commands                     | A command queue to apply structural changes to [Storage] such as registering services. See the [params] module for more info.
//! | Option\<P\>                  | An Option, where P implements `Param`. Allows components to run even when the underlying parameter is unavailable. See the [params] module for more info.             |
//! | (P1, P2, ...)                | A Tuple where each entry implements `Param`. Useful when you need more parameters than the current parameter limit. See the [params] module for more info.            |
//! | Config\<T\>                  | An immutable config value that will only be available once the underlying data has been locked. See The [params] module for more info.                                |
//! | ConfigMut\<T\>               | A mutable config value that will only be available while the underlying data is unlocked. See the [params] module for more info.                                      |
//! | Service\<T\>                 | A wrapper for producing and consuming services of a particular interface, `T`, that is agnostic to the underlying implementation. See [service] module for more info. |
//! | StandardBootServices         | Rust implementation of Boot Services                                                                                                                                  |
//!
//! ### Examples
//!
//! ### Compiled Examples
//!
//! This crate has multiple example binaries in it's `example` folder that can be compiled and executed. These show
//! implementations of common use cases and usage models for components and their parameters.
//!
//! ### Struct Component Example
//!
//! ```rust
//! use patina::{
//!     error::Result,
//!     component::{
//!         IntoComponent,
//!         params::Config,
//!     },
//! };
//!
//! #[derive(IntoComponent)]
//! struct MyStruct(u32);
//!
//! impl MyStruct {
//!     fn entry_point(self, _cfg: Config<String>) -> Result<()> {
//!         Ok(())
//!     }
//! }
//!
//! #[derive(IntoComponent)]
//! #[entry_point(path = driver)]
//! struct MyStruct2(u32);
//!
//! fn driver(s: MyStruct2, _cfg: Config<String>) -> Result<()> {
//!    Ok(())
//! }
//!
//! #[derive(IntoComponent)]
//! #[entry_point(path = MyEnum::run_me)]
//! enum MyEnum {
//!    A,
//!    B,
//! }
//!
//! impl MyEnum {
//!    fn run_me(self, _cfg: Config<String>) -> Result<()> {
//!       Ok(())
//!   }
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

pub mod hob;
mod metadata;
pub mod params;
pub mod service;
mod storage;
mod struct_component;

use crate::error::Result;

pub use metadata::MetaData;
pub use storage::Storage;
pub use storage::UnsafeStorageCell;

/// A part of the private API that must be public for the component macro to work. Users should not use this directly
/// and it is subject to change at any time.
#[doc(hidden)]
pub use struct_component::StructComponent;

pub use patina_macro::IntoComponent;

/// An executable object whose parameters implement [Param](params::Param).
pub trait Component {
    /// Runs the component when it does not have exclusive access to the storage.
    ///
    /// Components that run in parallel do not have exclusive access to the storage and thus must be executed using the
    /// this method.
    ///
    /// ## Safety
    ///
    /// - Each parameter must properly register its access, so the scheduler can ensure that there are no data
    ///   conflicts in [Params](params::Param) for parallel execution of components. See [Param::init_state](params::Param::init_state)
    ///   for more information on how to properly register parameter access.
    unsafe fn run_unsafe(&mut self, storage: storage::UnsafeStorageCell) -> Result<bool>;

    /// Runs the component with exclusive access to the storage.
    fn run(&mut self, storage: &mut storage::Storage) -> Result<bool> {
        storage.apply_deferred();
        let storage_cell = storage::UnsafeStorageCell::from(storage);
        // SAFETY: This is safe because this component has exclusive access to the storage.
        unsafe { self.run_unsafe(storage_cell) }
    }

    /// One-time initialization of the component. This is where parameter access requirements should be registered in
    /// the metadata of the component. The scheduler uses this metadata when scheduling components in a multi-threaded
    /// context. Typically this method will pass the metadata to each parameter to register its access requirements,
    /// but that is not a requirement.
    fn initialize(&mut self, storage: &mut storage::Storage);

    /// Returns the metadata of the component. used in a multi-threaded context to schedule components.
    fn metadata(&self) -> &metadata::MetaData;
}

/// A helper trait to convert an object into a [Component].
pub trait IntoComponent<Input> {
    /// Converts a non-[Component] struct into an object that does implement [Component].
    ///
    /// Returns a boxed trait object that implements [Component].
    fn into_component(self) -> alloc::boxed::Box<dyn Component>;
}

/// A prelude module that re-exports commonly used items from the `component` module.
pub mod prelude {
    pub use crate::component::IntoComponent;
    pub use crate::component::hob::{FromHob, Hob};
    pub use crate::component::params::{Commands, Config, ConfigMut};
    pub use crate::component::service::{IntoService, Service};
    pub use crate::error::{EfiError, Result};
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    extern crate std;

    use super::*;
    use crate as patina;
    use crate::{Guid, OwnedGuid};
    use crate::{
        component::{
            hob::{FromHob, Hob},
            params::ConfigMut,
        },
        error::{EfiError, Result},
    };

    #[derive(IntoComponent)]
    struct ComponentSuccess;

    impl ComponentSuccess {
        fn entry_point(self) -> Result<()> {
            Ok(())
        }
    }

    #[derive(IntoComponent)]
    struct ComponentNotDispatchedConfig;

    impl ComponentNotDispatchedConfig {
        fn entry_point(self, _: ConfigMut<u32>) -> Result<()> {
            Ok(())
        }
    }

    #[derive(IntoComponent)]
    struct ComponentFail;

    impl ComponentFail {
        fn entry_point(self) -> Result<()> {
            Err(EfiError::Aborted)
        }
    }

    #[derive(FromHob, Default, Clone, Copy)]
    #[hob = "d4ffc718-fb82-4274-9afc-aa8b1eef5293"]
    #[repr(C)]
    pub struct TestHob;

    #[derive(IntoComponent)]
    struct ComponentHobDep1;

    impl ComponentHobDep1 {
        fn entry_point(self, _hob: Hob<TestHob>) -> Result<()> {
            Ok(())
        }
    }

    #[derive(FromHob, Default, Clone, Copy)]
    #[hob = "d4ffc718-fb82-4274-9afc-aa8b1eef5293"]
    #[repr(C)]
    pub struct TestHob2;

    #[derive(IntoComponent)]
    struct ComponentHobDep2;

    impl ComponentHobDep2 {
        fn entry_point(self, _hob: Hob<TestHob2>) -> Result<()> {
            Ok(())
        }
    }
    #[derive(IntoComponent)]
    struct ComponentHobDep3;

    impl ComponentHobDep3 {
        fn entry_point(self, _hob: Hob<TestHob2>) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_component_run_return_handling() {
        const HOB_GUID: OwnedGuid =
            Guid::from_fields(0xd4ffc718, 0xfb82, 0x4274, 0x9a, 0xfc, [0xaa, 0x8b, 0x1e, 0xef, 0x52, 0x93]);

        let mut storage = storage::Storage::new();

        // Test component dispatched and succeeds does not panic does not panic and returns Ok(true)
        let mut component1 = ComponentSuccess.into_component();
        component1.initialize(&mut storage);
        assert!(component1.run(&mut storage).is_ok_and(|res| res));

        // Test component not dispatched does not panic and returns Ok(false)
        let mut component2 = ComponentNotDispatchedConfig.into_component();
        component2.initialize(&mut storage);
        storage.lock_configs(); // Lock the config so the component cannot run
        assert!(component2.run(&mut storage).is_ok_and(|res| !res));

        // Test component failed does not panic and returns Err(EfiError::<Something>)
        let mut component3 = ComponentFail.into_component();
        component3.initialize(&mut storage);
        assert!(component3.run(&mut storage).is_err_and(|res| res == EfiError::Aborted));

        let mut component4 = ComponentHobDep1.into_component();
        component4.initialize(&mut storage);
        assert!(component4.run(&mut storage).is_ok_and(|res| !res));

        let mut component5 = ComponentHobDep2.into_component();
        component5.initialize(&mut storage);
        assert!(component5.run(&mut storage).is_ok_and(|res| !res));

        let mut component6 = ComponentHobDep3.into_component();
        component6.initialize(&mut storage);
        assert!(component6.run(&mut storage).is_ok_and(|res| !res));

        storage.register_hob::<TestHob>();
        assert!(storage.get_hob::<TestHob>().is_none());

        // Two parsers should be registered for this HOB GUID since the HOBs are two unique types
        // (`TestHob` and `TestHob2`)
        assert!(storage.get_hob_parsers(&HOB_GUID).len() == 2);

        storage.add_hob(TestHob);
        assert!(storage.get_hob::<TestHob>().is_some());
        assert_eq!(storage.get_hob::<TestHob>().unwrap().iter().count(), 1);

        storage.add_hob(TestHob2);
        assert!(storage.get_hob::<TestHob2>().is_some());
        assert_eq!(storage.get_hob::<TestHob2>().unwrap().iter().count(), 1);

        // Both components should have there HOB dependencies satisfied
        assert!(component4.run(&mut storage).is_ok_and(|res| res));
        assert!(component5.run(&mut storage).is_ok_and(|res| res));
        assert!(component6.run(&mut storage).is_ok_and(|res| res));
    }
}
