//! Type-erased, column-oriented storage.
//!
//! Rows with the same set of field types share a table. Each field type is stored
//! in its own typed column, while callers refer to rows through generational
//! entities.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use alloc::{
    alloc::{alloc, dealloc, handle_alloc_error, realloc},
    vec,
    vec::Vec,
};
use core::{
    alloc::Layout,
    any::TypeId,
    cell::UnsafeCell,
    cmp::Ordering,
    fmt,
    mem::{self, ManuallyDrop},
    ptr::{self, NonNull},
};

/// A value that can be stored in a table column.
///
/// This trait is implemented automatically for every `'static` type.
pub(crate) trait Field: 'static {}

impl<T: 'static> Field for T {}

/// A collection of fields inserted into one row.
pub(crate) trait Fields: Sized {
    /// Returns the field type identifiers in this bundle.
    fn type_ids() -> Vec<TypeId>;
    /// Adds empty typed columns for this bundle to `table`.
    fn add_columns(table: &mut Table);
    /// Appends this bundle's values to `table`.
    fn insert(self, table: &mut Table);
}

macro_rules! impl_fields {
    ($($field:ident),+) => {
        #[allow(non_snake_case)]
        impl<$($field: Field),+> Fields for ($($field,)+) {
            fn type_ids() -> Vec<TypeId> {
                vec![$(TypeId::of::<$field>()),+]
            }

            fn add_columns(table: &mut Table) {
                $(table.add_column::<$field>();)+
            }

            fn insert(self, table: &mut Table) {
                let ($($field,)+) = self;
                $(table.insert_value($field);)+
            }
        }
    };
}

impl Fields for () {
    fn type_ids() -> Vec<TypeId> {
        Vec::new()
    }

    fn add_columns(_table: &mut Table) {}

    fn insert(self, _table: &mut Table) {}
}

impl_fields!(A);
impl_fields!(A, B);
impl_fields!(A, B, C);
impl_fields!(A, B, C, D);
impl_fields!(A, B, C, D, E);
impl_fields!(A, B, C, D, E, F);

/// A stable identity for one stored row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entity {
    index: usize,
    generation: u32,
}

impl Entity {
    /// Returns the metadata slot used by this entity.
    pub(crate) const fn index(self) -> usize {
        self.index
    }
}

/// Physical row location for a live entity.
#[derive(Clone, Copy, Debug)]
pub(super) struct Location {
    pub(super) table: usize,
    pub(super) row: usize,
}

/// Structural move prepared before newly added fields are inserted.
struct PendingAdd {
    source: Location,
    target: usize,
    target_row: usize,
    moved: Option<Entity>,
}

/// Generation and current row location for one entity slot.
#[derive(Clone, Copy, Debug)]
pub(super) struct EntityMeta {
    generation: u32,
    location: Option<Location>,
}

/// Errors produced by structural entity operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityError {
    /// The entity is stale or does not identify a live row.
    InvalidEntity,
    /// A field being added is already present on the entity.
    FieldAlreadyExists,
}

impl fmt::Display for EntityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEntity => f.write_str("invalid or stale entity"),
            Self::FieldAlreadyExists => f.write_str("field already exists on entity"),
        }
    }
}

impl core::error::Error for EntityError {}

type DropValue = unsafe fn(NonNull<u8>);

/// Drops one type-erased value as `T`.
///
/// # Safety
///
/// `value` must point to a live, properly aligned `T` that will not be used again.
unsafe fn drop_value<T>(value: NonNull<u8>) {
    // SAFETY: The caller guarantees that `value` identifies a live `T`.
    unsafe { value.cast::<T>().drop_in_place() };
}

/// Contiguous type-erased storage for one table field.
struct BlobColumn {
    values: NonNull<u8>,
    len: usize,
    capacity: usize,
    item_layout: Layout,
    drop_value: Option<DropValue>,
}

impl BlobColumn {
    /// Creates an empty column for values with `item_layout`.
    fn new(item_layout: Layout, drop_value: Option<DropValue>, capacity: usize) -> Self {
        Self::with_layout(item_layout, drop_value, capacity)
    }

    /// Creates an empty column, allocating space when the item layout and requested capacity require it.
    fn with_layout(item_layout: Layout, drop_value: Option<DropValue>, capacity: usize) -> Self {
        let mut column = Self {
            values: Self::dangling(item_layout),
            len: 0,
            capacity: if item_layout.size() == 0 { usize::MAX } else { 0 },
            item_layout,
            drop_value,
        };
        if item_layout.size() != 0 && capacity != 0 {
            column.grow(capacity);
        }
        column
    }

    /// Returns the number of initialized values in this column.
    fn len(&self) -> usize {
        self.len
    }

    /// Creates an empty column with the same item layout and drop behavior.
    fn empty(&self, capacity: usize) -> Self {
        Self::with_layout(self.item_layout, self.drop_value, capacity)
    }

