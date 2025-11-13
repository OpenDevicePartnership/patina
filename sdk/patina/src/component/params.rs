//! A module defining valid parameters for a [Component](super::Component).
//!
//! This module defines the [Param] trait, which is used to define how data is retrieved from the underlying
//! [Storage]. Any type that implements [Param] can be used as a parameter to a [Component](super::Component).
//!
//! Some custom types exist directly in this module, such as [Config] and [ConfigMut], however this trait is
//! implemented on many foreign types, so it is recommended to review the [Param] documentation directly,
//! which will show all types that can be used as parameters.
//!
//! ## Registering Access requirements
//!
//! It is the responsibility of each [Param] implementation to register it's access requirements with the
//! parent component's [MetaData]. This is done in the [init_state](Param::init_state) function. This is only
//! necessary for `Params` that can access data in both a mutable and immutable way. If accesses are only ever
//! immutable, then it is unnecessary.
//!
//! To enable parallel execution of components by the scheduler, the scheduler needs to be able to track what
//! parameters are used by each component and how these parameters are used. With this information, it can schedule
//! components to execute in parallel if they do not access the same data in a conflicting manner (e.g. one component
//! reads a value while another writes to it).
//!
//! To register access requirements, the [Param] trait has an [init_state](Param::init_state) function that is called
//! with mutable access to the component's [MetaData] which is used to store read / write access to certain types of
//! data as a bitset that must be maintained on a component-by-component basis. As it stands, the only data that can
//! possibly conflict with eachother are [Config] and [ConfigMut] as they reference the same underlying data in a
//! immutable and mutable manner. As new `Params` are added, access information in the [MetaData] struct may need to
//! be expanded to track more types of data.
//!
//! ## Param Function Size and Tuple Support
//!
//! For a function that supports dependency injection, a max parameter of 5 was selected. This is an arbitrary number
//! that is open to expansion in the future. The current implementation limit is indicated by the `impl_param_function`
//! macro usage in this module.
//!
//! To support the possible need of more than 5 parameters, tuples of parameters are supported. This also has an
//! arbitrary limit of 5 and is also open to expansion in the future. The current implementation limit is indicated by
//! the `impl_component_param_tuple` macro usage in this module.
//!
//! ### Example Tuple Usage
//!
//! ``` rust
//! use patina::component::params::{Config, ConfigMut};
//!
//! fn extremely_large_function(
//!     _config1: Config<i8>,
//!     _config2: Config<u8>,
//!     _config3: Config<i16>,
//!     _config4: Config<u16>,
//!     _config5: (Config<i32>, Config<u32>, Config<i64>, Config<u64>)
//! ) { /* todo */ }
//! ```
//!
//! ## Option\<P\> support
//!
//! As mentioned previously, components will not be executed unless all dependencies can be retrieved from the
//! underlying storage. In some circumstances, a component may wish to be executed even if the parameter is not
//! available. To support this functionality, you can wrap the parameter in an [Option] type. This will allow the
//! component to always run, but the option will be `None` if underlying parameter is not available.
//!
//! ### Example Option Usage
//!
//! ``` rust
//! # use patina::{error::Result, component::params::{ConfigMut, Config}};
//! // This component will execute even if the config is already locked. If the interface was just
//! // `config: ConfigMut<u32>`, and the config was locked, this component would never execute.
//! fn my_driver(mut config: Option<ConfigMut<u32>>) -> Result<()> {
//!     if let Some(mut config) = config {
//!        *config += 1;
//!     }
//!     // Continue on ...
//!     Ok(())
//! }
//! ```
//!
//! ## `Config` / `ConfigMut`
//!
//! A special note needs to be made about the [Config] and [ConfigMut] [Param] types, as they are intertwined.
//! The [Config] type is only available when the underlying datum is locked, while [ConfigMut] is only available while
//! the underlying datum is not locked. All Config datums are locked by default, however if a component is registered
//! with storage that requires it to be mutable, the datum will be unlocked. This is important because it means that
//! a component with a [Config] parameter will not be executed until the underlying datum is locked.
//!
//! A config datum can be locked via two separate ways:
//! 1. A component calling [lock](ConfigMut::lock) on the config datum
//! 2. Automatically by the core when all components that can execute, have executed, and components still exist in
//!    the queue.
//!
//! Once a config datum is locked, it cannot be unlocked, and no further components that have a [ConfigMut] parameter
//! will be executed.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use core::{
    cell::{Ref, RefCell, RefMut},
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use alloc::boxed::Box;

use crate::{
    boot_services::StandardBootServices,
    component::{
        metadata::MetaData,
        service::IntoService,
        storage::{Deferred, Storage, UnsafeStorageCell},
    },
    runtime_services::StandardRuntimeServices,
};

use super::storage::ConfigRaw;

type ParamItem<'w, 'state, P> = <P as Param>::Item<'w, 'state>;

/// A parameter that can be used in a [Component](super::Component).
///
/// ## Safety
///
/// - implementor must ensure [init_state](Param::init_state) correctly registers all
///   [Storage] accesses used by [get_param](Param::get_param) with provided
///   [MetaData].
/// - implementor must ensure [init_state](Param::init_state) validates the [Storage]
///   accesses used by [get_param](Param::get_param) does not conflict with any other
///   registered accesses found in the [MetaData]. Panics are allowed if this is violated.
pub unsafe trait Param {
    /// Data for the parameter that persists across component execution attempts.
    type State: Send + Sync + 'static;

    /// The item type that is retrieved from the [Storage].
    type Item<'storage, 'state>;

    /// Retrieves the item from [Storage].
    ///
    /// ## Safety
    ///
    /// - caller must ensure the [Item](Param::Item)'s access requirement is registered with
    ///   the owning [Component](super::Component).
    unsafe fn get_param<'storage, 'state>(
        _state: &'state Self::State,
        _storage: UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state>;

    /// Validates that [Item](Param::Item) exists and is in a state that can be retrieved
    /// from [Storage].
    fn validate(_state: &Self::State, _storage: UnsafeStorageCell) -> bool;

    /// A wrapper around [validate](Param::validate) that maps the boolean to a Result<(), &'static str>. where the
    /// &'static str is the name of the type that failed validation.
    fn try_validate(state: &Self::State, storage: UnsafeStorageCell) -> Result<(), &'static str> {
        if Self::validate(state, storage) { Ok(()) } else { Err(core::any::type_name::<Self>()) }
    }

