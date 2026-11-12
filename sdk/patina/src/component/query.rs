//! Typed queries over component storage.
//!
//! Queries describe the fields to project from each matching entity. A query is validated once and can then be
//! rebound to storage for repeated iteration.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use core::{any::TypeId, marker::PhantomData};

use super::{
    storage::Storage,
    tables::{Entity, EntityMeta, Field, Table, Tables, entity_location},
};

mod sealed {
    pub trait Sealed {}
}

/// A supported set of field projections for [`Query`].
///
/// This trait is sealed. Queries may contain a shared or mutable reference to one field, or a tuple of up to six
/// distinct field projections.
#[doc(hidden)]
#[allow(private_bounds)]
pub trait QueryData: sealed::Sealed {
    /// Table-specific state prepared before rows are projected.
    type State;
    /// The fields returned for one matching entity.
    type Item<'storage>;

    /// Visits the type identifier and mutability of every projected field.
    fn visit_accesses(visitor: &mut dyn FnMut(TypeId, bool));
    /// Prepares projection state for `table`, or returns `None` when the table does not match.
    fn prepare(table: &Table) -> Option<Self::State>;

    /// Projects one row from previously prepared query state.
    ///
    /// # Safety
    ///
    /// `row` must be in bounds for every pointer in `state`. Mutable projections must be unique for the row and
    /// field being fetched.
    unsafe fn fetch<'storage>(state: &Self::State, row: usize) -> Self::Item<'storage>;
}

/// A query projection that only provides shared access.
#[doc(hidden)]
pub trait ReadOnly {}

impl<T: Field> sealed::Sealed for &T {}
impl<T: Field> ReadOnly for &T {}

impl<T: Field> QueryData for &T {
    type State = *const T;
    type Item<'storage> = &'storage T;

    fn visit_accesses(visitor: &mut dyn FnMut(TypeId, bool)) {
        visitor(TypeId::of::<T>(), false);
    }

    fn prepare(table: &Table) -> Option<Self::State> {
        table.shared_value_ptr::<T>()
    }

    unsafe fn fetch<'storage>(values: &Self::State, row: usize) -> Self::Item<'storage> {
        // SAFETY: Query construction validates the column type and row bounds. Shared projections may alias only
        // other shared projections of this field.
        unsafe { &*values.add(row) }
    }
}

impl<T: Field> sealed::Sealed for &mut T {}

impl<T: Field> QueryData for &mut T {
    type State = *mut T;
    type Item<'storage> = &'storage mut T;

    fn visit_accesses(visitor: &mut dyn FnMut(TypeId, bool)) {
        visitor(TypeId::of::<T>(), true);
    }

    fn prepare(table: &Table) -> Option<Self::State> {
        table.mutable_value_ptr::<T>()
    }

    unsafe fn fetch<'storage>(values: &Self::State, row: usize) -> Self::Item<'storage> {
        // SAFETY: Query construction validates the column type and row bounds. A query visits each row once and
        // rejects duplicate field types, so this mutable reference is unique.
        unsafe { &mut *values.add(row) }
    }
}

impl sealed::Sealed for Entity {}
impl ReadOnly for Entity {}

impl QueryData for Entity {
    type State = *const Entity;
    type Item<'storage> = Entity;

    fn visit_accesses(_visitor: &mut dyn FnMut(TypeId, bool)) {}

    fn prepare(table: &Table) -> Option<Self::State> {
        Some(table.entities_ptr())
    }

    unsafe fn fetch<'storage>(entities: &Self::State, row: usize) -> Self::Item<'storage> {
        // SAFETY: Query preparation captures the entity column and iteration guarantees `row` is in bounds.
        unsafe { *entities.add(row) }
    }
}

macro_rules! impl_query_data {
    ($($field:ident),+) => {
        #[allow(non_snake_case)]
        impl<$($field: QueryData),+> sealed::Sealed for ($($field,)+) {}

        #[allow(non_snake_case)]
        impl<$($field: QueryData),+> QueryData for ($($field,)+) {
            type State = ($($field::State,)+);
            type Item<'storage> = ($($field::Item<'storage>,)+);

            fn visit_accesses(visitor: &mut dyn FnMut(TypeId, bool)) {
                $($field::visit_accesses(visitor);)+
            }

            fn prepare(table: &Table) -> Option<Self::State> {
                Some(($($field::prepare(table)?,)+))
            }

            unsafe fn fetch<'storage>(
                state: &Self::State,
                row: usize,
            ) -> Self::Item<'storage> {
                let ($($field,)+) = state;
                // SAFETY: Query construction rejects duplicate field types, validates every column, and visits each
                // row once. Each pointer therefore produces references with the access declared by its projection.
                unsafe { ($($field::fetch($field, row),)+) }
            }
        }

        #[allow(non_snake_case)]
        impl<$($field: QueryData + ReadOnly),+> ReadOnly for ($($field,)+) {}
    };
}

