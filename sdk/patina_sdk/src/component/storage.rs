//! A storage container for all datums that can be retrieved via dependency injection.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
extern crate alloc;

use crate::{
    component::{metadata::MetaData, params::Param},
    runtime_services::StandardRuntimeServices,
};

use crate::boot_services::StandardBootServices;
use alloc::{boxed::Box, collections::BTreeMap, vec::Vec};
use core::{
    any::{Any, TypeId},
    cell::{Ref, RefCell, RefMut, UnsafeCell},
    fmt::Debug,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    ptr,
};
use r_efi::efi::Guid;

use super::{
    hob::{FromHob, Hob},
    service::{IntoService, Service},
};

type HobParsers = BTreeMap<Guid, BTreeMap<TypeId, fn(&[u8], &mut Storage)>>;

/// A vector whose elements are sparsely populated.
#[derive(Debug)]
pub(crate) struct SparseVec<V> {
    values: Vec<Option<V>>,
}

impl<V> SparseVec<V> {
    /// Creates a new empty [SparseVec].
    pub const fn new() -> Self {
        Self { values: Vec::new() }
    }

    #[inline]
    /// Returns true if the [SparseVec] contains a value at the given index.
    pub fn contains(&self, index: usize) -> bool {
        self.values.get(index).map(|v| v.is_some()).unwrap_or(false)
    }

    #[inline]
    /// Returns the value at the given index, if it exists.
    pub fn get(&self, index: usize) -> Option<&V> {
        self.values.get(index).map(|v| v.as_ref())?
    }

    #[inline]
    pub fn get_mut(&mut self, index: usize) -> Option<&mut V> {
        self.values.get_mut(index).map(|v| v.as_mut())?
    }

    #[inline]
    /// Inserts a value at the given index.
    pub fn insert(&mut self, index: usize, value: V) {
        if index >= self.values.len() {
            self.values.resize_with(index + 1, || None);
        }
        self.values[index] = Some(value);
    }
}

impl<'a, V> IntoIterator for &'a SparseVec<V> {
    type Item = &'a Option<V>;
    type IntoIter = core::slice::Iter<'a, Option<V>>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

impl<'a, V> IntoIterator for &'a mut SparseVec<V> {
    type Item = &'a mut Option<V>;
    type IntoIter = core::slice::IterMut<'a, Option<V>>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.iter_mut()
    }
}

/// A container for an untyped config datum.
pub struct ConfigRaw(bool, Box<dyn Any>);

impl Debug for ConfigRaw {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ConfigRaw").field("is_locked", &self.0).field("raw_config", &self.1).finish()
    }
}

impl ConfigRaw {
    /// Creates a new [ConfigRaw] object.
    pub fn new(locked: bool, config: Box<dyn Any>) -> Self {
        Self(locked, config)
    }

    /// Locks the config, making it immutable.
    pub fn lock(&mut self) {
        self.0 = true;
    }

    /// Unlocks the config, making it mutable.
    pub(crate) fn unlock(&mut self) {
        self.0 = false;
    }

    /// Returns true if the config is locked.
    pub fn is_locked(&self) -> bool {
        self.0
    }
}

impl Deref for ConfigRaw {
    type Target = dyn Any;

    fn deref(&self) -> &Self::Target {
        self.1.as_ref()
    }
}

impl DerefMut for ConfigRaw {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.1.as_mut()
    }
}

/// A container for deferred commands that will be executed later.
#[derive(Default)]
#[allow(clippy::type_complexity)]
pub(crate) struct Deferred {
    queue: Vec<Box<dyn FnOnce(&mut Storage)>>,
}

impl Deferred {
    /// Adds a command to the deferred command queue.
    pub(crate) fn add_command<F: FnOnce(&mut Storage) + 'static>(&mut self, command: F) {
        self.queue.push(Box::new(command));
    }

    /// applies state changes to the storage via the deferred command queue.
    fn apply(&mut self, storage: &mut Storage) {
        for command in self.queue.drain(..) {
            command(storage);
        }
    }

    /// Returns if the deferred command queue is empty.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