    /// Initializes this Parameter's [State](Param::State).
    ///
    /// This is when the parameter should register its access requirements with the [MetaData]. See this module's
    /// top level documentation on how to properly register access requirements.
    fn init_state(storage: &mut Storage, meta: &mut MetaData) -> Self::State;
}

/// A hidden marker for functions that consume their input (take `In` by value).
/// These functions can only be executed once since they take ownership.
#[doc(hidden)]
pub struct RunOnce;

/// A hidden marker for functions that borrow their input (take `&In` or `&mut In`).
/// These functions can be executed multiple times since they don't consume the input.
#[doc(hidden)]
pub struct RunMany;

/// A trait that must be implemented by all components that have input.
#[doc(hidden)]
pub trait ComponentInput: Sized {}

#[doc(hidden)]
impl ComponentInput for () {}

/// A trait that allows the implementor to define a function whose parameters can be automatically retrieved from the
/// underlying [Storage] before being immediately executed.
#[diagnostic::on_unimplemented(
    message = "The function signature does not meet the requirements.\n\n{Self}\n",
    note = "1. The first parameter must be Self, &Self, or &mut Self.",
    note = "2. The remaining parameters must implement patina::component::params::Param",
    note = "3. Only a function with up to 5 parameters, excluding self, is supported.",
    note = "4. The return type must be patina::error::Result<()>"
)]
pub trait ParamFunction<Marker>: Send + Sync + 'static {
    /// All parameters of the function that are retrievable from [Storage].
    type Param: Param;
    /// The first input type of the function, () if there is no special input type.
    type In: ComponentInput;
    /// The return type of the function.
    type Out;

    /// Runs the function with the given input and parameter values.
    fn run(&mut self, input: &mut Option<Self::In>, param_value: ParamItem<Self::Param>) -> Self::Out;
}