    /// Appends the initialized value at `value` and assumes ownership of its bytes.
    ///
    /// # Safety
    ///
    /// `value` must point to an initialized value matching this column's layout. The caller must not subsequently
    /// drop or otherwise use the transferred value.
    unsafe fn push(&mut self, value: NonNull<u8>) {
        self.reserve_one();
        if self.item_layout.size() != 0 {
            // SAFETY: The caller guarantees `value` identifies an initialized value matching `item_layout`;
            // `reserve_one` guarantees an uninitialized target slot in a distinct allocation.
            unsafe {
                value.copy_to_nonoverlapping(self.element(self.len), self.item_layout.size());
            }
        }
        self.len += 1;
    }

    /// Removes a value without dropping it and returns the slot containing its bytes.
    ///
    /// # Safety
    ///
    /// The caller must read the returned value using this column's registered type and assume responsibility for
    /// dropping it.
    unsafe fn take(&mut self, row: usize) -> Option<NonNull<u8>> {
        if row >= self.len {
            return None;
        }
        let last = self.len - 1;
        if row != last && self.item_layout.size() != 0 {
            // SAFETY: `row` and `last` identify distinct initialized elements in this allocation.
            unsafe {
                ptr::swap_nonoverlapping(
                    self.element(row).as_ptr(),
                    self.element(last).as_ptr(),
                    self.item_layout.size(),
                );
            }
        }
        self.len = last;
        Some(self.element(last))
    }

    /// Removes and drops one row, filling the hole with the final value.
    fn swap_remove(&mut self, row: usize) {
        assert!(row < self.len, "row must be in bounds");
        let last = self.len - 1;
        if row != last && self.item_layout.size() != 0 {
            // SAFETY: `row` and `last` identify distinct initialized elements in this allocation.
            unsafe {
                ptr::swap_nonoverlapping(
                    self.element(row).as_ptr(),
                    self.element(last).as_ptr(),
                    self.item_layout.size(),
                );
            }
        }
        self.len = last;
        if let Some(drop_value) = self.drop_value {
            // SAFETY: The old last slot contains the removed value after the swap and is no longer part of the column.
            unsafe { drop_value(self.element(last)) };
        }
    }

    /// Transfers one row into `target` without dropping the moved value.
    fn move_row_to(&mut self, row: usize, target: &mut Self) {
        assert!(row < self.len, "row must be in bounds");
        assert_eq!(self.item_layout, target.item_layout, "matching columns must have the same layout");
        target.reserve_one();
        if self.item_layout.size() != 0 {
            // SAFETY: The source row is initialized, the target slot is allocated and uninitialized, and the
            // allocations are distinct.
            unsafe {
                self.element(row).copy_to_nonoverlapping(target.element(target.len), self.item_layout.size());
            }
        }
        target.len += 1;

        let last = self.len - 1;
        if row != last && self.item_layout.size() != 0 {
            // SAFETY: The last element is initialized and may overlap neither the source row nor itself.
            unsafe {
                self.element(last).copy_to_nonoverlapping(self.element(row), self.item_layout.size());
            }
        }
        self.len = last;
    }

    /// Ensures space exists for one additional value.
    fn reserve_one(&mut self) {
        if self.len == self.capacity {
            let capacity =
                if self.capacity == 0 { 4 } else { self.capacity.checked_mul(2).expect("column capacity overflow") };
            self.grow(capacity);
        }
    }

    /// Reallocates the column to exactly `capacity` elements.
    fn grow(&mut self, capacity: usize) {
        debug_assert!(self.item_layout.size() != 0);
        debug_assert!(capacity > self.capacity);
        let new_layout = self.allocation_layout(capacity);
        let values = if self.capacity == 0 {
            // SAFETY: `new_layout` is non-zero and valid for allocation.
            unsafe { alloc(new_layout) }
        } else {
            let old_layout = self.allocation_layout(self.capacity);
            // SAFETY: `values` was allocated with `old_layout`; `new_layout.size()` is the requested new size.
            unsafe { realloc(self.values.as_ptr(), old_layout, new_layout.size()) }
        };
        self.values = NonNull::new(values).unwrap_or_else(|| handle_alloc_error(new_layout));
        self.capacity = capacity;
    }

    /// Computes the allocation layout for `capacity` elements.
    fn allocation_layout(&self, capacity: usize) -> Layout {
        let size = self.item_layout.size().checked_mul(capacity).expect("column allocation size overflow");
        Layout::from_size_align(size, self.item_layout.align()).expect("field layout produces a valid allocation")
    }

    /// Returns the address of the slot at `index`.
    fn element(&self, index: usize) -> NonNull<u8> {
        debug_assert!(index <= self.capacity);
        // SAFETY: The allocation contains `capacity` elements and callers use an index no greater than capacity.
        unsafe { self.values.add(self.item_layout.size() * index) }
    }

    /// Creates a non-null aligned pointer for a column without an allocation.
    fn dangling(layout: Layout) -> NonNull<u8> {
        // SAFETY: Alignment is always non-zero and therefore forms a non-null, correctly aligned dangling pointer.
        unsafe { NonNull::new_unchecked(layout.align() as *mut u8) }
    }
}

