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
//! Components are defined by applying the `#[component]` attribute to an impl block that contains an `entry_point`
//! method. The `entry_point` method name is mandatory and cannot be customized.
//!
//! ### Basic Component Structure
//!
//! ```rust,ignore
//! use patina::component::component;
//! use patina::error::Result;
//!
//! pub struct MyComponent {
//!     data: u32,
//! }
//!
//! #[component]
//! impl MyComponent {
//!     fn entry_point(self) -> Result<()> {
//!         // Component logic here
//!         Ok(())
//!     }
//! }
//! ```
//!
//! The `#[component]` attribute automatically:
//! - Validates that an `entry_point` method exists
//! - Validates parameters for conflicts at compile time
//! - Generates the `IntoComponent` trait implementation
//!
//! ### Parameter Validation
//!
//! The attribute validates that there are no conflicting parameter combinations such as:
//! - Duplicate `ConfigMut<T>` types
//! - Both `Config<T>` and `ConfigMut<T>` for the same type T
//! - `&mut Storage` with `Config<T>` or `ConfigMut<T>`
//! - `&Storage` with `ConfigMut<T>`
//!
//! ### Entry Point Requirements
//!
//! The entry point function's first parameter must be `self, `mut self`, `&self` or `&mut self`. The rest of the
//! parameters must implement the [Param](params::Param) trait, which is described in more detail below.
//!
//! Note: there is an arbitrary parameter count limit of 5, but this can be changed in the future if needed. See the
//! [params] module for more information.
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
//!         component,
//!         params::Config,
//!     },
//! };
//!
//! struct MyStruct(u32);
//!
//! #[component]
//! impl MyStruct {
//!     fn entry_point(self, _cfg: Config<String>) -> Result<()> {
//!         Ok(())
//!     }
//! }
//!
//! struct MyStruct2(u32);
//!
//! #[component]
//! impl MyStruct2 {
//!     fn entry_point(self, _cfg: Config<String>) -> Result<()> {
//!        Ok(())
//!     }
//! }
//!
//! enum MyEnum {
//!    A,
//!    B,
//! }
//!
//! #[component]
//! impl MyEnum {
//!    fn entry_point(self, _cfg: Config<String>) -> Result<()> {
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
pub use storage::{Storage, UnsafeStorageCell};

/// A part of the private API that must be public for the component macro to work. Users should not use this directly
/// and it is subject to change at any time.
#[doc(hidden)]
pub use struct_component::StructComponent;

pub use patina_macro::component;

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
    ///
    /// Returns true if the component was successfully initialized, otherwise false.
    fn initialize(&mut self, storage: &mut storage::Storage) -> bool;

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
    pub use crate::{
        component::{
            IntoComponent,
            hob::{FromHob, Hob},
            params::{Commands, Config, ConfigMut},
            service::{IntoService, Service},
        },
        error::{EfiError, Result},
    };
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    extern crate std;

    use super::*;
    use crate as patina;
    use crate::{
        Guid, OwnedGuid,
        component::{
            component,
            hob::{FromHob, Hob},
            params::ConfigMut,
        },
        error::{EfiError, Result},
    };

    struct ComponentSuccess;

    #[component]
    impl ComponentSuccess {
        fn entry_point(self) -> Result<()> {
            Ok(())
        }
    }

    struct ComponentNotDispatchedConfig;

    #[component]
    impl ComponentNotDispatchedConfig {
        fn entry_point(self, _: ConfigMut<u32>) -> Result<()> {
            Ok(())
        }
    }

    struct ComponentFail;

    #[component]
    impl ComponentFail {
        fn entry_point(self) -> Result<()> {
            Err(EfiError::Aborted)
        }
    }

    #[derive(FromHob, zerocopy_derive::FromBytes)]
    #[hob = "d4ffc718-fb82-4274-9afc-aa8b1eef5293"]
    #[repr(C)]
    pub struct TestHob;

    struct ComponentHobDep1;

    #[component]
    impl ComponentHobDep1 {
        fn entry_point(self, _hob: Hob<TestHob>) -> Result<()> {
            Ok(())
        }
    }

    #[derive(FromHob, zerocopy_derive::FromBytes)]
    #[hob = "d4ffc718-fb82-4274-9afc-aa8b1eef5293"]
    #[repr(C)]
    pub struct TestHob2;

    struct ComponentHobDep2;

    #[component]
    impl ComponentHobDep2 {
        fn entry_point(self, _hob: Hob<TestHob2>) -> Result<()> {
            Ok(())
        }
    }
    struct ComponentHobDep3;

    #[component]
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
