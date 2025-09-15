//! A module for defining the [Hob] [Param] type and default implementations for the [FromHob] trait.
//!
//! This module contains the definitions for [Hob] and the [FromHob] trait. The [Hob] type is a new dependency
//! injectable [Param] implementation that allows components to access read-only HOB (Hand off Block) values. See the
//! types for more documentation.
//!
//! The [FromHob] trait is used to parse guided HOBs as specified in the PI specification.
//!
//! ## Example
//!
//! ```rust
//! use patina_sdk::{
//!    error::Result,
//!    component::hob::{Hob, FromHob},
//! };
//!
//! /// A HOB that is a simple pointer cast from byte array to a struct.
//! #[derive(Default, Clone, Copy, FromHob)]
//! #[hob = "8be4df61-93ca-11d2-aa0d-00e098032b8c"]
//! #[repr(C)]
//! struct MyHobStruct {
//!    field1: u32,
//!    field2: u32
//! }
//!
//! /// A Hob that requires more complex parsing logic.
//! #[derive(Default)]
//! struct MyComplexHobStruct {
//!     field1: String,
//!     field2: [u8; 16],
//!     field3: String,
//!     field4: Vec<MyHobStruct>
//! }
//!
//! impl FromHob for MyComplexHobStruct {
//!     const HOB_GUID: r_efi::efi::Guid = r_efi::efi::Guid::from_fields(0, 0, 0, 0, 0, &[0; 6]);
//!
//!    fn parse(bytes: &[u8]) -> Self {
//!        Self::default() // Simple for example
//!    }
//! }
//!
//! /// A component that will only run if the HOB was produced.
//! pub fn my_component(_hob: Hob<MyComplexHobStruct>) -> Result<()> {
//!     Ok(())
//! }
//!
//! /// A component that will always run, but will be None if the HOB was not produced.
//! fn my_other_component(_hob: Option<Hob<MyHobStruct>>) -> Result<()> {
//!     Ok(())
//! }
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
extern crate alloc;

use alloc::{boxed::Box, vec::Vec};

use core::{any::Any, ops::Deref};
use r_efi::efi::Guid;

use super::{
    metadata::MetaData,
    params::Param,
    storage::{Storage, UnsafeStorageCell},
};

/// A trait for automatically parsing guided HOBs to dependency injectable types.
///
/// The actual parsing of the byte array is done by the implementor via the `parse` method. The implementor has
/// the freedom to parse the byte array in anyway they see fit. It could be as simple as casting the byte array to a
/// struct or a more complex parsing process.
///
/// This trait is used to parse guided HOBs as specified in the PI specification.
///
/// ## Example
///
/// ```rust
/// use patina_sdk::component::hob::FromHob;
///
/// #[derive(Default, Clone, Copy)]
/// #[repr(C)]
/// struct MyConfig {
///     field1: u32,
///     field2: u32,
/// }
///
/// impl FromHob for MyConfig {
///     const HOB_GUID: r_efi::efi::Guid = r_efi::efi::Guid::from_fields(0, 0, 0, 0, 0, &[0; 6]);
///
///     fn parse(bytes: &[u8]) -> Self {
///         // SAFETY: Specification defined requirement that the byte array is this underlying C type.
///         unsafe { *(bytes.as_ptr() as *const Self) }
///     }
/// }
///
/// #[derive(FromHob, Default, Clone, Copy)]
/// #[hob = "8be4df61-93ca-11d2-aa0d-00e098032b8c"]
/// #[repr(C)]
/// struct MyConfig2 {
///    field1: u32,
///    field2: u32,
/// }
/// ```
pub trait FromHob: Sized + 'static {
    /// The guid value associated with the guided HOB to parse.
    const HOB_GUID: Guid;

    /// Registers the parsed hob with the provided [Storage] instance.
    fn register(bytes: &[u8], storage: &mut Storage) {
        storage.add_hob(Self::parse(bytes));
    }

    /// Parses the byte array into the type implementing this trait.
    fn parse(bytes: &[u8]) -> Self;
}

pub use patina_sdk_macro::FromHob;

