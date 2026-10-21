//! A storage container for all datums that can be retrieved via dependency injection.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::{
    component::{
        metadata::MetaData,
        params::Param,
        query::Query,
        tables::{Fields, Tables},
    },
    uefi::runtime_services::StandardRuntimeServices,
};

use crate::BinaryGuid;
use alloc::{borrow::Cow, boxed::Box, vec::Vec};
use core::{
    any::{Any, TypeId},
    cell::{Ref, RefCell, RefMut, UnsafeCell},
    fmt::Debug,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    ptr,
};

use super::{
    hob::{FromHob, Hob},
    service::{IntoService, Service},
};

use super::tables::{Entity, EntityError};

/// A container for an untyped config datum.
pub struct ConfigRaw(bool, Box<dyn Any>);

impl Debug for ConfigRaw {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ConfigRaw").field("is_locked", &self.0).field("raw_config", &self.1).finish()
    }
}

impl ConfigRaw {
    /// Creates a new [`ConfigRaw`] object.
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

pub(crate) struct StoredConfig {
    id: TypeId,
    value: RefCell<ConfigRaw>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HobId(pub(crate) TypeId);

pub(crate) struct HobValue(pub(crate) Box<dyn Any>);

pub(crate) struct HobParser {
    guid: BinaryGuid,
    parser: fn(&[u8], &mut Storage),
}

/// One registered service interface, stored on its own entity.
pub(crate) struct StoredService {
    id: TypeId,
    value: &'static dyn Any,
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

/// Immediate structural access to one [`Entity`].
pub struct EntityMut<'storage> {
    entity: Entity,
    tables: &'storage mut Tables,
}

#[allow(private_bounds)]
impl EntityMut<'_> {
    /// Returns this entry's entity identity.
    pub fn id(&self) -> Entity {
        self.entity
    }

    /// Adds fields to this entity.
    ///
    /// # Errors
    ///
    /// Returns [`EntityError::FieldAlreadyExists`] if the entity already contains any field in `fields`.
    pub fn add_fields<F: Fields>(&mut self, fields: F) -> Result<&mut Self, EntityError> {
        self.tables.add_fields(self.entity, fields)?;
        Ok(self)
    }

    /// Removes and returns one field from this entity.
    ///
    /// Returns `None` if the entity no longer exists or does not contain `T`.
    pub fn take_field<T: 'static>(&mut self) -> Option<T> {
        self.tables.take_field::<T>(self.entity)
    }

    /// Removes this entity and drops all of its fields.
    pub fn remove(self) {
        self.tables.remove(self.entity).expect("EntityMut must reference a live entity");
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
    /// Type-erased, column-oriented data used by component infrastructure.
    tables: Tables,
    // Standard Runtime Services.
    runtime_services: StandardRuntimeServices,
    // Image handle for the DXE Core (used as parent for LoadImage calls).
    image_handle: Option<crate::standard::efi::Handle>,
}

impl Default for Storage {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(private_bounds)]
impl Storage {
    /// Creates a new [Storage] object with empty containers for all datums.
    pub const fn new() -> Self {
        Self {
            deferred: None,
            tables: Tables::new(),
            runtime_services: StandardRuntimeServices::new_uninit(),
            image_handle: None,
        }
    }

    pub(crate) const fn get_tables_mut(&mut self) -> &mut Tables {
        &mut self.tables
    }

    pub(crate) const fn get_tables(&self) -> &Tables {
        &self.tables
    }

    /// Returns immediate structural access to `entity`.
    ///
    /// Returns `None` if the entity no longer exists.
    pub fn entity_mut(&mut self, entity: Entity) -> Option<EntityMut<'_>> {
        self.tables.contains(entity).then_some(EntityMut { entity, tables: &mut self.tables })
    }

    /// Creates an entity with no fields.
    pub fn create_entity(&mut self) -> EntityMut<'_> {
        let entity = self.tables.insert_new(());
        EntityMut { entity, tables: &mut self.tables }
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