impl Debug for Deferred {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Deferred").field("queue", &self.queue.len()).finish()
    }
}

/// Storage container for all datums that can be consumed by a Component.
///
/// The [Component](crate::component::Component) trait provides the interface that a component must implement to be
/// executed by a scheduler. By reviewing the [run](crate::component::Component::run) method, it can be seen that
/// the only data passed to a component during execution is the [Storage] object. This design point was chosen as it
/// eliminates breaking changes when implementing new [Params](crate::component::params::Param). Instead of changing
/// the `Component` trait interface to pass in new data when a new param is added, we can instead updating the
/// [Storage] object to either hold new data, or a reference to new data.
///
/// The expectation is that when a new `Param` is implemented for the SDK, any necessary changes can be made to the
/// [Storage] object, and the [Component](crate::component::Component) trait will not need to be updated. The
/// *breaking* change will occur in the DXE Core itself when consuming the update, it will have the responsibility of
/// providing the new data to the [Storage] object.
///
/// ## Storage as a Param
///
/// The [Storage] object itself implements the [Param] trait so that it can be provided to any component that requests
/// it. Directly accessing the [Storage] object in a component used to make changes to the underlying storage that
/// require exclusive access to the storage. Examples of this is inserting or removing data from storage, as this can
/// invalidate any references to data in the [Storage] object.
///
/// **Users should prefer the param [Commands](super::params::Commands) to make structural changes to the storage.**)
#[derive(Debug)]
pub struct Storage {
    /// A container for all deferred commands that components can register. This is used to delay the execution of
    /// commands that can result in structural changes to the storage.
    deferred: Option<Deferred>,
    /// A container for all [Config](super::params::Config) and [ConfigMut](super::params::ConfigMut) datums. This
    /// resource can be accessed both immutably and mutably, so it must be tracked by
    /// [Access](super::metadata::Access).
    configs: SparseVec<RefCell<ConfigRaw>>,
    /// A map to convert from a TypeId to a config index.
    config_indices: BTreeMap<TypeId, usize>,
    /// A container for all service datums. This resource can only be accessed immutably, but one service datum can
    /// represent multiple services. Services must have internal mutability if they need to be modified.
    services: SparseVec<&'static dyn Any>,
    /// A map to convert a Service type to a concrete service index.
    service_indices: BTreeMap<TypeId, usize>,
    /// HOB parsers for converting guided HOBs into `Hob<T>` datums.
    hob_parsers: HobParsers,
    /// A container for all [Hob](super::hob::Hob) datums.
    hobs: SparseVec<Vec<Box<dyn Any>>>,
    /// a map to convert from TypeId to a hob index.
    hob_indices: BTreeMap<TypeId, usize>,
    // Standard Boot Services.
    boot_services: StandardBootServices,
    // Standard Runtime Services.
    runtime_services: StandardRuntimeServices,
}

impl Default for Storage {
    fn default() -> Self {
        Self::new()
    }
}

impl Storage {
    /// Creates a new [Storage] object with empty containers for all datums.
    pub const fn new() -> Self {
        Self {
            deferred: None,
            configs: SparseVec::new(),
            config_indices: BTreeMap::new(),
            services: SparseVec::new(),
            service_indices: BTreeMap::new(),
            hob_parsers: BTreeMap::new(),
            hobs: SparseVec::new(),
            hob_indices: BTreeMap::new(),
            boot_services: StandardBootServices::new_uninit(),
            runtime_services: StandardRuntimeServices::new_uninit(),
        }
    }

    /// Applies all deferred actions to the storage. Used in a multi-threaded context
    pub(crate) fn apply_deferred(&mut self) {
        if let Some(mut deferred) = self.deferred.take() {
            deferred.apply(self);
        }
    }

