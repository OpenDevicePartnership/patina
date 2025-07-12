//! This module provides C pointer abstractions for safe pointer handling in Rust.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::boxed::Box;
use core::{
    ffi::c_void,
    marker::PhantomData,
    mem::{self, ManuallyDrop},
    ops::Deref,
    ptr,
};

#[derive(Copy)]
/// Ptr metadata is a struct that hold everything to rebuild the original pointer from this metadata.
pub struct PtrMetadata<'a, T> {
    /// The pointer value as a usize
    pub ptr_value: usize,
    _container: PhantomData<&'a T>,
}

impl<T> Clone for PtrMetadata<'_, T> {
    fn clone(&self) -> Self {
        Self { ptr_value: self.ptr_value, _container: PhantomData }
    }
}

impl<'a, R: CPtr<'a, Type = T>, T> PtrMetadata<'a, R> {
    /// retrieve the original pointer from the pointer metadata
    ///
    /// # Safety
    /// Caller must ensure that the pointed-to memory is still valid and uphold rust pointer invariants (e.g. no aliasing)
    pub unsafe fn into_original_ptr(self) -> R {
        unsafe { mem::transmute_copy(&self.ptr_value) }
    }
}

/// Trait for type that can be used as pointer.
///
/// # Safety
///
/// Implementers must ensure that rust invariants around pointer usage (e.g. no aliasing) are respected.
pub unsafe trait CPtr<'a>: Sized {
    /// The type of the pointed-to value.
    type Type: Sized;

    /// Returns a pointer to the underlying data.
    fn as_ptr(&self) -> *const Self::Type;

    /// Consumes the `CPtr`, returning a raw pointer to the underlying data.
    fn into_ptr(self) -> *const Self::Type {
        let this = ManuallyDrop::new(self);
        this.as_ptr()
    }

    /// Returns a [PtrMetadata] that maintains the underlying lifetime and type information.
    fn metadata(&self) -> PtrMetadata<'a, Self> {
        PtrMetadata { ptr_value: self.as_ptr() as usize, _container: PhantomData }
    }
}

/// Trait for type that can be used as mutable pointer.
///
/// # Safety
///
/// implementers must ensure that rust invariants around pointer usage (e.g. no aliasing) are respected.
pub unsafe trait CMutPtr<'a>: CPtr<'a> {
    /// Returns a mutable pointer to the underlying data.
    fn as_mut_ptr(&mut self) -> *mut Self::Type {
        <Self as CPtr>::as_ptr(self) as *mut _
    }

    /// Consumes the `CMutPtr`, returning a mutable raw pointer to the underlying data.
    fn into_mut_ptr(self) -> *mut Self::Type {
        let mut this = ManuallyDrop::new(self);
        this.as_mut_ptr()
    }
}

// Trait for type that can be used as pointer and that can not be null.
///
/// # Safety
///
/// implementers must ensure that rust invariants around pointer usage (e.g. no aliasing) are respected.
pub unsafe trait CRef<'a>: CPtr<'a> {
    /// Returns a reference to the pointed-to value.
    fn as_ref(&self) -> &Self::Type {
        unsafe { self.as_ptr().as_ref().unwrap() }
    }
}

/// Trait for type that can be used as mutable pointer and that can not be null.
///
/// # Safety
///
/// implementers must ensure that rust invariants around pointer usage (e.g. no aliasing) are respected.
pub unsafe trait CMutRef<'a>: CRef<'a> + CMutPtr<'a> {
    /// Returns a mutable reference to the pointed-to value.
    fn as_mut(&mut self) -> &mut Self::Type {
        unsafe { self.as_mut_ptr().as_mut().unwrap() }
    }
}

// *const T
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<T> CPtr<'static> for *const T {
    type Type = T;

    /// Returns a pointer to the underlying data.
    fn as_ptr(&self) -> *const Self::Type {
        *self
    }
}

// *mut T
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<T> CPtr<'static> for *mut T {
    type Type = T;

    /// Returns a pointer to the underlying data.
    fn as_ptr(&self) -> *const Self::Type {
        *self
    }
}
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<T> CMutPtr<'static> for *mut T {}

// &T
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<'a, T> CPtr<'a> for &'a T {
    type Type = T;

    /// Returns a pointer to the underlying data.
    fn as_ptr(&self) -> *const Self::Type {
        *self as *const _
    }
}
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<'a, T> CRef<'a> for &'a T {}

// &mut T
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<'a, T> CPtr<'a> for &'a mut T {
    type Type = T;

    /// Returns a pointer to the underlying data.
    fn as_ptr(&self) -> *const Self::Type {
        *self as *const _
    }
}
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<'a, T> CRef<'a> for &'a mut T {}
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<'a, T> CMutPtr<'a> for &'a mut T {}
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<'a, T> CMutRef<'a> for &'a mut T {}