    /// Stores a pointer to the UEFI Runtime Services Table.
    pub fn set_runtime_services(&mut self, rs: StandardRuntimeServices) {
        self.runtime_services = rs;
    }

    /// Returns the UEFI Runtime Services Table reference.
    pub fn runtime_services(&self) -> &StandardRuntimeServices {
        &self.runtime_services
    }

    /// Sets the image handle for the DXE Core.
    ///
    /// This handle is used as the parent image handle for `LoadImage()` calls
    /// when loading boot applications.
    pub fn set_image_handle(&mut self, handle: crate::standard::efi::Handle) {
        self.image_handle = Some(handle);
    }

    /// Returns the image handle for the DXE Core, if set.
    pub fn image_handle(&self) -> Option<crate::standard::efi::Handle> {
        self.image_handle
    }

    /// Adds a default valued config datum to the storage if it does not exist.
    pub(crate) fn add_config_default_if_not_present<C: Default + 'static>(&mut self) -> Entity {
        let id = TypeId::of::<C>();
        if let Some(entity) = self.config_entity(id) {
            return entity;
        }

        self.insert_config(id, Box::<C>::default())
    }

    /// Adds a config datum to the storage, overwriting an existing value if it exists.
    pub fn add_config<C: Default + 'static>(&mut self, config: C) {
        self.add_config_erased(TypeId::of::<C>(), Box::new(config));
    }

    fn add_config_erased(&mut self, id: TypeId, config: Box<dyn Any>) {
        if let Some(entity) = self.config_entity(id) {
            Query::<&StoredConfig>::new()
                .query(self)
                .get(&entity)
                .expect("config entity must contain its config")
                .value
                .replace(ConfigRaw::new(true, config));
        } else {
            self.insert_config(id, config);
        }
    }

    fn insert_config(&mut self, id: TypeId, config: Box<dyn Any>) -> Entity {
        self.tables.insert_new((StoredConfig { id, value: RefCell::new(ConfigRaw::new(true, config)) },))
    }

    /// Attempts to retrieve a config datum from the storage.
    pub fn get_config<C: Default + 'static>(&self) -> Option<crate::component::params::Config<'_, C>> {
        let entity = self.config_entity(TypeId::of::<C>())?;
        Some(crate::component::params::Config::from(self.get_raw_config(entity)))
    }

    /// Attempts to retrieve a mutable config datum from the storage.
    pub fn get_config_mut<C: Default + 'static>(&mut self) -> Option<crate::component::params::ConfigMut<'_, C>> {
        let entity = self.config_entity(TypeId::of::<C>())?;
        Some(crate::component::params::ConfigMut::from(self.get_raw_config_mut(entity)))
    }

    fn config_entity(&self, id: TypeId) -> Option<Entity> {
        Query::<(Entity, &StoredConfig)>::new()
            .query(self)
            .find_map(|(entity, config)| (config.id == id).then_some(entity))
    }

    pub(crate) fn get_raw_config(&self, entity: Entity) -> Ref<'_, ConfigRaw> {
        Query::<&StoredConfig>::new()
            .query(self)
            .get(&entity)
            .expect("config entity must contain its config")
            .value
            .borrow()
    }

    pub(crate) fn get_raw_config_mut(&self, entity: Entity) -> RefMut<'_, ConfigRaw> {
        Query::<&StoredConfig>::new()
            .query(self)
            .get(&entity)
            .expect("config entity must contain its config")
            .value
            .borrow_mut()
    }

    pub(crate) fn config_is_locked(&self, entity: Entity) -> bool {
        self.get_raw_config(entity).is_locked()
    }

    /// Unlocks a config in the storage.
    pub(crate) fn unlock_config(&self, entity: Entity) {
        if let Some(config) = Query::<&StoredConfig>::new().query(self).get(&entity) {
            config.value.borrow_mut().unlock();
        }
    }

    /// Marks all configs present in the storage as locked (immutable).
    pub fn lock_configs(&self) {
        Query::<&StoredConfig>::new().query(self).for_each(|config| config.value.borrow_mut().lock());
    }

    /// Inserts a service into the storage.
    pub(crate) fn insert_service<S: ?Sized + 'static>(&mut self, service: &'static dyn Any) {
        self.insert_service_erased(TypeId::of::<S>(), service);
    }

    fn insert_service_erased(&mut self, id: TypeId, service: &'static dyn Any) {
        if self.find_service(id).is_some() {
            return;
        }

        self.tables.insert_new((StoredService { id, value: service },));
    }

    fn find_service(&self, id: TypeId) -> Option<&'static dyn Any> {
        Query::<&StoredService>::new().query(self).find_map(|service| (service.id == id).then_some(service.value))
    }

    /// Adds a new service to the storage.
    pub fn add_service<S: IntoService + 'static>(&mut self, service: S) {
        service.register(self);
    }

    /// Attempts to retrieve a service from the storage.
    pub fn get_service<S: ?Sized + 'static>(&self) -> Option<Service<S>> {
        Some(Service::from(self.find_service(TypeId::of::<S>())?))
    }

    pub(crate) fn add_hob_parser<T: FromHob>(&mut self) {
        let guid = T::HOB_GUID;
        let id = TypeId::of::<T>();
        if Query::<(&HobId, &HobParser)>::new()
            .query(self)
            .any(|(hob_id, parser)| hob_id.0 == id && parser.guid == guid)
        {
            return;
        }
        self.tables.insert_new((HobId(id), HobParser { guid, parser: T::register }));
    }

    /// Adds a HOB datum to the storage.
    pub(crate) fn add_hob<H: FromHob>(&mut self, hob: H) {
        self.add_hob_erased(TypeId::of::<H>(), Box::new(hob));
    }

    fn add_hob_erased(&mut self, id: TypeId, hob: Box<dyn Any>) {
        self.tables.insert_new((HobId(id), HobValue(hob)));
    }

    /// Attempts to retrieve a HOB datum from the storage.
    pub fn get_hob<T: FromHob>(&self) -> Option<Hob<'_, T>> {
        let id = TypeId::of::<T>();
        let hob: Hob<'_, T> = Hob::from((self, id, Query::<(&'static HobId, &'static HobValue)>::new()));
        hob.iter().next()?;
        Some(hob)
    }

    /// Attempts to retrieve a HOB parser from the storage.
    pub fn get_hob_parsers(&self, guid: &BinaryGuid) -> Vec<fn(&[u8], &mut Storage)> {
        Query::<&HobParser>::new()
            .query(self)
            .filter_map(|parser| (parser.guid == *guid).then_some(parser.parser))
            .collect()
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

// SAFETY: UnsafeStorageCell wraps a raw pointer but does not own the data. The lifetime specifier ensures the pointer
// remains valid. Send is safe because the wrapper doesn't allow unsynchronized access. All access goes through
// unsafe methods that require the caller to ensure proper synchronization. The PhantomData ties the lifetime
// to the storage reference, preventing the cell from outliving the storage.
unsafe impl Send for UnsafeStorageCell<'_> {}
// SAFETY: Sync is safe because UnsafeStorageCell provides controlled access to storage through unsafe methods
// that place the synchronization burden on the caller. The wrapper itself contains no mutable state - just a
// raw pointer and phantom data. Multiple threads can hold UnsafeStorageCell instances safely as long as they
// follow the access rules documented on storage() and storage_mut().
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
    /// Creates a [`UnsafeStorageCell`] that can be used to access everything immutably.
    #[inline]
    pub fn new_readonly(storage: &'s Storage) -> Self {
        Self(ptr::from_ref(storage).cast_mut(), PhantomData)
    }

    /// Creates a [`UnsafeStorageCell`] that can be used to access everything mutably.
    #[inline]
    pub fn new_mutable(storage: &'s mut Storage) -> Self {
        Self(ptr::from_mut(storage), PhantomData)
    }

    /// Gets a mutable reference to the [Storage] this [`UnsafeStorageCell`] wraps.
    ///
    /// This is an incredibly error-prone operation is only valid in a small number of
    /// circumstances.
    ///
    /// ## Safety
    ///
    /// - `self` must have been obtained from a call to [`UnsafeStorageCell::new_mutable`]
    ///   (*not* `as_unsafe_storage_cell_readonly` or any other method of contruction that does not
    ///   provide mutable access to the entire storage).
    ///   - This means that if you have an [`UnsafeStorageCell`] that you did not create yourself, it
    ///     is likely *unsound* to call this method.
    /// - The returned `&mut Storage` *must* by unique: it must never be allowed to exists at the
    ///   same time as any other borrows of the storage or any accesses to its data.
    ///   - `&mut Storage` *may* exist at the same time as instances of `UnsafeStorageCell`, so
    ///     long as none of those instances are used to access storage data in any way while the
    ///     mutable borrow is active.
    #[inline]
    pub unsafe fn storage_mut(self) -> &'s mut Storage {
        // SAFETY:
        // - caller ensures the created `&mut Storage` is the only borrow of the storage.
        unsafe { &mut *self.0 }
    }

    /// Gets a reference to the [Storage] this [`UnsafeStorageCell`] wraps.
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
        // SAFETY:
        // - caller ensures there is no `&mut Storage`
        // - caller ensures there is no mutable borrows of storage data. This means the caller
        //   cannot misuse the returned `&World`.
        unsafe { &*self.0 }
    }
}