    /// A queue for commands to be executed later.
    ///
    /// This is used to defer structural changes to the storage until a later time, to prevent scheduling conflicts
    /// in a multi-threaded environment.
    pub(crate) fn deferred(&mut self) -> &mut Deferred {
        if self.deferred.is_none() {
            self.deferred = Some(Deferred::default());
        }
        self.deferred.as_mut().unwrap()
    }

    /// Stores a pointer to the UEFI Boot Services Table.
    pub fn set_boot_services(&mut self, bs: StandardBootServices) {
        self.boot_services = bs;
    }

    /// Stores a pointer to the UEFI Runtime Services Table.
    pub fn set_runtime_services(&mut self, rs: StandardRuntimeServices) {
        self.runtime_services = rs;
    }

    /// Returns the UEFI Boot Services Table reference.
    pub fn boot_services(&self) -> &StandardBootServices {
        &self.boot_services
    }

    /// Returns the UEFI Runtime Services Table reference.
    pub fn runtime_services(&self) -> &StandardRuntimeServices {
        &self.runtime_services
    }

    /// Registers a config type with the storage and returns its global id.
    pub(crate) fn register_config<C: Default + 'static>(&mut self) -> usize {
        let idx = self.config_indices.len();
        *self.config_indices.entry(TypeId::of::<C>()).or_insert(idx)
    }

    /// Adds a default valued config datum to the storage if it does not exist.
    pub(crate) fn add_config_default_if_not_present<C: Default + 'static>(&mut self) -> usize {
        let idx = self.register_config::<C>();
        if !self.configs.contains(idx) {
            self.configs.insert(idx, RefCell::new(ConfigRaw::new(true, Box::<C>::default())));
        }
        idx
    }

    /// Adds a config datum to the storage, overwriting an existing value if it exists.
    pub fn add_config<C: Default + 'static>(&mut self, config: C) {
        let id = self.register_config::<C>();
        self.configs.insert(id, RefCell::new(ConfigRaw::new(true, Box::new(config))));
    }

    /// Attempts to retrieve a config datum from the storage.
    pub fn get_config<C: Default + 'static>(&self) -> Option<crate::component::params::Config<C>> {
        let id = self.config_indices.get(&TypeId::of::<C>())?;
        let untyped = self.get_raw_config(*id);
        Some(crate::component::params::Config::from(untyped))
    }

    /// Attempts to retrieve a mutable config datum from the storage.
    pub fn get_config_mut<C: Default + 'static>(&mut self) -> Option<crate::component::params::ConfigMut<C>> {
        let id = self.config_indices.get(&TypeId::of::<C>())?;
        let untyped = self.get_raw_config_mut(*id);
        Some(crate::component::params::ConfigMut::from(untyped))
    }

    /// Retrieves a config from the storage.
    pub(crate) fn get_raw_config(&self, id: usize) -> Ref<ConfigRaw> {
        self.configs
            .get(id)
            .unwrap_or_else(|| panic!("Could not find Config value when with id [{}] it should always exist.", id))
            .borrow()
    }

    /// Retrieves a mutable config from the storage.
    pub(crate) fn get_raw_config_mut(&self, id: usize) -> RefMut<ConfigRaw> {
        self.configs
            .get(id)
            .unwrap_or_else(|| panic!("Could not find Config value when with id [{}] it should always exist.", id))
            .borrow_mut()
    }

    /// Unlocks a config in the storage.
    pub(crate) fn unlock_config(&self, id: usize) {
        if let Some(config) = self.configs.get(id) {
            config.borrow_mut().unlock();
        }
    }

    /// Marks all configs present in the storage as locked (immutable).
    pub fn lock_configs(&self) {
        (&self.configs).into_iter().flatten().for_each(|config| config.borrow_mut().lock());
    }

    /// Registers a service type with the storage and returns its global id.
    pub(crate) fn register_service<C: ?Sized + 'static>(&mut self) -> usize {
        self.get_or_register_service(TypeId::of::<C>())
    }

    /// Gets the global id of a service, registering it if it does not exist.
    pub(crate) fn get_or_register_service(&mut self, id: TypeId) -> usize {
        let idx = self.service_indices.len();
        *self.service_indices.entry(id).or_insert(idx)
    }

    /// Inserts a service into the storage.
    pub(crate) fn insert_service(&mut self, id: usize, service: &'static dyn Any) {
        self.services.insert(id, service);
    }

    /// Adds a new service to the storage.
    pub fn add_service<S: IntoService + 'static>(&mut self, service: S) {
        service.register(self);
    }

    /// Retrieves a service from the underlying storage in its untyped form.
    pub(crate) fn get_raw_service(&self, id: usize) -> Option<&'static dyn Any> {
        // Copy the reference, not the underlying value
        self.services.get(id).copied()
    }

    /// Attempts to retrieve a service from the storage.
    pub fn get_service<S: ?Sized + 'static>(&self) -> Option<Service<S>> {
        let idx = *self.service_indices.get(&TypeId::of::<S>())?;
        Some(Service::from(self.get_raw_service(idx)?))
    }

    pub(crate) fn add_hob_parser<T: FromHob>(&mut self) {
        self.hob_parsers.entry(T::HOB_GUID).or_default().insert(TypeId::of::<T>(), T::register);
    }

    /// Registers a HOB with the storage and returns its global id.
    pub(crate) fn register_hob<T: FromHob>(&mut self) -> usize {
        self.get_or_register_hob(TypeId::of::<T>())
    }

    /// Gets the global id of a HOB, registering it if it does not exist.
    pub(crate) fn get_or_register_hob(&mut self, id: TypeId) -> usize {
        let idx = self.hob_indices.len();
        let idx = self.hob_indices.entry(id).or_insert(idx);
        if self.hobs.get(*idx).is_none() {
            self.hobs.insert(*idx, Vec::new());
        }
        *idx
    }

    /// Adds a HOB datum to the storage, overwriting an existing value if it exists.
    pub(crate) fn add_hob<H: FromHob>(&mut self, hob: H) {
        let id = self.register_hob::<H>(); // This creates the index if it does not exist.
        self.hobs.get_mut(id).expect("Hob Index should always exist.").push(Box::new(hob));
    }

    /// Retrieves the underlying HOB datum from the storage.
    pub(crate) fn get_raw_hob(&self, id: usize) -> &[Box<dyn Any>] {
        self.hobs
            .get(id)
            .unwrap_or_else(|| panic!("Could not find Hob value when with id [{}] it should always exist.", id))
    }

    /// Attempts to retrieve a HOB datum from the storage.
    pub fn get_hob<T: FromHob>(&self) -> Option<Hob<T>> {
        let id = self.hob_indices.get(&TypeId::of::<T>())?;
        self.hobs.get(*id).and_then(|hob| {
            if hob.is_empty() {
                return None;
            }
            Some(Hob::from(hob.as_slice()))
        })
    }

    /// Attempts to retrieve a HOB parser from the storage.
    pub fn get_hob_parsers(&self, guid: &Guid) -> Vec<fn(&[u8], &mut Storage)> {
        self.hob_parsers.get(guid).map(|type_map| type_map.values().copied().collect()).unwrap_or_default()
    }
}