macro_rules! impl_param_function {
    ($($param:ident),*) => {
        #[allow(unused_variables)]
        #[allow(non_snake_case)]
        impl<Out, Func, $($param : Param),*> ParamFunction<fn($($param,)*)->Out> for Func
        where
            Func: Send + Sync + 'static,
            for<'a, 'b> &'a mut Func:
                FnMut($($param), *) -> Out +
                FnMut($(ParamItem<$param>),*) -> Out,
            Out: 'static,
        {
            type Param = ($($param,)*);
            type In = ();
            type Out = Out;
            fn run(&mut self, _input: &mut Option<()>, param_value: ParamItem<($($param,)*)>) -> Out {
                fn call_inner<Out, $($param),*>(
                    mut f: impl FnMut($($param),*) -> Out,
                    $($param: $param,)*
                ) -> Out {
                    f($($param),*)
                }
                let ($($param,)*) = param_value;
                call_inner(self, $($param),*)
            }
        }

        #[allow(unused_variables)]
        #[allow(non_snake_case)]
        impl<In, Out, Func, $($param: Param),*> ParamFunction<(RunOnce, fn(In, $($param,)*)->Out)> for Func
        where
            Func: Send + Sync + 'static,
            for <'a> &'a mut Func:
                FnMut(In, $($param),*) -> Out +
                FnMut(In, $(ParamItem<$param>),*) -> Out,
            In: ComponentInput + 'static,
            Out: 'static,
        {
            type Param = ($($param,)*);
            type In = In;
            type Out = Out;
            fn run(&mut self, input: &mut Option<In>, param_value: ParamItem<($($param,)*)>) -> Out {
                #[allow(clippy::too_many_arguments)]
                fn call_inner<In, Out, $($param,)*>(
                    mut f: impl FnMut(In, $($param),*) -> Out,
                    input: In,
                    $($param: $param,)*
                ) -> Out {
                    f(input, $($param),*)
                }
                let ($($param,)*) = param_value;
                call_inner(self, input.take().unwrap(), $($param),*)
            }
        }

        #[allow(unused_variables)]
        #[allow(non_snake_case)]
        impl<In, Out, Func, $($param: Param),*> ParamFunction<(RunMany, fn(&mut In, $($param,)*)->Out)> for Func
        where
            Func: Send + Sync + 'static,
            for <'a, 'b> &'a mut Func:
                FnMut(&'b mut In, $($param),*) -> Out +
                FnMut(&'b mut In, $(ParamItem<$param>),*) -> Out,
            In: ComponentInput + 'static,
            Out: 'static,
        {
            type Param = ($($param,)*);
            type In = In;
            type Out = Out;
            fn run(&mut self, input: &mut Option<In>, param_value: ParamItem<($($param,)*)>) -> Out {
                #[allow(clippy::too_many_arguments)]
                fn call_inner<In, Out, $($param,)*>(
                    mut f: impl FnMut(&mut In, $($param),*) -> Out,
                    input: &mut In,
                    $($param: $param,)*
                ) -> Out {
                    f(input, $($param),*)
                }
                let ($($param,)*) = param_value;
                call_inner(self, input.as_mut().unwrap(), $($param),*)
            }
        }

        #[allow(unused_variables)]
        #[allow(non_snake_case)]
        impl<In, Out, Func, $($param: Param),*> ParamFunction<(RunMany, fn(&In, $($param,)*)->Out)> for Func
        where
            Func: Send + Sync + 'static,
            for <'a, 'b> &'a mut Func:
                FnMut(&'b In, $($param),*) -> Out +
                FnMut(&'b In, $(ParamItem<$param>),*) -> Out,
            In: ComponentInput + 'static,
            Out: 'static,
        {
            type Param = ($($param,)*);
            type In = In;
            type Out = Out;
            fn run(&mut self, input: &mut Option<In>, param_value: ParamItem<($($param,)*)>) -> Out {
                #[allow(clippy::too_many_arguments)]
                fn call_inner<In, Out, $($param,)*>(
                    mut f: impl FnMut(&In, $($param),*) -> Out,
                    input: &In,
                    $($param: $param,)*
                ) -> Out {
                    f(input, $($param),*)
                }
                let ($($param,)*) = param_value;
                call_inner(self, input.as_ref().unwrap(), $($param),*)
            }
        }
    }
}

impl_param_function!();
impl_param_function!(T1);
impl_param_function!(T1, T2);
impl_param_function!(T1, T2, T3);
impl_param_function!(T1, T2, T3, T4);
impl_param_function!(T1, T2, T3, T4, T5);
impl_param_function!(T1, T2, T3, T4, T5, T6);

// SAFETY: Option<P> makes parameter P optional. Always validates as available (returns Some/None based
// on P::validate). Safe delegation to P::get_param when P validates successfully.
unsafe impl<P: Param> Param for Option<P> {
    type State = P::State;
    type Item<'storage, 'state> = Option<P::Item<'storage, 'state>>;

    unsafe fn get_param<'storage, 'state>(
        state: &'state Self::State,
        storage: UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state> {
        match P::validate(state, storage) {
            // SAFETY: P::validate returned true, so P::get_param is safe to call with this state and storage.
            true => Some(unsafe { P::get_param(state, storage) }),
            false => None,
        }
    }

    fn validate(_state: &Self::State, _storage: UnsafeStorageCell) -> bool {
        // Always available
        true
    }

    fn init_state(storage: &mut Storage, meta: &mut MetaData) -> Self::State {
        P::init_state(storage, meta)
    }
}

/// An immutable configuration value registered with [Storage] prior to
/// [Component](super::Component) execution.
#[derive(Debug)]
pub struct Config<'c, T: Default + 'static> {
    value: Ref<'c, ConfigRaw>,
    _marker: PhantomData<T>,
}

impl<T: Default + 'static> Config<'_, T> {
    /// Creates an instance of Config by creating a RefCell and leaking it.
    ///
    /// This function is intended for testing purposes only. Dropping the returned value will cause a memory leak as
    /// the underlying (leaked) RefCell cannot be deallocated.
    ///
    /// ## Example
    /// ``` rust
    /// use patina::component::params::Config;
    ///
    /// fn my_component_to_test(config: Config<i32>) {
    ///     assert_eq!(*config, 42);
    /// }
    ///
    /// #[test]
    /// fn test_my_component() {
    ///     let config = Config::mock(42);
    ///     my_component_to_test(config);
    /// }
    /// ```
    #[allow(clippy::test_attr_in_doctest)]
    pub fn mock(value: T) -> Self {
        let refcell: RefCell<ConfigRaw> = RefCell::new(ConfigRaw::new(true, Box::new(value)));
        let leaked = Box::leak(Box::new(refcell));
        Config { value: leaked.borrow(), _marker: PhantomData }
    }
}

impl<T: Default + 'static> Deref for Config<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value.downcast_ref().unwrap_or_else(|| panic!("Config should be of type {}", core::any::type_name::<T>()))
    }
}

impl<'c, T: Default + 'static> From<Ref<'c, ConfigRaw>> for Config<'c, T> {
    fn from(value: Ref<'c, ConfigRaw>) -> Self {
        Self { value, _marker: PhantomData }
    }
}

// SAFETY: Config<T> parameter provides immutable access to locked configuration values.
// State tracks the config ID. Validates that config is locked before allowing access.
unsafe impl<T: Default + 'static> Param for Config<'_, T> {
    /// The id of the Config, so we can request it directly without converting T->id.
    type State = usize;
    type Item<'storage, 'state> = Config<'storage, T>;

    unsafe fn get_param<'storage, 'state>(
        lookup_id: &'state Self::State,
        storage: UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state> {
        // SAFETY: lookup_id is the config ID validated by init_state and validate.
        // UnsafeStorageCell provides exclusive access to storage under Param protocol.
        Config::from(unsafe { storage.storage().get_raw_config(*lookup_id) })
    }

    // `Config` is only available if the underlying datum is locked.
    fn validate(state: &Self::State, storage: UnsafeStorageCell) -> bool {
        // SAFETY: accesses are correctly registered with storage, no conflicts
        unsafe { storage.storage() }.get_raw_config(*state).is_locked()
    }

    fn init_state(storage: &mut Storage, meta: &mut MetaData) -> Self::State {
        let id = storage.add_config_default_if_not_present::<T>();

        debug_assert!(
            !meta.access().has_writes_all_configs(),
            "Config<{0}> in component {1} conflicts with a previous &mut Storage access.",
            core::any::type_name::<T>(),
            meta.name(),
        );

        debug_assert!(
            !meta.access().has_config_write(id),
            "Config<{0}> in component {1} conflicts with a previous ConfigMut<{0}> access.",
            core::any::type_name::<T>(),
            meta.name(),
        );

        meta.access_mut().add_config_read(id);
        id
    }
}

/// A mutable configuration value registered with [Storage] prior to
/// [Component](super::Component) execution.
#[derive(Debug)]
pub struct ConfigMut<'c, T: Default + 'static> {
    value: RefMut<'c, ConfigRaw>,
    _marker: PhantomData<T>,
}

impl<T: Default + 'static> ConfigMut<'_, T> {
    /// Creates an instance of Config by creating a RefCell and leaking it.
    ///
    /// This function is intended for testing purposes only. Dropping the returned value will cause a memory leak as
    /// the underlying (leaked) RefCell cannot be deallocated.
    ///
    /// ## Example
    /// ``` rust
    /// use patina::component::params::ConfigMut;
    ///
    /// fn my_component_to_test(config: ConfigMut<i32>) {
    ///     assert_eq!(*config, 42);
    /// }
    ///
    /// #[test]
    /// fn test_my_component() {
    ///     let config = ConfigMut::mock(42);
    ///     my_component_to_test(config);
    /// }
    /// ```
    #[allow(clippy::test_attr_in_doctest)]
    pub fn mock(value: T) -> Self {
        let refcell: RefCell<ConfigRaw> = RefCell::new(ConfigRaw::new(false, Box::new(value)));
        let leaked = Box::leak(Box::new(refcell));
        ConfigMut { value: leaked.borrow_mut(), _marker: PhantomData }
    }

    /// Locks the underlying datum, prevent any further changes and allowing [Config] to be used.
    pub fn lock(&mut self) {
        self.value.lock();
    }
}

impl<T: Default + 'static> Deref for ConfigMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value.downcast_ref().unwrap_or_else(|| panic!("Config should be of type {}", core::any::type_name::<T>()))
    }
}

impl<T: Default + 'static> DerefMut for ConfigMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.value.downcast_mut().unwrap_or_else(|| panic!("Config should be of type {}", core::any::type_name::<T>()))
    }
}