impl Drop for BlobColumn {
    fn drop(&mut self) {
        if let Some(drop_value) = self.drop_value {
            for index in 0..self.len {
                // SAFETY: Every index below `len` contains one live value of the column's registered type.
                unsafe { drop_value(self.element(index)) };
            }
        }
        if self.item_layout.size() != 0 && self.capacity != 0 {
            let layout = self.allocation_layout(self.capacity);
            // SAFETY: `values` was allocated with this layout and has not been deallocated.
            unsafe { dealloc(self.values.as_ptr(), layout) };
        }
    }
}

/// One type-erased field column and its runtime type identifier.
struct Column {
    type_id: TypeId,
    values: UnsafeCell<BlobColumn>,
}

/// One archetype table whose rows all contain the same field types.
pub struct Table {
    entities: Vec<Entity>,
    columns: Vec<Column>,
}

impl Table {
    /// Creates an empty archetype table with row capacity `capacity`.
    fn with_capacity(capacity: usize) -> Self {
        Self { entities: Vec::with_capacity(capacity), columns: Vec::new() }
    }

    /// Returns the number of entities stored in this table.
    pub(super) fn len(&self) -> usize {
        self.entities.len()
    }

    /// Returns the sorted field type identifiers defining this table's archetype.
    fn type_ids(&self) -> Vec<TypeId> {
        let mut type_ids = Vec::with_capacity(self.columns.len());
        for column in &self.columns {
            type_ids.push(column.type_id);
        }
        type_ids
    }

    /// Returns whether this table contains exactly the sorted field types in `type_ids`.
    fn has_type_ids(&self, type_ids: &[TypeId]) -> bool {
        if self.columns.len() != type_ids.len() {
            return false;
        }
        for (index, column) in self.columns.iter().enumerate() {
            if type_ids.get(index) != Some(&column.type_id) {
                return false;
            }
        }
        true
    }

    /// Finds a field column or the position where that field would be inserted.
    #[inline(never)]
    fn column_index(&self, type_id: TypeId) -> Result<usize, usize> {
        for (index, column) in self.columns.iter().enumerate() {
            match column.type_id.cmp(&type_id) {
                Ordering::Less => {}
                Ordering::Equal => return Ok(index),
                Ordering::Greater => return Err(index),
            }
        }
        Err(self.columns.len())
    }

    /// Adds an empty column for `T`.
    fn add_column<T: Field>(&mut self) {
        self.add_column_erased(
            TypeId::of::<T>(),
            Layout::new::<T>(),
            mem::needs_drop::<T>().then_some(drop_value::<T> as DropValue),
        );
    }

    /// Adds an empty type-erased column while preserving sorted column order.
    #[inline(never)]
    fn add_column_erased(&mut self, type_id: TypeId, layout: Layout, drop_value: Option<DropValue>) {
        assert!(self.entities.is_empty(), "columns can only be added to an empty table");
        let index = self.column_index(type_id).expect_err("field bundle contains a duplicate type");
        self.columns.insert(
            index,
            Column { type_id, values: UnsafeCell::new(BlobColumn::new(layout, drop_value, self.entities.capacity())) },
        );
    }

    /// Appends one typed value to its matching column.
    fn insert_value<T: Field>(&mut self, value: T) {
        let mut value = ManuallyDrop::new(value);
        self.insert_value_erased(TypeId::of::<T>(), Layout::new::<T>(), NonNull::from(&mut value).cast());
    }

    /// Appends one type-erased value to its matching column.
    #[inline(never)]
    fn insert_value_erased(&mut self, type_id: TypeId, layout: Layout, value: NonNull<u8>) {
        let index = self.column_index(type_id).expect("table must contain the inserted field type");
        let column = self.columns.get_mut(index).expect("column index was obtained from this table").values.get_mut();
        debug_assert_eq!(column.item_layout, layout);
        // SAFETY: The typed caller supplies initialized bytes matching `layout`; ownership transfers to the column.
        unsafe { column.push(value) };
    }

    /// Returns the allocation backing the column for `T`.
    pub(super) fn shared_value_ptr<T: Field>(&self) -> Option<*const T> {
        Some(self.shared_value_ptr_erased(TypeId::of::<T>(), Layout::new::<T>())?.cast::<T>().as_ptr().cast_const())
    }

    /// Returns the type-erased allocation backing the requested shared column.
    #[inline(never)]
    fn shared_value_ptr_erased(&self, type_id: TypeId, layout: Layout) -> Option<NonNull<u8>> {
        let index = self.column_index(type_id).ok()?;
        let values = self.columns.get(index)?.values.get();
        // SAFETY: This only projects shared access to the column allocation.
        let column = unsafe { &*values };
        debug_assert_eq!(column.item_layout, layout);
        Some(column.values)
    }

    /// Returns the mutable allocation backing the column for `T`.
    pub(super) fn mutable_value_ptr<T: Field>(&self) -> Option<*mut T> {
        Some(self.mutable_value_ptr_erased(TypeId::of::<T>(), Layout::new::<T>())?.cast::<T>().as_ptr())
    }