/// A wrapper around a reference to a [Storage] object that allows for unsafe mutable
/// access to the storage.
///
/// ## Safety
///
/// - The caller must ensure that no simultaneous conflicting access to the storage occurs.
/// - The caller must ensure exclusive access to the storage when making structural changes.
#[derive(Copy, Clone)]
pub struct UnsafeStorageCell<'s>(*mut Storage, PhantomData<(&'s Storage, &'s UnsafeCell<Storage>)>);

unsafe impl Send for UnsafeStorageCell<'_> {}
unsafe impl Sync for UnsafeStorageCell<'_> {}

impl<'s> From<&'s mut Storage> for UnsafeStorageCell<'s> {
    fn from(storage: &'s mut Storage) -> Self {
        UnsafeStorageCell::new_mutable(storage)
    }
}

impl<'s> From<&'s Storage> for UnsafeStorageCell<'s> {
    fn from(storage: &'s Storage) -> Self {
        UnsafeStorageCell::new_readonly(storage)
    }
}

impl<'s> UnsafeStorageCell<'s> {
    /// Creates a [UnsafeStorageCell] that can be used to access everything immutably.
    #[inline]
    pub fn new_readonly(storage: &'s Storage) -> Self {
        Self(ptr::from_ref(storage).cast_mut(), PhantomData)
    }