/// An immutable Hob value registered with [Storage] via the [FromHob] trait.
///
/// The underlying datum of this type is a slice. The first element of the slice can be directly accessed by
/// dereferencing the struct. The entire slice can be iterated over using the [Hob::iter] method or the [IntoIterator]
/// trait.
///
/// ## Example
///
/// ```rust
/// # use patina_sdk::component::hob::{FromHob, Hob};
/// # #[derive(Debug)]
/// # struct MyStruct{ value: u32 };
/// # impl FromHob for MyStruct {
/// #     const HOB_GUID: r_efi::efi::Guid = r_efi::efi::Guid::from_fields(0, 0, 0, 0, 0, &[0; 6]);
/// #     fn parse(bytes: &[u8]) -> Self {
/// #         MyStruct { value: 5 }
/// #     }
/// # }
/// let hob = Hob::mock(vec![MyStruct{ value: 5 }, MyStruct{ value: 10 }]);
/// let first_value = hob.value; // Deref allows to directly access the first value
///
/// // Access all values
/// for value in hob.iter() {
///    println!("{:?}", value); // Iterate over the values
/// }
/// ```
pub struct Hob<'h, T: FromHob + 'static> {
    value: &'h [Box<dyn Any>],
    _marker: core::marker::PhantomData<T>,
}

impl<'h, T: FromHob + 'static> Hob<'h, T> {
    /// Creates an instance of Hob by leaking the provided value into static memory.
    ///
    /// This function is intended for testing purposes only. Dropping the returned value will cause a memory leak as
    /// the underlying (leaked) value will cannot be deallocated.
    ///
    /// ## Example
    ///
    /// ```rust
    /// use patina_sdk::component::hob::{FromHob, Hob};
    /// use r_efi::efi::Guid;
    ///
    /// struct MyStruct;
    ///
    /// impl FromHob for MyStruct {
    ///     const HOB_GUID: Guid = Guid::from_fields(0, 0, 0, 0, 0, &[0; 6]);
    ///
    ///    fn parse(bytes: &[u8]) -> Self {
    ///        MyStruct
    ///    }
    /// }
    ///
    /// fn my_component_to_test(hob: Hob<MyStruct>) {
    ///    // Use the hob value here
    /// }
    ///
    /// #[test]
    /// fn test_my_component() {
    ///     let hob = Hob::mock(vec![MyStruct]);
    ///     my_component_to_test(hob);
    /// }
    /// ```
    #[allow(clippy::test_attr_in_doctest)]
    pub fn mock(value: Vec<T>) -> Self {
        let value = value.into_iter().map(|v| Box::new(v) as Box<dyn Any>).collect::<Vec<_>>();
        let static_value = Box::leak(value.into_boxed_slice());

        Self { value: static_value, _marker: core::marker::PhantomData }
    }

    /// Returns an iterator over the values of the Hob.
    pub fn iter(&self) -> HobIter<'h, T> {
        HobIter { inner: self.value.iter(), _marker: core::marker::PhantomData }
    }
}

impl<'h, T: FromHob + 'static> From<&'h [Box<dyn Any>]> for Hob<'h, T> {
    fn from(value: &'h [Box<dyn Any>]) -> Self {
        Self { value, _marker: core::marker::PhantomData }
    }
}

impl<T: FromHob + 'static> Deref for Hob<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value[0].downcast_ref::<T>().expect("Failed to downcast Hob value")
    }
}

unsafe impl<T: FromHob + 'static> Param for Hob<'_, T> {
    type State = usize;
    type Item<'storage, 'state> = Hob<'storage, T>;

    unsafe fn get_param<'storage, 'state>(
        lookup_id: &'state Self::State,
        storage: UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state> {
        Hob::from(unsafe { storage.storage().get_raw_hob(*lookup_id) })
    }

    fn validate(state: &Self::State, storage: UnsafeStorageCell) -> bool {
        // SAFETY: accesses are correctly regsitered with storage, no conflicts
        !unsafe { storage.storage() }.get_raw_hob(*state).is_empty()
    }

    fn init_state(storage: &mut Storage, _meta: &mut MetaData) -> Self::State {
        storage.add_hob_parser::<T>();
        storage.register_hob::<T>()
    }
}

/// An iterator of the underlying values of the Hob.
///
/// ## Example
///
/// ```rust
/// # use patina_sdk::component::hob::{FromHob, Hob};
/// # struct MyStruct(u32);
/// # impl FromHob for MyStruct {
/// #     const HOB_GUID: r_efi::efi::Guid = r_efi::efi::Guid::from_fields(0, 0, 0, 0, 0, &[0; 6]);
/// #     fn parse(bytes: &[u8]) -> Self {
/// #         MyStruct(5)
/// #     }
/// # }
/// # let hob = Hob::mock(vec![MyStruct(5), MyStruct(10)]);
/// let mut hob_iter = hob.iter();
/// for value in hob_iter {
///   // Do something with the value
/// }
/// ```
pub struct HobIter<'h, T> {
    inner: core::slice::Iter<'h, Box<dyn Any>>,
    _marker: core::marker::PhantomData<T>,
}