    /// Returns the type-erased allocation backing the requested mutable column.
    #[inline(never)]
    fn mutable_value_ptr_erased(&self, type_id: TypeId, layout: Layout) -> Option<NonNull<u8>> {
        let index = self.column_index(type_id).ok()?;
        let values = self.columns.get(index)?.values.get();
        // SAFETY: Mutable queries hold exclusive table access and reject duplicate field projections.
        let column = unsafe { &mut *values };
        debug_assert_eq!(column.item_layout, layout);
        Some(column.values)
    }

    /// Removes and returns the value of type `T` from `row`.
    fn take_value<T: Field>(&mut self, row: usize) -> Option<T> {
        let value = self.take_value_erased(TypeId::of::<T>(), Layout::new::<T>(), row)?;
        // SAFETY: The erased helper validated the column layout and transferred ownership of this initialized slot.
        Some(unsafe { value.cast::<T>().read() })
    }

    /// Removes one type-erased value without dropping it.
    #[inline(never)]
    fn take_value_erased(&mut self, type_id: TypeId, layout: Layout, row: usize) -> Option<NonNull<u8>> {
        let index = self.column_index(type_id).ok()?;
        let column = self.columns.get_mut(index)?.values.get_mut();
        debug_assert_eq!(column.item_layout, layout);
        // SAFETY: The matching TypeId and layout identify the column, and ownership of the removed slot transfers out.
        unsafe { column.take(row) }
    }

    /// Removes and drops one row, returning the entity moved into its place.
    fn swap_remove(&mut self, row: usize) -> Option<Entity> {
        for column in &mut self.columns {
            column.values.get_mut().swap_remove(row);
        }
        self.entities.swap_remove(row);
        self.entities.get(row).copied()
    }

    /// Creates an empty table by copying the requested column definitions from this table.
    fn empty_from_existing(&self, type_ids: &[TypeId]) -> Self {
        let mut table = Self::with_capacity(4);
        for type_id in type_ids {
            let Ok(index) = self.column_index(*type_id) else {
                continue;
            };
            let source = self.columns.get(index).expect("column index was obtained from this table");
            // SAFETY: Structural table changes require exclusive table access, so no query references are live.
            let values = unsafe { &*source.values.get() };
            table
                .columns
                .push(Column { type_id: *type_id, values: UnsafeCell::new(values.empty(table.entities.capacity())) });
        }
        table
    }

    /// Moves one row into `target`, optionally leaving one removed field behind.
    fn move_row_to(&mut self, row: usize, target: &mut Self, removed: Option<TypeId>) -> (usize, Option<Entity>) {
        let target_row = target.entities.len();
        let entity = self.entities.swap_remove(row);
        target.entities.push(entity);

        for source in &mut self.columns {
            if removed == Some(source.type_id) {
                continue;
            }
            let index =
                target.column_index(source.type_id).expect("target table must contain every non-removed source column");
            let target = target.columns.get_mut(index).expect("column index was obtained from target table");
            source.values.get_mut().move_row_to(row, target.values.get_mut());
        }

        let rows = self.entities.len();
        debug_assert!(self.columns.iter_mut().all(|column| column.values.get_mut().len() == rows));
        (target_row, self.entities.get(row).copied())
    }

    /// Returns the allocation backing this table's entity column.
    pub(super) fn entities_ptr(&self) -> *const Entity {
        self.entities.as_ptr()
    }
}

/// A collection of archetype tables and their generational entity metadata.
pub(crate) struct Tables {
    entity_metadata: Vec<EntityMeta>,
    free_entity_indices: Vec<usize>,
    tables: Vec<Table>,
}

impl Tables {
    /// Creates an empty table collection.
    pub const fn new() -> Self {
        Self { entity_metadata: Vec::new(), free_entity_indices: Vec::new(), tables: Vec::new() }
    }

    /// Returns the number of live rows.
    pub fn len(&self) -> usize {
        self.tables.iter().map(Table::len).sum()
    }

    /// Returns whether `entity` identifies a live row.
    pub fn contains(&self, entity: Entity) -> bool {
        self.metadata(entity).is_some()
    }

    /// Inserts a row at the end of its matching archetype table.
    pub fn insert_new<F: Fields>(&mut self, fields: F) -> Entity {
        let ids = normalized_ids::<F>();
        let table = self.table_index(&ids, F::add_columns);
        let entity = self.insert_entity(table);
        let table = self.tables.get_mut(table).expect("table index was just resolved");
        fields.insert(table);
        let rows = table.entities.len();
        debug_assert!(table.columns.iter_mut().all(|column| column.values.get_mut().len() == rows));
        entity
    }