    /// Creates a [UnsafeStorageCell] that can be used to access everything mutably.
    #[inline]
    pub fn new_mutable(storage: &'s mut Storage) -> Self {
        Self(ptr::from_mut(storage), PhantomData)
    }

    /// Gets a mutable reference to the [Storage] this [UnsafeStorageCell] wraps.
    ///
    /// This is an incredibly error-prone operation is only valid in a small number of
    /// circumstances.
    ///
    /// ## Safety
    ///
    /// - `self` must have been obtained from a call to [UnsafeStorageCell::new_mutable]
    ///   (*not* `as_unsafe_storage_cell_readonly` or any other method of contruction that does not
    ///   provide mutable access to the entire storage).
    ///   - This means that if you have an [UnsafeStorageCell] that you did not create yourself, it
    ///     is likely *unsound* to call this method.
    /// - The returned `&mut Storage` *must* by unique: it must never be allowed to exists at the
    ///   same time as any other borrows of the storage or any accesses to its data.
    ///   - `&mut Storage` *may* exist at the same time as instances of `UnsafeStorageCell`, so
    ///     long as none of those instances are used to access storage data in any way while the
    ///     mutable borrow is active.
    #[inline]
    pub unsafe fn storage_mut(self) -> &'s mut Storage {
        // Safety:
        // - caller ensures the created `&mut Storage` is the only borrow of the storage.
        unsafe { &mut *self.0 }
    }

    /// Gets a reference to the [Storage] this [UnsafeStorageCell] wraps.
    ///
    /// This can be used for arbitrary shared/readonly access.
    ///
    /// ## Safety
    ///
    /// - must have permission to access the whole storage immutably
    /// - there must be no live exclusive borrows on storage data
    /// - there must be no live exclusive borrow of storage
    #[inline]
    pub unsafe fn storage(self) -> &'s Storage {
        // Safety:
        // - caller ensures there is no `&mut Storage`
        // - caller ensures there is no mutable borrows of storage data. This means the caller
        //   cannot misuse the returned `&World`.
        unsafe { &*self.0 }
    }
}

unsafe impl Param for &mut Storage {
    type State = ();
    type Item<'storage, 'state> = &'storage mut Storage;

    unsafe fn get_param<'storage, 'state>(
        _state: &'state Self::State,
        storage: UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state> {
        unsafe { storage.storage_mut() }
    }

    fn validate(_state: &Self::State, _storage: UnsafeStorageCell) -> bool {
        // Always available
        true
    }

    fn init_state(_storage: &mut Storage, meta: &mut MetaData) -> Self::State {
        // Storage provides global access to configuration. That means by manipulating the storage,
        // we can invalidate any config access, so we make sure no other config access has been
        // registered, and set ourselves as exclusive.
        assert!(
            !meta.access().has_any_config_write(),
            "&mut Storage in system {} conflicts with a previous ConfigMut<T> access.",
            meta.name()
        );

        assert!(
            !meta.access().has_any_config_read(),
            "&mut Storage in system {} conflicts with a previous Config<T> access.",
            meta.name()
        );
        meta.access_mut().writes_all_configs();
    }
}

unsafe impl Param for &Storage {
    type State = ();
    type Item<'storage, 'state> = &'storage Storage;