impl_query_data!(A, B);
impl_query_data!(A, B, C);
impl_query_data!(A, B, C, D);
impl_query_data!(A, B, C, D, E);
impl_query_data!(A, B, C, D, E, F);

struct QueryTable<Q: QueryData> {
    len: usize,
    state: Q::State,
}

/// Typed access to every table row containing the requested fields.
///
/// A query supports shared (`&T`) and mutable (`&mut T`) field projections. Tuple projections return tuples in the
/// same order. Rows that do not contain every requested field are skipped.
pub struct Query<Q: QueryData> {
    query_marker: PhantomData<fn() -> Q>,
}

impl<Q: QueryData> Copy for Query<Q> {}

impl<Q: QueryData> Clone for Query<Q> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Q: QueryData> Query<Q> {
    /// Creates and validates a reusable query.
    pub fn new() -> Self {
        validate_accesses::<Q>().expect("a mutable query cannot project the same field type more than once");
        Self { query_marker: PhantomData }
    }

    /// Binds this query to exclusive storage access.
    pub fn query_mut<'storage>(&self, storage: &'storage mut Storage) -> QueryIter<'storage, Q> {
        self.bind(storage.get_tables_mut())
    }

    /// Binds this previously validated query to a table collection.
    fn bind<'storage>(&self, tables: &'storage Tables) -> QueryIter<'storage, Q> {
        QueryIter {
            tables: tables.iter(),
            entity_metadata: tables.entity_metadata(),
            current: None,
            row: 0,
            marker: PhantomData,
        }
    }
}

impl<Q: QueryData + ReadOnly> Query<Q> {
    /// Binds this query to shared storage access.
    pub fn query<'storage>(&self, storage: &'storage Storage) -> QueryIter<'storage, Q> {
        self.bind(storage.get_tables())
    }
}

impl<Q: QueryData> Default for Query<Q> {
    fn default() -> Self {
        Self::new()
    }
}

/// An iterator over projected query fields.
pub struct QueryIter<'storage, Q: QueryData> {
    tables: core::slice::Iter<'storage, Table>,
    entity_metadata: &'storage [EntityMeta],
    current: Option<QueryTable<Q>>,
    row: usize,
    marker: PhantomData<Q::Item<'storage>>,
}

impl<'storage, Q: QueryData> Iterator for QueryIter<'storage, Q> {
    type Item = Q::Item<'storage>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(table) = &self.current
                && self.row < table.len
            {
                let row = self.row;
                self.row += 1;
                // SAFETY: The matching state was prepared from this table and each row is yielded at most once.
                let item = unsafe { Q::fetch(&table.state, row) };
                return Some(item);
            }

            self.current =
                self.tables.find_map(|table| Q::prepare(table).map(|state| QueryTable { len: table.len(), state }));
            self.row = 0;
            self.current.as_ref()?;
        }
    }
}

impl<'storage, Q: QueryData> QueryIter<'storage, Q> {
    /// Returns the projected fields for `entity` if this iterator has not started.
    pub fn get(self, entity: &Entity) -> Option<Q::Item<'storage>> {
        if self.current.is_some() || self.row != 0 {
            return None;
        }

        let location = entity_location(self.entity_metadata, *entity)?;
        let table = self.tables.as_slice().get(location.table)?;
        let state = Q::prepare(table)?;
        // SAFETY: Entity metadata identifies an in-bounds row in this table, and the query state was prepared from it.
        Some(unsafe { Q::fetch(&state, location.row) })
    }
}

/// Rejects repeated field types when any projection of that field is mutable.
fn validate_accesses<Q: QueryData>() -> Result<(), &'static str> {
    let mut invalid = false;
    Q::visit_accesses(&mut |type_id, mutable| {
        let mut count = 0;
        let mut has_mutable_access = mutable;
        Q::visit_accesses(&mut |other, other_mutable| {
            if other == type_id {
                count += 1;
                has_mutable_access |= other_mutable;
            }
        });
        invalid |= count > 1 && has_mutable_access;
    });
    if invalid { Err("a mutable query cannot project the same field type more than once") } else { Ok(()) }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct A(u32);

    fn clone_value<T: Clone>(value: &T) -> T {
        value.clone()
    }

    #[test]
    fn reusable_query_supports_default_clone_and_copy() {
        let query = Query::<&A>::default();
        let cloned = clone_value(&query);
        let copied = query;
        let mut storage = Storage::new();
        storage.get_tables_mut().insert_new((A(1),));

        assert_eq!(cloned.query_mut(&mut storage).next(), Some(&A(1)));
        assert_eq!(copied.query_mut(&mut storage).next(), Some(&A(1)));
    }

    #[test]
    fn entity_projection_does_not_conflict_with_stored_entity_fields() {
        let mut storage = Storage::new();
        let stored = storage.get_tables_mut().insert_new(());
        let row = storage.get_tables_mut().insert_new((stored,));

        let (projected, value) = Query::<(Entity, &mut Entity)>::new().query_mut(&mut storage).next().unwrap();

        assert_eq!(projected, row);
        assert_eq!(*value, stored);
    }
}