impl<'c, T: Default + 'static> From<RefMut<'c, ConfigRaw>> for ConfigMut<'c, T> {
    fn from(value: RefMut<'c, ConfigRaw>) -> Self {
        Self { value, _marker: PhantomData }
    }
}

// SAFETY: ConfigMut<T> parameter provides mutable access to unlocked configuration values.
// State tracks the config ID. Validates that config is unlocked before allowing mutable access.
unsafe impl<T: Default + 'static> Param for ConfigMut<'_, T> {
    /// The id of the Config, so we can request it directly without converting T->id.
    type State = usize;
    type Item<'storage, 'state> = ConfigMut<'storage, T>;

    unsafe fn get_param<'storage, 'state>(
        lookup_id: &'state Self::State,
        storage: UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state> {
        // SAFETY: lookup_id is the config ID validated by init_state and validate.
        // UnsafeStorageCell provides exclusive mutable access to storage under Param protocol.
        ConfigMut::from(unsafe { storage.storage().get_raw_config_mut(*lookup_id) })
    }

    // `ConfigMut` is only available if the underlying datum is not locked.
    fn validate(state: &Self::State, storage: UnsafeStorageCell) -> bool {
        // SAFETY: accesses are correctly registered with storage, no conflicts
        !unsafe { storage.storage() }.get_raw_config(*state).is_locked()
    }

    fn init_state(storage: &mut Storage, meta: &mut MetaData) -> Self::State {
        let id = storage.add_config_default_if_not_present::<T>();
        // All config is locked by default. We only unlock it (like below) when a component is detected that needs
        // it to be mutable.
        storage.unlock_config(id);

        debug_assert!(
            !meta.access().has_writes_all_configs(),
            "ConfigMut<{0}> in component {1} conflicts with a previous &mut Storage access.",
            core::any::type_name::<T>(),
            meta.name(),
        );
        debug_assert!(
            !meta.access().has_reads_all_configs(),
            "ConfigMut<{0}> in component {1} conflicts with a previous &Storage access.",
            core::any::type_name::<T>(),
            meta.name(),
        );
        debug_assert!(
            !meta.access().has_config_write(id),
            "ConfigMut<{0}> in component {1} conflicts with a previous ConfigMut<{0}> access.",
            core::any::type_name::<T>(),
            meta.name(),
        );
        debug_assert!(
            !meta.access().has_config_read(id),
            "ConfigMut<{0}> in component {1} conflicts with a previous Config<{0}> access.",
            core::any::type_name::<T>(),
            meta.name(),
        );

        meta.access_mut().add_config_write(id);
        id
    }
}

/// A Command queue to apply structural changes to [Storage] sometime after component execution has completed.
///
/// Allows for a non-conflicting way to manipulate [Storage] while also accessing parameters that would be in conflict
/// with [Storage] access, such as [Config] and [ConfigMut] by deferring structural manipulation of [Storage] until
/// sometime after the component has executed.
///
/// **Prefer using this over using [Storage] directly in a component.**
///
/// As an example, a component with the interface ``fn(&mut Storage, Config<i32>) -> Result<()>`` would
/// normally be in conflict, as it allows for the usage of [Storage::add_config]`, which could invalidate the requested
/// ``Config<i32>`` parameter.
pub struct Commands<'storage> {
    queue: &'storage mut Deferred,
}

impl Commands<'_> {
    /// Adds a config to storage sometime after the component has been executed.
    pub fn add_config<C: Default + 'static>(&mut self, config: C) {
        self.queue.add_command(move |storage| {
            storage.add_config(config);
        });
    }

    /// Adds a service to storage sometime after the component has been executed.
    pub fn add_service<S: IntoService + 'static>(&mut self, service: S) {
        self.queue.add_command(move |storage| {
            storage.add_service(service);
        });
    }

    /// Creates an instance of Commands that will never apply any commands to the storage.
    ///
    /// This function is intended for testing purposes only. Dropping the returned value will cause a memory leak as
    /// the underlying (leaked) Vec cannot be deallocated.
    ///
    /// ## Example
    /// ``` rust
    /// use patina::component::params::Commands;
    ///
    /// fn my_component_to_test(mut commands: Commands) {
    ///     commands.add_config(42);
    /// }
    ///
    /// #[test]
    /// fn test_my_component() {
    ///     my_component_to_test(Commands::mock());
    /// }
    /// ```
    #[allow(clippy::test_attr_in_doctest)]
    pub fn mock() -> Self {
        Commands { queue: Box::leak(Box::new(Deferred::default())) }
    }

    /// Returns if the queue is empty.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

// SAFETY: Commands parameter provides access to the deferred command queue.
// Deferred queue access is tracked in MetaData to prevent conflicts.
unsafe impl Param for Commands<'_> {
    type State = ();
    type Item<'storage, 'state> = Commands<'storage>;

    /// SAFETY: Deferred access is properly registered with the component's metadata.
    unsafe fn get_param<'storage, 'state>(
        _state: &'state Self::State,
        storage: UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state> {
        // SAFETY: Deferred queue access is properly registered with the component's metadata
        // via init_state. UnsafeStorageCell provides exclusive mutable access to storage.
        Commands { queue: unsafe { storage.storage_mut().deferred() } }
    }

    fn validate(_state: &Self::State, _storage: UnsafeStorageCell) -> bool {
        true
    }

    fn init_state(_storage: &mut Storage, meta: &mut MetaData) -> Self::State {
        debug_assert!(
            !meta.access().has_deferred(),
            "Commands in component {0} conflicts with a previous Commands access.",
            meta.name(),
        );
        meta.access_mut().deferred();
    }
}