// SAFETY: &mut Storage parameter provides exclusive access to the entire storage. The Param implementation
// ensures exclusive access by tracking config reads/writes in MetaData. init_state() asserts no conflicting
// config access exists and marks all configs as exclusively written. get_param() uses storage_mut() which
// requires the UnsafeStorageCell was created from mutable storage access.
unsafe impl Param for &mut Storage {
    type State = ();
    type Item<'storage, 'state> = &'storage mut Storage;

    // SAFETY: UnsafeStorageCell was created from &mut Storage (validated by init_state). storage_mut()
    // returns the underlying mutable reference. The Param trait ensures this is the only active borrow
    // of storage data.
    unsafe fn get_param<'storage, 'state>(
        _state: &'state Self::State,
        storage: UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state> {
        // SAFETY: UnsafeStorageCell was created with exclusive access. init_state ensured no conflicting
        // config access. storage_mut() safety requirements are met by the Param trait.
        unsafe { storage.storage_mut() }
    }

    fn validate(_state: &Self::State, _storage: UnsafeStorageCell) -> bool {
        // Always available
        true
    }

    fn init_state(_storage: &mut Storage, meta: &mut MetaData) -> Result<Self::State, Cow<'static, str>> {
        // Storage provides global access to configuration. That means by manipulating the storage,
        // we can invalidate any config access, so we make sure no other config access has been
        // registered, and set ourselves as exclusive.
        if meta.access().has_writes_all_configs() {
            return Err(Cow::from("&mut Storage conflicts with a previous &mut Storage access."));
        }

        if meta.access().has_reads_all_configs() {
            return Err(Cow::from("&mut Storage conflicts with a previous &Storage access."));
        }

        if meta.access().has_any_config_write() {
            return Err(Cow::from("&mut Storage conflicts with a previous ConfigMut<T> access."));
        }

        if meta.access().has_any_config_read() {
            return Err(Cow::from("&mut Storage conflicts with a previous Config<T> access."));
        }
        meta.access_mut().writes_all_configs();
        Ok(())
    }
}