    /// Removes a row and all its fields.
    pub fn remove(&mut self, entity: Entity) -> Result<(), EntityError> {
        let location = self.metadata(entity).ok_or(EntityError::InvalidEntity)?;
        let moved =
            self.tables.get_mut(location.table).expect("live entity must reference a table").swap_remove(location.row);
        if let Some(moved) = moved {
            self.entity_metadata.get_mut(moved.index).expect("table entity must have metadata").location =
                Some(Location { table: location.table, row: location.row });
        }

        let metadata = self.entity_metadata.get_mut(entity.index).expect("validated entity must have metadata");
        metadata.generation = metadata.generation.wrapping_add(1);
        metadata.location = None;
        self.free_entity_indices.push(entity.index);
        Ok(())
    }

    /// Adds fields to a row, moving it to the matching archetype table.
    pub fn add_fields<F: Fields>(&mut self, entity: Entity, fields: F) -> Result<(), EntityError> {
        let added = normalized_ids::<F>();
        let pending = self.begin_add_fields(entity, &added, F::add_columns)?;
        fields.insert(self.tables.get_mut(pending.target).expect("target table must exist"));
        self.finish_move(entity, pending.target, pending.target_row, pending.source, pending.moved);
        Ok(())
    }

    /// Removes and returns one field from an entity.
    pub fn take_field<T: Field>(&mut self, entity: Entity) -> Option<T> {
        let removed = TypeId::of::<T>();
        let (location, target) = self.prepare_take_field(entity, removed)?;
        let value = self
            .tables
            .get_mut(location.table)
            .expect("live entity must reference a table")
            .take_value::<T>(location.row)
            .expect("validated field type must downcast");
        let (target_row, moved) = self.move_entity_excluding(location, target, removed);
        self.finish_move(entity, target, target_row, location, moved);
        Some(value)
    }

    /// Prepares the structural row move required to add `added` fields.
    #[inline(never)]
    fn begin_add_fields(
        &mut self,
        entity: Entity,
        added: &[TypeId],
        add_columns: fn(&mut Table),
    ) -> Result<PendingAdd, EntityError> {
        let source = self.metadata(entity).ok_or(EntityError::InvalidEntity)?;
        let mut target_ids = self.tables.get(source.table).expect("live entity must reference a table").type_ids();
        if added.iter().any(|type_id| target_ids.contains(type_id)) {
            return Err(EntityError::FieldAlreadyExists);
        }

        target_ids.extend_from_slice(added);
        sort_type_ids(&mut target_ids);
        let target = self.table_index_from(source.table, &target_ids, Some(add_columns));
        let (target_row, moved) = self.move_entity(source, target);
        Ok(PendingAdd { source, target, target_row, moved })
    }

    /// Finds or creates the target table needed to remove one field.
    #[inline(never)]
    fn prepare_take_field(&mut self, entity: Entity, removed: TypeId) -> Option<(Location, usize)> {
        let source = self.metadata(entity)?;
        let old = self.tables.get(source.table).expect("live entity must reference a table").type_ids();
        if !old.contains(&removed) {
            return None;
        }

        let target_ids = old.into_iter().filter(|type_id| *type_id != removed).collect::<Vec<_>>();
        let target = self.table_index_from(source.table, &target_ids, None);
        Some((source, target))
    }

    /// Allocates a new entity identity, reusing a vacant metadata slot when possible.
    fn new_entity(&mut self) -> Entity {
        if let Some(index) = self.free_entity_indices.pop() {
            let metadata = self.entity_metadata.get(index).expect("free entity index must have metadata");
            Entity { index, generation: metadata.generation }
        } else {
            let index = self.entity_metadata.len();
            self.entity_metadata.push(EntityMeta { generation: 0, location: None });
            Entity { index, generation: 0 }
        }
    }

    /// Resolves `entity` to its current row location.
    fn metadata(&self, entity: Entity) -> Option<Location> {
        entity_location(&self.entity_metadata, entity)
    }

    /// Finds or creates the table for a newly inserted field bundle.
    #[inline(never)]
    fn table_index(&mut self, ids: &[TypeId], add_columns: fn(&mut Table)) -> usize {
        if let Some(index) = self.find_table(ids) {
            return index;
        }

        let mut table = Table::with_capacity(4);
        add_columns(&mut table);
        debug_assert_eq!(table.type_ids(), ids);
        self.insert_table(table)
    }

    /// Appends a new entity to `table` and records its location.
    #[inline(never)]
    fn insert_entity(&mut self, table: usize) -> Entity {
        let row = self.tables.get(table).expect("table index was just resolved").len();
        let entity = self.new_entity();
        self.tables.get_mut(table).expect("table index was just resolved").entities.push(entity);
        self.entity_metadata.get_mut(entity.index).expect("new entity must have metadata").location =
            Some(Location { table, row });
        entity
    }

    /// Finds or derives a target table from an existing archetype.
    #[inline(never)]
    fn table_index_from(&mut self, source: usize, ids: &[TypeId], add_columns: Option<fn(&mut Table)>) -> usize {
        if let Some(index) = self.find_table(ids) {
            return index;
        }

        let source = self.tables.get(source).expect("source table must exist");
        let mut table = source.empty_from_existing(ids);
        if let Some(add_columns) = add_columns {
            add_columns(&mut table);
        }
        debug_assert_eq!(table.type_ids(), ids);
        self.insert_table(table)
    }