impl<'h, T: FromHob + 'static> Iterator for HobIter<'h, T> {
    type Item = &'h T;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(any_box) = self.inner.next() {
            return Some(
                any_box
                    .downcast_ref::<T>()
                    .unwrap_or_else(|| panic!("Hob should be of type {}", core::any::type_name::<T>())),
            );
        }
        None
    }
}

impl<'h, T: FromHob + 'static> IntoIterator for &Hob<'h, T> {
    type Item = &'h T;
    type IntoIter = HobIter<'h, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use crate as patina_sdk;
    use crate::{
        component::IntoComponent,
        error::{EfiError, Result},
    };

    use super::*;

    #[derive(Default)]
    struct MyStruct {
        unused: u32,
    }

    impl FromHob for MyStruct {
        const HOB_GUID: Guid = Guid::from_fields(0, 0, 0, 0, 0, &[0; 6]);

        fn parse(_bytes: &[u8]) -> Self {
            MyStruct::default()
        }
    }

    #[test]
    fn test_single_hob() {
        let mut storage = Storage::new();

        let id = storage.register_hob::<MyStruct>();
        storage.add_hob(MyStruct { unused: 5 });

        let hob: Hob<MyStruct> = Hob::from(storage.get_raw_hob(id));
        assert_eq!(hob.unused, 5);
    }

    #[test]
    fn test_multiple_hobs() {
        let mut storage = Storage::new();

        let id1 = storage.register_hob::<MyStruct>();

        storage.add_hob(MyStruct { unused: 5 });
        storage.add_hob(MyStruct { unused: 10 });

        let hobs: Hob<MyStruct> = Hob::from(storage.get_raw_hob(id1));

        {
            let mut iter = hobs.iter();

            assert_eq!(iter.next().unwrap().unused, 5);
            assert_eq!(iter.next().unwrap().unused, 10);
        }

        for hob in &hobs {
            assert!([5, 10].contains(&hob.unused))
        }
    }

    #[test]
    fn test_iter_next_function() {
        let hobs = Hob::mock(vec![MyStruct { unused: 5 }, MyStruct { unused: 10 }]);
        let mut iter = hobs.iter();

        assert_eq!(iter.next().unwrap().unused, 5);
        assert_eq!(iter.next().unwrap().unused, 10);
        assert!(iter.next().is_none()); // No more elements
    }

    #[test]
    fn test_component_flow() {
        #[derive(IntoComponent)]
        #[entry_point(path = my_component)]
        struct MyComponent;

        fn my_component(_: MyComponent, hob: Hob<MyStruct>) -> Result<()> {
            if hob.unused == 0 {
                return Err(EfiError::NotReady);
            }
            if hob.unused != 5 {
                return Err(EfiError::InvalidParameter);
            }
            Ok(())
        }

        let mut storage = Storage::new();

        let mut comp = MyComponent.into_component();
        comp.initialize(&mut storage);

        assert!(!Hob::<MyStruct>::validate(&0, UnsafeStorageCell::from(&storage)));
        MyStruct::register(&[0], &mut storage);
        assert!(Hob::<MyStruct>::validate(&0, UnsafeStorageCell::from(&storage)));

        let x = unsafe { Hob::<MyStruct>::get_param(&0, UnsafeStorageCell::from(&storage)) };

        assert!(my_component(MyComponent, x).is_err_and(|e| e == EfiError::NotReady));

        assert!(my_component(MyComponent, Hob::mock(vec![MyStruct { unused: 5 }])).is_ok());
        assert!(
            my_component(MyComponent, Hob::mock(vec![MyStruct { unused: 10 }]))
                .is_err_and(|e| e == EfiError::InvalidParameter)
        );
    }
}