    unsafe fn get_param<'storage, 'state>(
        _state: &'state Self::State,
        storage: UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state> {
        unsafe { storage.storage() }
    }

    fn validate(_state: &Self::State, _storage: UnsafeStorageCell) -> bool {
        // Always available
        true
    }

    fn init_state(_storage: &mut Storage, meta: &mut MetaData) -> Self::State {
        assert!(
            !meta.access().has_any_config_write(),
            "&mut Storage in system {} conflicts with a previous ConfigMut<T> access.",
            meta.name()
        );

        meta.access_mut().reads_all_configs();
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn validate_iter_works() {
        let mut v: SparseVec<u32> = SparseVec::new();

        v.insert(4, 100);
        v.insert(9, 200);
        v.insert(75, 300);

        assert_eq!(v.into_iter().filter(|o| o.is_some()).count(), 3);

        // Check that interior mutability sticks
        for v in &mut v {
            if let Some(v) = v.as_mut() {
                *v += 1;
            }
        }

        assert_eq!(v.get(4), Some(&101));
        assert_eq!(v.get(9), Some(&201));
        assert_eq!(v.get(75), Some(&301));

        // Check that exterior mutability sticks
        for v in &mut v {
            *v = None;
        }

        assert_eq!(v.into_iter().filter(|o| o.is_some()).count(), 0);
    }

    #[test]
    fn test_hob_functionality() {
        use crate as patina_sdk;
        #[derive(Copy, Clone, FromHob)]
        #[repr(C)]
        #[hob = "12345678-1234-1234-1234-123456789012"]
        struct MyStruct;

        let mut storage = Storage::new();

        storage.register_hob::<MyStruct>();
        assert!(storage.get_hob::<MyStruct>().is_none());
        assert!(storage.get_hob_parsers(&MyStruct::HOB_GUID).is_empty());

        storage.add_hob(MyStruct);
        assert!(storage.get_hob::<MyStruct>().is_some());
        assert_eq!(storage.get_hob::<MyStruct>().unwrap().iter().count(), 1);

        storage.add_hob(MyStruct);
        assert_eq!(storage.get_hob::<MyStruct>().unwrap().iter().count(), 2);

        storage.add_hob_parser::<MyStruct>();
        assert!(!storage.get_hob_parsers(&MyStruct::HOB_GUID).is_empty());
    }

    #[test]
    fn test_services_still_work_if_storage_requires_re_alloc() {
        use crate as patina_sdk;
        trait TestService {
            fn test(&self) -> usize;
        }

        #[derive(IntoService)]
        #[service(dyn TestService)]
        struct TestServiceImpl {
            id: usize,
        }

        impl TestService for TestServiceImpl {
            fn test(&self) -> usize {
                self.id
            }
        }

        let mut storage = Storage::new();

        storage.add_service(TestServiceImpl { id: 42 });
        let service = storage.get_service::<dyn TestService>().unwrap();

        assert_eq!(service.test(), 42);

        // Mimic all of storage's service storage being reallocated (due to possible vec reallocations when inserting)
        // by dropping the entire storage.
        drop(storage);

        // service should still work as it is pointing to the static leaked memory, not the reference in the storage.
        assert_eq!(service.test(), 42);
    }

    #[test]
    fn test_apply_deferred_storage() {
        use crate as patina_sdk;
        use patina_sdk::component::params::Commands;

        trait TestService {
            fn test(&self) -> usize;
        }

        #[derive(IntoService)]
        #[service(dyn TestService)]
        struct TestServiceImpl {
            id: usize,
        }

        impl TestService for TestServiceImpl {
            fn test(&self) -> usize {
                self.id
            }
        }

        let mut storage = Storage::new();

        assert!(storage.get_service::<dyn TestService>().is_none());

        {
            let mut commands = unsafe { <Commands as Param>::get_param(&(), UnsafeStorageCell::from(&mut storage)) };
            commands.add_service(TestServiceImpl { id: 42 });
        }

        assert!(storage.get_service::<dyn TestService>().is_none());

        storage.apply_deferred();
        assert!(storage.get_service::<dyn TestService>().is_some());
        let service = storage.get_service::<dyn TestService>().unwrap();
        assert_eq!(service.test(), 42);
    }
}