    /// Finds the table whose field types exactly match `ids`.
    #[inline(never)]
    fn find_table(&self, ids: &[TypeId]) -> Option<usize> {
        for (index, table) in self.tables.iter().enumerate() {
            if table.has_type_ids(ids) {
                return Some(index);
            }
        }
        None
    }

    /// Appends a newly created archetype table and returns its index.
    fn insert_table(&mut self, table: Table) -> usize {
        let index = self.tables.len();
        self.tables.push(table);
        index
    }

    /// Moves a complete row between two archetype tables.
    fn move_entity(&mut self, source: Location, target: usize) -> (usize, Option<Entity>) {
        assert_ne!(source.table, target, "structural changes must move a row to a different archetype");
        let (source_table, target_table) = get_two_mut(&mut self.tables, source.table, target);
        source_table.move_row_to(source.row, target_table, None)
    }

    /// Moves a row while omitting one removed field.
    fn move_entity_excluding(&mut self, source: Location, target: usize, removed: TypeId) -> (usize, Option<Entity>) {
        assert_ne!(source.table, target, "structural changes must move a row to a different archetype");
        let (source_table, target_table) = get_two_mut(&mut self.tables, source.table, target);
        source_table.move_row_to(source.row, target_table, Some(removed))
    }

    /// Updates metadata after a row moves between archetype tables.
    #[inline(never)]
    fn finish_move(
        &mut self,
        entity: Entity,
        target: usize,
        target_row: usize,
        source: Location,
        moved: Option<Entity>,
    ) {
        if let Some(moved) = moved {
            self.entity_metadata.get_mut(moved.index).expect("moved entity must have metadata").location =
                Some(Location { table: source.table, row: source.row });
        }
        self.entity_metadata.get_mut(entity.index).expect("validated entity must have metadata").location =
            Some(Location { table: target, row: target_row });
    }

    /// Iterates over every archetype table.
    pub(super) fn iter(&self) -> core::slice::Iter<'_, Table> {
        self.tables.iter()
    }

    /// Returns the metadata used to resolve generational entities.
    pub(super) fn entity_metadata(&self) -> &[EntityMeta] {
        &self.entity_metadata
    }
}

impl Default for Tables {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Tables {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tables")
            .field("rows", &self.len())
            .field("tables", &self.tables.len())
            .field("entity_slots", &self.entity_metadata.len())
            .field("free_entity_slots", &self.free_entity_indices.len())
            .field("schemas", &self.tables.len())
            .finish()
    }
}

/// Returns the normalized field identifiers for `F`.
fn normalized_ids<F: Fields>() -> Vec<TypeId> {
    normalize_ids(F::type_ids())
}

/// Sorts and validates a field identifier bundle.
#[inline(never)]
fn normalize_ids(mut ids: Vec<TypeId>) -> Vec<TypeId> {
    sort_type_ids(&mut ids);
    assert!(
        ids.windows(2).all(|pair| pair.first() != pair.get(1)),
        "a field bundle cannot contain the same type more than once"
    );
    ids
}

/// Sorts the small field identifier lists used as archetype schemas.
#[inline(never)]
fn sort_type_ids(ids: &mut [TypeId]) {
    for index in 1..ids.len() {
        let mut current = index;
        while current > 0
            && ids.get(current).zip(ids.get(current - 1)).is_some_and(|(value, previous)| value < previous)
        {
            ids.swap(current, current - 1);
            current -= 1;
        }
    }
}

/// Returns mutable references to two distinct slice elements.
fn get_two_mut<T>(values: &mut [T], a: usize, b: usize) -> (&mut T, &mut T) {
    assert_ne!(a, b, "indices must be distinct");
    if a < b {
        let (left, right) = values.split_at_mut(b);
        (
            left.get_mut(a).expect("first index must be in bounds"),
            right.get_mut(0).expect("second index must be in bounds"),
        )
    } else {
        let (left, right) = values.split_at_mut(a);
        (
            right.get_mut(0).expect("first index must be in bounds"),
            left.get_mut(b).expect("second index must be in bounds"),
        )
    }
}