// Box<T>
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<T> CPtr<'_> for Box<T> {
    type Type = T;

    fn as_ptr(&self) -> *const Self::Type {
        AsRef::as_ref(self) as *const _
    }
}
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<T> CRef<'_> for Box<T> {}
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<T> CMutPtr<'_> for Box<T> {}
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<T> CMutRef<'_> for Box<T> {}

// ()
// SAFETY: Use null pointer wich is a valid c pointer.
unsafe impl CPtr<'static> for () {
    type Type = c_void;

    fn as_ptr(&self) -> *const Self::Type {
        ptr::null()
    }
}

// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl CMutPtr<'static> for () {
    fn as_mut_ptr(&mut self) -> *mut Self::Type {
        ptr::null_mut()
    }
}

// Option<T>
unsafe impl<'a, R: CPtr<'a, Type = T>, T> CPtr<'a> for Option<R> {
    type Type = T;

    fn as_ptr(&self) -> *const Self::Type {
        self.as_ref().map_or(ptr::null(), |p| p.as_ptr())
    }
}
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<'a, R: CMutPtr<'a, Type = T>, T> CMutPtr<'a> for Option<R> {}

// ManuallyDrop<T>
unsafe impl<'a, R: CPtr<'a, Type = T>, T> CPtr<'a> for ManuallyDrop<R> {
    type Type = T;

    fn as_ptr(&self) -> *const Self::Type {
        <R as CPtr>::as_ptr(self.deref())
    }
}
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<'a, R: CMutPtr<'a, Type = T>, T> CMutPtr<'a> for ManuallyDrop<R> {}
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<'a, R: CRef<'a, Type = T>, T> CRef<'a> for ManuallyDrop<R> {}
// SAFETY: Memory layout and mutability are respected for these types.
unsafe impl<'a, R: CMutRef<'a, Type = T>, T> CMutRef<'a> for ManuallyDrop<R> {}

#[cfg(test)]
mod test {
    use core::ptr;

    use super::*;

    #[test]
    fn test_ref() {
        let mut foo = 10;
        let ptr = ptr::addr_of!(foo);

        assert_eq!(ptr, (&foo).as_ptr());
        assert_eq!(ptr, (&mut foo).as_mut_ptr());

        assert_eq!(ptr, (&foo).as_ref() as *const _);
        assert_eq!(ptr, (&mut foo).as_mut() as *const _);

        assert_eq!(ptr, (&mut foo).into_ptr());
        let mut foo = 10;
        let ptr = ptr::addr_of!(foo);
        assert_eq!(ptr, (&mut foo).into_mut_ptr());
    }

    #[test]
    fn test_box() {
        let b = Box::new(10);
        let b_ptr = ptr::from_ref(<Box<_> as AsRef<_>>::as_ref(&b));

        assert_eq!(b_ptr, CPtr::as_ptr(&b));
        assert_eq!(b_ptr, CPtr::into_ptr(b));

        // Box should leak with into_ptr
        let mut b = unsafe { Box::from_raw(b_ptr as *mut i32) };
        assert_eq!(&10, <Box<_> as AsRef<_>>::as_ref(&b));

        assert_eq!(b_ptr, CMutPtr::as_mut_ptr(&mut b));
        assert_eq!(b_ptr, CMutPtr::into_mut_ptr(b));
    }

    #[test]
    fn test_unit_type() {
        assert_eq!(ptr::null(), ().as_ptr());
        assert_eq!(ptr::null_mut(), ().as_mut_ptr());
    }

    #[test]
    fn test_option() {
        assert_eq!(ptr::null(), (Option::<Box<i32>>::None).as_ptr());
        assert_eq!(ptr::null_mut(), (Option::<Box<i32>>::None).as_mut_ptr());

        let b = Box::new(10);
        let ptr = b.as_ptr();
        assert_eq!(ptr, Some(b).as_ptr());

        let b = Box::new(10);
        let ptr = b.as_ptr();
        assert_eq!(ptr, Some(b).as_mut_ptr());

        let b = Box::new(10);
        let ptr = b.as_ptr();
        assert_eq!(ptr, Some(b).into_ptr());

        let b = Box::new(10);
        let ptr = b.as_ptr();
        assert_eq!(ptr, Some(b).into_mut_ptr());
    }

    #[test]
    fn test_manually_drop() {
        let b = Box::new(10);
        let ptr = b.as_ptr();
        let mut mdb = ManuallyDrop::new(b);
        assert_eq!(ptr, mdb.as_ptr());
        assert_eq!(ptr, mdb.as_mut_ptr());
        assert_eq!(ptr, mdb.into_ptr());

        let mdb = ManuallyDrop::new(unsafe { Box::from_raw(ptr as *mut i32) });
        assert_eq!(ptr, mdb.into_mut_ptr());

        assert_eq!(ptr::null(), ManuallyDrop::new(()).as_ptr());
        assert_eq!(ptr::null_mut(), ManuallyDrop::new(()).as_mut_ptr());
    }
}