// SAFETY: &Storage parameter provides shared immutable access to the entire storage. The Param implementation
// ensures no conflicting mutable access exists by checking that no ConfigMut<T> parameters have been registered
// (which would require mutable config access). get_param() uses storage() which only requires immutable access.
unsafe impl Param for &Storage {
    type State = ();
    type Item<'storage, 'state> = &'storage Storage;

    // SAFETY: UnsafeStorageCell provides access to storage. storage() returns an immutable reference.
    // init_state() ensured no conflicting mutable config access exists. The Param protocol ensures this
    // shared access is safe.
    unsafe fn get_param<'storage, 'state>(
        _state: &'state Self::State,
        storage: UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state> {
        // SAFETY: storage() requires no exclusive borrows of storage data. init_state verified no
        // ConfigMut<T> access exists that would create conflicting mutable access.
        unsafe { storage.storage() }
    }

    fn validate(_state: &Self::State, _storage: UnsafeStorageCell) -> bool {
        // Always available
        true
    }

    fn init_state(_storage: &mut Storage, meta: &mut MetaData) -> Result<Self::State, Cow<'static, str>> {
        if meta.access().has_writes_all_configs() {
            return Err(Cow::from("&Storage conflicts with a previous &mut Storage access."));
        }

        if meta.access().has_any_config_write() {
            return Err(Cow::from("&Storage conflicts with a previous ConfigMut<T> access."));
        }
        meta.access_mut().reads_all_configs();
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::component::Query;
    use alloc::vec;

    #[derive(Debug, PartialEq, Eq)]
    struct QueryA(u32);

    #[derive(Debug, PartialEq, Eq)]
    struct QueryB(u32);

    #[test]
    fn test_entity_adds_fields_before_querying() {
        let mut storage = Storage::new();
        let mut entity_mut = storage.create_entity();
        let entity = entity_mut.id();
        assert_eq!(entity_mut.id(), entity);
        entity_mut.add_fields((QueryA(1), QueryB(2))).unwrap();

        let rows =
            Query::<(&QueryA, &QueryB)>::new().query_mut(&mut storage).map(|(a, b)| (a.0, b.0)).collect::<Vec<_>>();
        assert_eq!(rows, vec![(1, 2)]);
    }

    #[test]
    fn test_entity_takes_and_restores_fields_one_at_a_time() {
        let mut storage = Storage::new();
        let entity = storage.create_entity().add_fields((QueryA(1), QueryB(2))).unwrap().id();

        {
            let mut entity_mut = storage.entity_mut(entity).unwrap();
            assert_eq!(entity_mut.take_field::<QueryA>(), Some(QueryA(1)));
            assert_eq!(entity_mut.take_field::<QueryB>(), Some(QueryB(2)));
        }
        assert!(Query::<(&QueryA, &QueryB)>::new().query_mut(&mut storage).get(&entity).is_none());
        storage.entity_mut(entity).unwrap().add_fields((QueryA(1), QueryB(3))).unwrap();
        assert_eq!(
            Query::<(&QueryA, &QueryB)>::new().query_mut(&mut storage).get(&entity),
            Some((&QueryA(1), &QueryB(3)))
        );
    }

    #[test]
    fn test_configs_use_separate_entities_and_overwrite_in_place() {
        #[derive(Default)]
        struct ConfigA(u32);
        #[derive(Default)]
        struct ConfigB;

        let mut storage = Storage::new();
        let a = storage.add_config_default_if_not_present::<ConfigA>();
        let b = storage.add_config_default_if_not_present::<ConfigB>();
        assert_ne!(a, b);

        storage.add_config(ConfigA(42));
        assert_eq!(storage.add_config_default_if_not_present::<ConfigA>(), a);
        assert_eq!(storage.get_config::<ConfigA>().unwrap().0, 42);
        assert_eq!(Query::<&StoredConfig>::new().query_mut(&mut storage).count(), 2);
    }

    #[test]
    fn test_hob_functionality() {
        use crate as patina;
        #[derive(FromHob, zerocopy_derive::FromBytes)]
        #[repr(C)]
        #[hob = "12345678-1234-1234-1234-123456789012"]
        struct MyStruct;

        let mut storage = Storage::new();

        assert!(storage.get_hob::<MyStruct>().is_none());
        assert!(storage.get_hob_parsers(&MyStruct::HOB_GUID).is_empty());

        storage.add_hob(MyStruct);
        assert!(storage.get_hob::<MyStruct>().is_some());
        assert_eq!(storage.get_hob::<MyStruct>().unwrap().iter().count(), 1);

        storage.add_hob(MyStruct);
        assert_eq!(storage.get_hob::<MyStruct>().unwrap().iter().count(), 2);
        let entities = Query::<(Entity, &HobId, &HobValue)>::new()
            .query_mut(&mut storage)
            .filter_map(|(entity, id, value)| {
                if id.0 == TypeId::of::<MyStruct>() {
                    assert!(value.0.downcast_ref::<MyStruct>().is_some());
                    Some(entity)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(entities.len(), 2);
        assert_ne!(entities[0], entities[1]);

        storage.add_hob_parser::<MyStruct>();
        assert!(!storage.get_hob_parsers(&MyStruct::HOB_GUID).is_empty());
    }

    #[test]
    fn test_services_still_work_if_storage_requires_re_alloc() {
        use crate as patina;
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
        storage.add_service(TestServiceImpl { id: 7 });
        assert_eq!(storage.get_service::<dyn TestService>().unwrap().test(), 42);

        // Mimic all of storage's service storage being reallocated (due to possible vec reallocations when inserting)
        // by dropping the entire storage.
        drop(storage);

        // service should still work as it is pointing to the static leaked memory, not the reference in the storage.
        assert_eq!(service.test(), 42);
    }

    #[test]
    fn test_apply_deferred_storage() {
        use crate as patina;
        use patina::component::params::Commands;

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
            // SAFETY: Test code - Commands parameter is always available without validation.
            let mut commands = unsafe { <Commands as Param>::get_param(&(), UnsafeStorageCell::from(&mut storage)) };
            commands.add_service(TestServiceImpl { id: 42 });
        }

        assert!(storage.get_service::<dyn TestService>().is_none());

        storage.apply_deferred();
        assert!(storage.get_service::<dyn TestService>().is_some());
        let service = storage.get_service::<dyn TestService>().unwrap();
        assert_eq!(service.test(), 42);
    }

    #[test]
    fn test_service_interfaces_have_separate_entities() {
        use crate as patina;

        trait TestService {
            fn value(&self) -> u32;
        }
        trait OtherService {
            fn value(&self) -> u32;
        }

        #[derive(IntoService)]
        #[service(dyn TestService, dyn OtherService)]
        struct TestServiceImpl(u32);

        impl TestService for TestServiceImpl {
            fn value(&self) -> u32 {
                self.0
            }
        }
        impl OtherService for TestServiceImpl {
            fn value(&self) -> u32 {
                self.0
            }
        }

        let mut storage = Storage::new();
        storage.add_service(TestServiceImpl(42));
        storage.add_service(TestServiceImpl(99));

        let interfaces = Query::<(Entity, &StoredService)>::new().query(&storage).collect::<Vec<_>>();
        assert_eq!(interfaces.len(), 2);
        assert_ne!(interfaces[0].0, interfaces[1].0);
        assert!(interfaces.iter().any(|(_, service)| service.id == TypeId::of::<dyn TestService>()));
        assert!(interfaces.iter().any(|(_, service)| service.id == TypeId::of::<dyn OtherService>()));
        assert_eq!(storage.get_tables().len(), 2);
        assert_eq!(storage.get_service::<dyn TestService>().unwrap().value(), 42);
        assert_eq!(storage.get_service::<dyn OtherService>().unwrap().value(), 42);
    }

    #[test]
    fn test_service_manual_registration_preserves_public_trait() {
        trait TestService {
            fn value(&self) -> u32;
        }

        struct TestServiceImpl;

        impl TestService for TestServiceImpl {
            fn value(&self) -> u32 {
                42
            }
        }

        impl IntoService for TestServiceImpl {
            fn register(self, storage: &mut Storage) {
                let service: &'static dyn TestService = Box::leak(Box::new(self));
                let erased: &'static dyn Any = Box::leak(Box::new(service));
                Self::register_service::<dyn TestService>(storage, erased);
            }
        }

        let mut storage = Storage::new();
        TestServiceImpl.register(&mut storage);
        assert_eq!(storage.get_service::<dyn TestService>().unwrap().value(), 42);
        assert_eq!(storage.get_tables().len(), 1);
    }

    #[test]
    fn test_mutable_storage_param_conflict_scenarios() {
        // `&mut Storage` conflicts with any config access (mutable or immutable) and any other storage access.
        // This test simulates those scenarios by manipulating the MetaData access directly.

        // Scenario 1: `&mut Storage` + `&mut Storage` conflict
        let mut storage = Storage::new();
        let mut metadata = MetaData::new::<bool>();
        assert!(<&mut Storage as Param>::init_state(&mut storage, &mut metadata).is_ok());
        assert_eq!(
            <&mut Storage as Param>::init_state(&mut storage, &mut metadata).unwrap_err(),
            "&mut Storage conflicts with a previous &mut Storage access."
        );

        // Scenario 2: `&Storage` + `&mut Storage` conflict
        let mut storage = Storage::new();
        let mut metadata = MetaData::new::<bool>();
        assert!(<&Storage as Param>::init_state(&mut storage, &mut metadata).is_ok());
        assert_eq!(
            <&mut Storage as Param>::init_state(&mut storage, &mut metadata).unwrap_err(),
            "&mut Storage conflicts with a previous &Storage access."
        );

        // Scenario 3: `ConfigMut<T>` + `&mut Storage` conflict
        let mut storage = Storage::new();
        let mut metadata = MetaData::new::<bool>();
        assert!(<crate::component::params::ConfigMut<u32> as Param>::init_state(&mut storage, &mut metadata).is_ok());
        assert_eq!(
            <&mut Storage as Param>::init_state(&mut storage, &mut metadata).unwrap_err(),
            "&mut Storage conflicts with a previous ConfigMut<T> access."
        );

        // Scenario 4: `Config<T>` + `&mut Storage` conflict
        let mut storage = Storage::new();
        let mut metadata = MetaData::new::<bool>();
        assert!(<crate::component::params::Config<u32> as Param>::init_state(&mut storage, &mut metadata).is_ok());
        assert_eq!(
            <&mut Storage as Param>::init_state(&mut storage, &mut metadata).unwrap_err(),
            "&mut Storage conflicts with a previous Config<T> access."
        );
    }

    #[test]
    fn test_immutable_storage_param_conflict_scenarios() {
        // `&Storage` conflicts with any mutable config access and any `&mut Storage` access.
        // This test simulates those scenarios by manipulating the MetaData access directly.

        // Scenario 1: `&mut Storage` + `&Storage` conflict
        let mut storage = Storage::new();
        let mut metadata = MetaData::new::<bool>();
        assert!(<&mut Storage as Param>::init_state(&mut storage, &mut metadata).is_ok());
        assert_eq!(
            <&Storage as Param>::init_state(&mut storage, &mut metadata).unwrap_err(),
            "&Storage conflicts with a previous &mut Storage access."
        );

        // Scenario 2: `ConfigMut<T>` + `&Storage` conflict
        let mut storage = Storage::new();
        let mut metadata = MetaData::new::<bool>();
        assert!(<crate::component::params::ConfigMut<u32> as Param>::init_state(&mut storage, &mut metadata).is_ok());
        assert_eq!(
            <&Storage as Param>::init_state(&mut storage, &mut metadata).unwrap_err(),
            "&Storage conflicts with a previous ConfigMut<T> access."
        );
    }
}