// SAFETY: StandardBootServices parameter provides access to boot services.
// Access is validated and no mutation tracking needed for immutable service access.
unsafe impl Param for StandardBootServices {
    type State = ();
    type Item<'storage, 'state> = Self;

    unsafe fn get_param<'state>(
        _state: &'state Self::State,
        storage: UnsafeStorageCell<'_>,
    ) -> Self::Item<'static, 'state> {
        // SAFETY: Boot services are immutably borrowed from storage.
        // Clone creates a new service handle without violating borrowing rules.
        StandardBootServices::clone(unsafe { storage.storage().boot_services() })
    }

    fn validate(_state: &Self::State, storage: UnsafeStorageCell) -> bool {
        // Safety: Storage access is valid - UnsafeStorageCell ensures proper synchronization.
        unsafe { storage.storage() }.boot_services().is_init()
    }

    fn init_state(_storage: &mut Storage, _meta: &mut MetaData) -> Self::State {}
}

// SAFETY: StandardRuntimeServices parameter provides access to runtime services.
// Access is validated and no mutation tracking needed for immutable service access.
unsafe impl Param for StandardRuntimeServices {
    type State = ();
    type Item<'storage, 'state> = Self;

    unsafe fn get_param<'state>(
        _state: &'state Self::State,
        storage: UnsafeStorageCell<'_>,
    ) -> Self::Item<'static, 'state> {
        // SAFETY: Runtime services are immutably borrowed from storage.
        // Clone creates a new service handle without violating borrowing rules.
        StandardRuntimeServices::clone(unsafe { storage.storage().runtime_services() })
    }

    fn validate(_state: &Self::State, storage: UnsafeStorageCell) -> bool {
        // Safety: Storage access is valid - UnsafeStorageCell ensures proper synchronization.
        unsafe { storage.storage() }.runtime_services().is_init()
    }

    fn init_state(_storage: &mut Storage, _meta: &mut MetaData) -> Self::State {}
}

macro_rules! impl_component_param_tuple {
    ($($param: ident), *) => {
        #[allow(non_snake_case)]
        #[allow(clippy::unused_unit)]
        // SAFETY: Tuple parameter delegates to each component parameter's Param impl.
        // Each parameter's safety guarantees are preserved through delegation.
        unsafe impl<$($param: Param),*> Param for ($($param,)*) {
            type State = ($($param::State,)*);
            type Item<'storage, 'state> = ($($param::Item::<'storage, 'state>,)*);

            unsafe fn get_param<'storage, 'state>(state: &'state  Self::State, _storage: UnsafeStorageCell<'storage>) -> Self::Item<'storage, 'state> {
                let ($($param,)*) = state;
                #[allow(unused_unsafe)]
                ($(
                    // SAFETY: Each parameter's get_param is called with its validated state.
                    // Caller ensures state is validated and storage access is exclusive.
                    unsafe { $param::get_param($param, _storage) },
                )*)
            }

            fn try_validate(state: &Self::State, _storage: UnsafeStorageCell) -> Result<(), &'static str> {
                let ($($param,)*) = state;
                $(
                    if !$param::validate($param, _storage) {
                        return Err(core::any::type_name::<$param>());
                    }
                )*
                Ok(())
            }

            // This function is not used as we are overwriting the try_validate to call the individual param validate function
            // instead of this one.
            fn validate(_state: &Self::State, _storage: UnsafeStorageCell) -> bool {
                true
            }

            fn init_state(_storage: &mut Storage, _meta: &mut MetaData) -> Self::State {
                (($($param::init_state(_storage, _meta),)*))
            }
        }
    }
}

impl_component_param_tuple!();
impl_component_param_tuple!(T1);
impl_component_param_tuple!(T1, T2);
impl_component_param_tuple!(T1, T2, T3);
impl_component_param_tuple!(T1, T2, T3, T4);
impl_component_param_tuple!(T1, T2, T3, T4, T5);
impl_component_param_tuple!(T1, T2, T3, T4, T5, T6);

#[cfg(test)]
#[coverage(off)]
mod tests {
    use core::sync::atomic::AtomicBool;

    use crate::{
        component::{IntoComponent, storage::Storage},
        error::Result,
    };

    use crate as patina;

    use super::*;