/// Resolves an entity only when its generation still matches its metadata slot.
#[inline(never)]
pub(super) fn entity_location(entity_metadata: &[EntityMeta], entity: Entity) -> Option<Location> {
    let metadata = entity_metadata.get(entity.index)?;
    (metadata.generation == entity.generation).then_some(metadata.location).flatten()
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use alloc::{format, rc::Rc, vec};
    use core::{
        cell::Cell,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use crate::component::{Query, QueryData, QueryIter, Storage};

    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct A(u32);

    #[derive(Debug, PartialEq, Eq)]
    struct B(u32);

    #[derive(Debug, PartialEq, Eq)]
    struct C(u32);

    #[derive(Debug, PartialEq, Eq)]
    #[repr(align(64))]
    struct Aligned(u32);

    struct DropValue(Rc<Cell<usize>>);

    impl Drop for DropValue {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    struct DropZst;

    static DROP_ZST_COUNT: AtomicUsize = AtomicUsize::new(0);

    impl Drop for DropZst {
        fn drop(&mut self) {
            DROP_ZST_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn insert<F: Fields>(storage: &mut Storage, fields: F) -> Entity {
        storage.get_tables_mut().insert_new(fields)
    }

    fn query<'storage, Q: QueryData>(storage: &'storage mut Storage) -> QueryIter<'storage, Q> {
        Query::<Q>::new().query_mut(storage)
    }

    fn get<'storage, Q: QueryData>(storage: &'storage mut Storage, entity: Entity) -> Option<Q::Item<'storage>> {
        query::<Q>(storage).get(&entity)
    }

    #[test]
    fn entity_errors_have_clear_messages() {
        assert_eq!(format!("{}", EntityError::InvalidEntity), "invalid or stale entity");
        assert_eq!(format!("{}", EntityError::FieldAlreadyExists), "field already exists on entity");
    }

    #[test]
    fn table_defaults_and_debug_output_track_entity_metadata() {
        let mut tables = Tables::default();
        let entity = tables.insert_new((A(1),));

        assert!(tables.contains(entity));
        assert_eq!(
            format!("{tables:?}"),
            "Tables { rows: 1, tables: 1, entity_slots: 1, free_entity_slots: 0, schemas: 1 }"
        );

        tables.remove(entity).unwrap();
        assert!(!tables.contains(entity));
    }

    #[test]
    fn blob_column_take_rejects_an_out_of_bounds_row() {
        let mut column = BlobColumn::new(Layout::new::<u32>(), None, 0);

        assert_eq!(column.len(), 0);
        // SAFETY: No value is returned or read because row zero is outside the empty column.
        assert!(unsafe { column.take(0) }.is_none());
    }

    #[test]
    fn insert_get_mut_and_remove_rows() {
        let mut storage = Storage::new();
        let first = insert(&mut storage, (A(1), B(2)));
        let second = insert(&mut storage, (B(3), A(4)));

        assert_eq!(storage.get_tables().len(), 2);
        assert_eq!(storage.get_tables().tables.len(), 1);
        assert_eq!(get::<&A>(&mut storage, first), Some(&A(1)));
        get::<&mut A>(&mut storage, second).unwrap().0 = 5;
        assert_eq!(get::<&A>(&mut storage, second), Some(&A(5)));

        storage.get_tables_mut().remove(first).unwrap();
        assert!(get::<&A>(&mut storage, first).is_none());
        assert_eq!(get::<&A>(&mut storage, second), Some(&A(5)));
    }

    #[test]
    fn stale_entities_are_rejected_and_slots_reused() {
        let mut tables = Tables::new();
        let old = tables.insert_new((A(1),));
        tables.remove(old).unwrap();
        let new = tables.insert_new((A(2),));

        assert_eq!(old.index, new.index);
        assert_ne!(old.generation, new.generation);
        assert_eq!(tables.remove(old), Err(EntityError::InvalidEntity));
    }

    #[test]
    fn insertion_preserves_append_order() {
        let mut storage = Storage::new();
        let first = insert(&mut storage, (A(1),));
        let second = insert(&mut storage, (A(2),));
        let third = insert(&mut storage, (A(3),));

        let order = query::<&A>(&mut storage).map(|value| value.0).collect::<Vec<_>>();
        assert_eq!(order, vec![1, 2, 3]);
        assert_eq!(get::<&A>(&mut storage, first), Some(&A(1)));
        assert_eq!(get::<&A>(&mut storage, second), Some(&A(2)));
        assert_eq!(get::<&A>(&mut storage, third), Some(&A(3)));
    }

    #[test]
    fn add_fields_moves_rows_safely() {
        let mut storage = Storage::new();
        let first = insert(&mut storage, (A(1),));
        let second = insert(&mut storage, (A(2),));

        storage.get_tables_mut().add_fields(first, (B(3),)).unwrap();
        assert_eq!(get::<&A>(&mut storage, first), Some(&A(1)));
        assert_eq!(get::<&B>(&mut storage, first), Some(&B(3)));
        assert_eq!(get::<&A>(&mut storage, second), Some(&A(2)));
        assert_eq!(storage.get_tables_mut().add_fields(first, (B(4),)), Err(EntityError::FieldAlreadyExists));
    }

    #[test]
    fn take_field_returns_value_and_preserves_other_rows() {
        let mut storage = Storage::new();
        let first = insert(&mut storage, (A(1), B(2)));
        let second = insert(&mut storage, (A(3), B(4)));

        assert_eq!(storage.get_tables_mut().take_field::<C>(first), None);
        assert_eq!(storage.get_tables_mut().take_field::<B>(first), Some(B(2)));
        assert!(get::<&B>(&mut storage, first).is_none());
        assert_eq!(get::<&A>(&mut storage, first), Some(&A(1)));
        assert_eq!(get::<&B>(&mut storage, second), Some(&B(4)));

        storage.get_tables_mut().add_fields(first, (B(5),)).unwrap();
        assert_eq!(get::<&B>(&mut storage, first), Some(&B(5)));
    }

    #[test]
    fn mutable_two_column_query_updates_matching_rows() {
        let mut storage = Storage::new();
        let first = insert(&mut storage, (A(1), B(2)));
        let second = insert(&mut storage, (A(3), C(4)));

        for (entity, a, b) in query::<(Entity, &mut A, &mut B)>(&mut storage) {
            assert_eq!(entity, first);
            a.0 += 10;
            b.0 += 20;
        }

        assert_eq!(get::<&A>(&mut storage, first), Some(&A(11)));
        assert_eq!(get::<&B>(&mut storage, first), Some(&B(22)));
        assert_eq!(get::<&A>(&mut storage, second), Some(&A(3)));
    }

    #[test]
    fn typed_query_projects_only_matching_rows_with_entities() {
        let mut storage = Storage::new();
        let matching = insert(&mut storage, (A(1), B(2)));
        insert(&mut storage, (A(3), C(4)));

        let rows = query::<(Entity, &A, &B)>(&mut storage).map(|(entity, a, b)| (entity, a.0, b.0)).collect::<Vec<_>>();

        assert_eq!(rows, vec![(matching, 1, 2)]);
    }

    #[test]
    fn typed_query_mutates_projected_fields_and_gets_by_entity() {
        let mut storage = Storage::new();
        let matching = insert(&mut storage, (A(1), B(2)));
        let not_matching = insert(&mut storage, (A(3),));

        for (a, b) in query::<(&mut A, &B)>(&mut storage) {
            a.0 += b.0;
        }

        let (a, b) = get::<(&A, &B)>(&mut storage, matching).unwrap();
        assert_eq!((a.0, b.0), (3, 2));
        assert!(get::<(&A, &B)>(&mut storage, not_matching).is_none());
    }

    #[test]
    fn validated_query_can_be_rebound_after_structural_changes() {
        let query = Query::<&A>::new();
        let mut storage = Storage::new();
        insert(&mut storage, (A(1),));
        assert_eq!(query.query_mut(&mut storage).count(), 1);

        insert(&mut storage, (A(2),));
        assert_eq!(query.query_mut(&mut storage).count(), 2);
    }

    #[test]
    fn get_only_searches_unvisited_rows() {
        let query = Query::<(Entity, &mut A)>::new();
        let mut storage = Storage::new();
        let first = insert(&mut storage, (A(1),));
        let second = insert(&mut storage, (A(2),));

        let mut iter = query.query_mut(&mut storage);
        assert_eq!(iter.next().map(|(entity, _)| entity), Some(first));
        assert!(iter.get(&first).is_none());

        assert_eq!(query.query_mut(&mut storage).get(&second).map(|(_, value)| value.0), Some(2));
    }

    #[test]
    #[should_panic(expected = "a mutable query cannot project the same field type more than once")]
    fn typed_query_rejects_duplicate_field_types() {
        let _ = Query::<(&A, &mut A)>::new();
    }

    #[test]
    fn read_only_query_allows_duplicate_field_types() {
        let query = Query::<(&A, &A)>::new();
        let mut storage = Storage::new();
        insert(&mut storage, (A(1),));

        let (first, second) = query.query_mut(&mut storage).next().unwrap();
        assert!(core::ptr::eq(first, second));
    }

    #[test]
    fn values_are_dropped_exactly_once() {
        let drops = Rc::new(Cell::new(0));
        {
            let mut tables = Tables::new();
            let entity = tables.insert_new((DropValue(drops.clone()),));
            tables.remove(entity).unwrap();
            assert_eq!(drops.get(), 1);
            tables.insert_new((DropValue(drops.clone()),));
        }
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn blob_columns_grow_and_preserve_alignment() {
        let mut storage = Storage::new();
        for value in 0..32 {
            insert(&mut storage, (Aligned(value),));
        }

        for (expected, value) in query::<&Aligned>(&mut storage).enumerate() {
            assert_eq!(value.0, expected as u32);
            assert_eq!(ptr::from_ref(value).addr() % 64, 0);
        }
    }

    #[test]
    fn zero_sized_fields_are_dropped_exactly_once() {
        DROP_ZST_COUNT.store(0, Ordering::SeqCst);
        {
            let mut tables = Tables::new();
            let removed = tables.insert_new((DropZst,));
            let taken = tables.insert_new((DropZst,));
            tables.insert_new((DropZst,));

            tables.remove(removed).unwrap();
            drop(tables.take_field::<DropZst>(taken).unwrap());
            assert_eq!(DROP_ZST_COUNT.load(Ordering::SeqCst), 2);
        }
        assert_eq!(DROP_ZST_COUNT.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn fields_with_drop_survive_archetype_moves() {
        let drops = Rc::new(Cell::new(0));
        {
            let mut tables = Tables::new();
            let entity = tables.insert_new((DropValue(drops.clone()),));
            tables.add_fields(entity, (A(1),)).unwrap();
            assert_eq!(drops.get(), 0);
        }
        assert_eq!(drops.get(), 1);
    }
}