    #[test]
    #[should_panic(
        expected = "ConfigMut<usize> in component patina::component::params::tests::test_two_mutable_config_access_to_same_type_fails::TestComponent conflicts with a previous ConfigMut<usize> access."
    )]
    fn test_two_mutable_config_access_to_same_type_fails() {
        #[derive(IntoComponent)]
        struct TestComponent;
        impl TestComponent {
            fn entry_point(self, _config: ConfigMut<usize>, _config2: ConfigMut<usize>) -> Result<()> {
                todo!()
            }
        }

        let mut storage = Storage::new();

        let mut component = TestComponent.into_component();
        component.initialize(&mut storage);
    }

    #[test]
    #[should_panic(
        expected = "Config<usize> in component patina::component::params::tests::test_mutable_and_immutable_config_access_to_same_type_fails1::TestComponent conflicts with a previous ConfigMut<usize> access."
    )]
    fn test_mutable_and_immutable_config_access_to_same_type_fails1() {
        #[derive(IntoComponent)]
        struct TestComponent;
        impl TestComponent {
            fn entry_point(self, _config: ConfigMut<usize>, _config2: Config<usize>) -> Result<()> {
                todo!()
            }
        }

        let mut storage = Storage::new();

        let mut component = TestComponent.into_component();
        component.initialize(&mut storage);
    }

    #[test]
    #[should_panic(
        expected = "ConfigMut<usize> in component patina::component::params::tests::test_mutable_and_immutable_config_access_to_same_type_fails2::TestComponent conflicts with a previous Config<usize> access."
    )]
    fn test_mutable_and_immutable_config_access_to_same_type_fails2() {
        #[derive(IntoComponent)]
        struct TestComponent;
        impl TestComponent {
            fn entry_point(self, _config: Config<usize>, _config2: ConfigMut<usize>) -> Result<()> {
                todo!()
            }
        }

        let mut storage = Storage::new();

        let mut component = TestComponent.into_component();
        component.initialize(&mut storage);
    }

    #[test]
    #[should_panic(
        expected = "Config<usize> in component patina::component::params::tests::test_mutable_storage_and_immutable_config_fail::TestComponent conflicts with a previous &mut Storage access."
    )]
    fn test_mutable_storage_and_immutable_config_fail() {
        #[derive(IntoComponent)]
        struct TestComponent;
        impl TestComponent {
            fn entry_point(self, _storage: &mut Storage, _config: Config<usize>) -> Result<()> {
                todo!()
            }
        }

        let mut component = TestComponent.into_component();
        component.initialize(&mut Storage::new());
    }

    #[test]
    #[should_panic(
        expected = "ConfigMut<usize> in component patina::component::params::tests::test_mutable_storage_and_mutable_config_fail::TestComponent conflicts with a previous &mut Storage access."
    )]
    fn test_mutable_storage_and_mutable_config_fail() {
        #[derive(IntoComponent)]
        struct TestComponent;
        impl TestComponent {
            fn entry_point(self, _storage: &mut Storage, _config: ConfigMut<usize>) -> Result<()> {
                todo!()
            }
        }

        let mut component = TestComponent.into_component();
        component.initialize(&mut Storage::new());
    }

    #[test]
    #[should_panic(
        expected = "&mut Storage in component patina::component::params::tests::test_config_and_mutable_storage_fail::TestComponent conflicts with a previous Config<T> access."
    )]
    fn test_config_and_mutable_storage_fail() {
        #[derive(IntoComponent)]
        struct TestComponent;
        impl TestComponent {
            fn entry_point(self, _config: Config<usize>, _storage: &mut Storage) -> Result<()> {
                todo!()
            }
        }

        let mut component = TestComponent.into_component();
        component.initialize(&mut Storage::new());
    }

    #[test]
    #[should_panic(
        expected = "&mut Storage in component patina::component::params::tests::test_mutable_config_and_mutable_storage_fail::TestComponent conflicts with a previous ConfigMut<T> access."
    )]
    fn test_mutable_config_and_mutable_storage_fail() {
        #[derive(IntoComponent)]
        struct TestComponent;
        impl TestComponent {
            fn entry_point(self, _config: ConfigMut<usize>, _storage: &mut Storage) -> Result<()> {
                todo!()
            }
        }

        let mut component = TestComponent.into_component();
        component.initialize(&mut Storage::new());
    }

    #[test]
    fn test_config_mut_deref_sticks_outside_fn() {
        fn my_fn(mut cfg: ConfigMut<i32>) {
            // DerefMut
            *cfg += 1;
        }

        let inner_data: RefCell<ConfigRaw> = RefCell::new(ConfigRaw::new(false, Box::new(42)));
        let config = ConfigMut::from(inner_data.borrow_mut());
        my_fn(config);

        // Deref
        let config = ConfigMut { value: inner_data.borrow_mut(), _marker: PhantomData::<i32> };
        assert_eq!(*config, 43);
    }

    #[test]
    fn test_config_mut_deref_sticks_inside_fn() {
        fn my_fn(mut cfg: ConfigMut<i32>) {
            // DerefMut
            *cfg += 1;
            assert_eq!(43, *cfg);
        }

        let cfg = ConfigMut::mock(42);
        my_fn(cfg);
    }

    #[test]
    fn test_config_deref() {
        fn my_fn(cfg: Config<i32>) {
            assert_eq!(*cfg, 42);
        }

        let config = Config::mock(42);

        my_fn(config);
    }

    #[test]
    fn test_config_can_be_accessed_while_unlocked() {
        let mut storage = Storage::new();
        let mut mock_metadata = MetaData::new::<i32>();

        let id = Config::<i32>::init_state(&mut storage, &mut mock_metadata);

        assert!(Config::<i32>::try_validate(&id, (&storage).into()).is_ok());

        let cell_storage = UnsafeStorageCell::new_mutable(&mut storage);
        // SAFETY: Test code - Config parameter is available in storage.
        assert_eq!(0_i32, unsafe { *Config::<i32>::get_param(&id, cell_storage) });
    }

    #[test]
    fn test_config_cannot_be_accessed_while_unlocked() {
        let mut storage = Storage::new();
        let mut mock_metadata = MetaData::new::<i32>();

        // ConfigMut will keep config unlocked
        let id = ConfigMut::<i32>::init_state(&mut storage, &mut mock_metadata);

        // Trying to access it with config, validation should fail because it is unlocked.
        assert!(
            Config::<i32>::try_validate(&id, (&storage).into())
                .is_err_and(|err| err == "patina::component::params::Config<'_, i32>")
        );
    }

    #[test]
    fn test_config_mut_cannot_be_accessed_while_locked() {
        let mut storage = Storage::new();
        let mut mock_metadata = MetaData::new::<i32>();

        let id = Config::<i32>::init_state(&mut storage, &mut mock_metadata);
        assert!(
            ConfigMut::<i32>::try_validate(&id, (&storage).into())
                .is_err_and(|err| err == "patina::component::params::ConfigMut<'_, i32>")
        );
    }

    #[test]
    fn test_config_mut_can_always_be_retrieved_while_unlocked() {
        let mut storage = Storage::new();
        let mut mock_metadata = MetaData::new::<i32>();

        let id = ConfigMut::<i32>::init_state(&mut storage, &mut mock_metadata);

        assert!(ConfigMut::<i32>::try_validate(&id, (&storage).into()).is_ok());

        let cell_storage = UnsafeStorageCell::new_mutable(&mut storage);
        // SAFETY: Test code - ConfigMut parameter is available in storage.
        assert_eq!(0_i32, unsafe { *ConfigMut::<i32>::get_param(&id, cell_storage) });
    }

    #[test]
    fn test_config_mut_lock_fn_prevents_future_config_mut_access() {
        let mut storage = Storage::new();
        let mut mock_metadata = MetaData::new::<i32>();

        let id = ConfigMut::<i32>::init_state(&mut storage, &mut mock_metadata);

        assert!(ConfigMut::<i32>::try_validate(&id, (&storage).into()).is_ok());

        storage.get_config_mut::<i32>().unwrap().lock();

        assert!(ConfigMut::<i32>::try_validate(&id, (&storage).into()).is_err());
    }

    #[test]
    #[should_panic(expected = "ConfigMut<i32> in component i32 conflicts with a previous &Storage access.")]
    fn test_config_mut_and_storage_cannot_be_requested_in_same_function() {
        let mut storage = Storage::new();

        // Mock metadata for the param function. This gets updated as you init each param.
        // The i32 will be the component name. Typically this is the function signature.
        let mut mock_metadata = MetaData::new::<i32>();

        <&Storage as Param>::init_state(&mut storage, &mut mock_metadata);

        ConfigMut::<i32>::init_state(&mut storage, &mut mock_metadata); // panic here
    }

    #[test]
    fn test_storage_can_always_be_retrieved() {
        let mut storage = Storage::new();
        let mut mock_metadata = MetaData::new::<i32>();

        <&Storage as Param>::init_state(&mut storage, &mut mock_metadata);

        assert!(<&Storage as Param>::try_validate(&(), (&storage).into()).is_ok());

        let cell_storage = UnsafeStorageCell::new_mutable(&mut storage);
        // SAFETY: Test code - Storage parameter has been validated.
        // does not panic
        let _ = unsafe { <&Storage as Param>::get_param(&(), cell_storage) };
    }

    #[test]
    fn test_storage_mut_can_always_be_retrieved() {
        let mut storage = Storage::new();
        let mut mock_metadata = MetaData::new::<i32>();

        <&mut Storage as Param>::init_state(&mut storage, &mut mock_metadata);

        assert!(<&mut Storage as Param>::try_validate(&(), (&storage).into()).is_ok());

        let cell_storage = UnsafeStorageCell::new_mutable(&mut storage);
        // SAFETY: Test code - Storage parameter has been validated.
        // does not panic
        let _ = unsafe { <&mut Storage as Param>::get_param(&(), cell_storage) };
    }

    #[test]
    fn test_boot_services_fails_to_validate_when_null() {
        let mut storage = Storage::default(); // boot_services is an empty pointer
        let mut mock_metadata = MetaData::new::<i32>();

        <StandardBootServices as Param>::init_state(&mut storage, &mut mock_metadata);
        assert_eq!(
            Err("patina::boot_services::StandardBootServices"),
            <StandardBootServices as Param>::try_validate(&(), (&storage).into())
        );
    }

    #[test]
    fn test_boot_services_can_be_retrieved() {
        let mut storage = Storage::default();
        let mut mock_metadata = MetaData::new::<i32>();

        // OOF, this is bad. But I don't wan't to write dummy functions for all the boot service functions. So we do this
        // instead, so that the pointer to the boot services table is not null.
        #[allow(invalid_value)]
        let efi_bs = core::mem::MaybeUninit::<r_efi::efi::BootServices>::zeroed();

        // SAFETY: Test code - Creating StandardBootServices from a zeroed BootServices struct for testing.
        // This is acceptable in test code as we're only checking parameter validation logic.
        let bs = unsafe { StandardBootServices::new(&*efi_bs.as_ptr()) };

        storage.set_boot_services(bs);

        <StandardBootServices as Param>::init_state(&mut storage, &mut mock_metadata);
        assert!(<StandardBootServices as Param>::try_validate(&(), (&storage).into()).is_ok());

        let cell_storage = UnsafeStorageCell::new_mutable(&mut storage);
        // SAFETY: Test code - StandardBootServices parameter has been validated.
        // does not panic
        let _ = unsafe { <StandardBootServices as Param>::get_param(&(), cell_storage) };
    }

    #[test]
    fn test_runtime_services_fails_to_validate_when_null() {
        let mut storage = Storage::default(); // runtime_services is an empty pointer
        let mut mock_metadata = MetaData::new::<i32>();

        <StandardRuntimeServices as Param>::init_state(&mut storage, &mut mock_metadata);
        assert_eq!(
            Err("patina::runtime_services::StandardRuntimeServices"),
            <StandardRuntimeServices as Param>::try_validate(&(), (&storage).into())
        );
    }

    #[test]
    fn test_runtime_services_can_be_retrieved() {
        let mut storage = Storage::default();
        let mut mock_metadata = MetaData::new::<i32>();

        // OOF, this is bad. But I don't wan't to write dummy functions for all the boot service functions. So we do this
        // instead, so that the pointer to the boot services table is not null.
        #[allow(invalid_value)]
        let efi_rt = core::mem::MaybeUninit::<r_efi::efi::RuntimeServices>::zeroed();

        // SAFETY: Test code - Creating StandardRuntimeServices from a zeroed RuntimeServices struct for testing.
        // This is acceptable in test code as we're only checking parameter validation logic.
        let rt = unsafe { StandardRuntimeServices::new(&*efi_rt.as_ptr()) };

        storage.set_runtime_services(rt);

        <StandardRuntimeServices as Param>::init_state(&mut storage, &mut mock_metadata);
        assert!(<StandardRuntimeServices as Param>::try_validate(&(), (&storage).into()).is_ok());

        let cell_storage = UnsafeStorageCell::new_mutable(&mut storage);
        // SAFETY: Test code - StandardRuntimeServices parameter has been validated.
        // does not panic
        let _ = unsafe { <StandardRuntimeServices as Param>::get_param(&(), cell_storage) };
    }

    #[test]
    fn test_option_returns_none_when_underlying_param_is_unavailable() {
        let mut storage = Storage::default();
        let mut mock_meadata = MetaData::new::<i32>();

        <Option<StandardBootServices> as Param>::init_state(&mut storage, &mut mock_meadata);
        assert!(<Option<StandardBootServices> as Param>::try_validate(&(), (&storage).into()).is_ok());
        // SAFETY: Test code - Option<StandardBootServices> parameter has been validated.
        assert!(unsafe { <Option<StandardBootServices> as Param>::get_param(&(), (&storage).into()).is_none() });
    }

    #[test]
    fn test_option_returns_underlying_param() {
        let mut storage = Storage::default();
        let mut mock_metadata = MetaData::new::<i32>();
        storage.add_config(42u32);

        let state = <Option<Config<u32>> as Param>::init_state(&mut storage, &mut mock_metadata);
        assert!(<Option<Config<u32>> as Param>::try_validate(&state, (&storage).into()).is_ok());
        // SAFETY: Test code - Option<Config<u32>> parameter has been validated.
        assert!(unsafe {
            <Option<Config<u32>> as Param>::get_param(&state, (&storage).into()).is_some_and(|v| *v == 42)
        });
    }

    #[test]
    fn test_try_validate_on_tuple_returns_underlying_param_type_not_full_tuple_name() {
        let mut storage = Storage::default();
        let mut mock_meadata = MetaData::new::<i32>();
        <(StandardBootServices, Config<i32>) as Param>::init_state(&mut storage, &mut mock_meadata);
        // This will always return true, because this function is not used with tuples. The tuple implementations
        // override the next level up, `try_validate`.
        assert!(<(StandardBootServices, Config<i32>) as Param>::validate(&((), 0), (&storage).into()));
        assert_eq!(
            Err("patina::boot_services::StandardBootServices"),
            <(StandardBootServices, Config<i32>) as Param>::try_validate(&((), 1), (&storage).into())
        );
    }

    #[test]
    fn test_get_commands() {
        let mut storage = Storage::default();
        let mut mock_metadata = MetaData::new::<i32>();

        {
            <Commands as Param>::init_state(&mut storage, &mut mock_metadata);
            assert!(<Commands as Param>::try_validate(&(), (&storage).into()).is_ok());

            let cell_storage = UnsafeStorageCell::new_mutable(&mut storage);
            // SAFETY: Test code - Commands parameter has been validated.
            let mut commands = unsafe { <Commands as Param>::get_param(&(), cell_storage) };
            assert!(commands.is_empty());
            commands.add_config(42i32);
        }

        let cell_storage = UnsafeStorageCell::new_mutable(&mut storage);
        // SAFETY: Test code - Commands parameter has been validated and storage is mutable.
        let commands = unsafe { <Commands as Param>::get_param(&(), cell_storage) };
        assert!(!commands.is_empty());
    }

    #[test]
    fn test_deferred_commands_are_applied() {
        trait TestService {
            #[allow(dead_code)]
            fn test(self);
        }

        #[derive(IntoService)]
        #[service(dyn TestService)]
        struct TestServiceImpl;
        impl TestService for TestServiceImpl {
            #[allow(dead_code)]
            fn test(self) {
                // do nothing
            }
        }

        #[derive(IntoComponent)]
        struct TestComponent;
        impl TestComponent {
            fn entry_point(self, mut cmds: Commands) -> Result<()> {
                cmds.add_config(42i32);
                cmds.add_service(TestServiceImpl);
                Ok(())
            }
        }

        let mut storage = Storage::new();

        let mut component = TestComponent.into_component();
        component.initialize(&mut storage);
        assert!(storage.get_config::<i32>().is_none());
        assert_eq!(component.run(&mut storage), Ok(true));

        assert!(storage.get_config::<i32>().is_none());
        assert!(storage.get_service::<dyn TestService>().is_none());

        storage.apply_deferred();
        assert_eq!(*(storage.get_config::<i32>().unwrap()), 42);
        assert!(storage.get_service::<dyn TestService>().is_some());
    }

    #[test]
    /// Ensure the common story of "Create service from Config" works
    fn test_deferred_and_config_compatability() {
        #[derive(IntoComponent)]
        struct TestComponent;
        impl TestComponent {
            fn entry_point(self, _cmds: Commands, _config: Config<i32>, _config2: ConfigMut<u32>) -> Result<()> {
                Ok(())
            }
        }

        let mut storage = Storage::new();

        let mut component = TestComponent.into_component();
        component.initialize(&mut storage);
    }

    #[test]
    #[should_panic(
        expected = "Commands in component patina::component::params::tests::test_cannot_have_two_commands_in_same_function::TestComponent conflicts with a previous Commands access."
    )]
    fn test_cannot_have_two_commands_in_same_function() {
        #[derive(IntoComponent)]
        struct TestComponent;
        impl TestComponent {
            fn entry_point(self, _cmds: Commands, _cmds2: Commands) -> Result<()> {
                Ok(())
            }
        }

        let mut storage = Storage::new();
        let mut component = TestComponent.into_component();
        component.initialize(&mut storage);
    }

    #[test]
    fn test_param_function_consume_self_runs_successfully() {
        static DID_RUN: AtomicBool = AtomicBool::new(false);

        #[derive(IntoComponent)]
        struct TestComponent;
        impl TestComponent {
            fn entry_point(self) -> Result<()> {
                DID_RUN.store(true, core::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
        }

        let mut storage = Storage::new();

        let mut component = TestComponent.into_component();
        component.initialize(&mut storage);
        assert_eq!(component.run(&mut storage), Ok(true));
        assert!(DID_RUN.load(core::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn test_param_function_consume_ref_self_runs_successfully() {
        static DID_RUN: AtomicBool = AtomicBool::new(false);

        #[derive(IntoComponent)]
        struct TestComponent;
        impl TestComponent {
            fn entry_point(&self) -> Result<()> {
                DID_RUN.store(true, core::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
        }

        let mut storage = Storage::new();

        let mut component = TestComponent.into_component();
        component.initialize(&mut storage);
        assert_eq!(component.run(&mut storage), Ok(true));
        assert!(DID_RUN.load(core::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn test_param_function_consume_mut_ref_self_runs_successfully() {
        static DID_RUN: AtomicBool = AtomicBool::new(false);

        #[derive(IntoComponent)]
        struct TestComponent;
        impl TestComponent {
            fn entry_point(&mut self) -> Result<()> {
                DID_RUN.store(true, core::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
        }

        let mut storage = Storage::new();

        let mut component = TestComponent.into_component();
        component.initialize(&mut storage);
        assert_eq!(component.run(&mut storage), Ok(true));
        assert!(DID_RUN.load(core::sync::atomic::Ordering::SeqCst));
    }
}
