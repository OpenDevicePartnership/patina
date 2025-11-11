//! This module provides a Box type whose lifetime is tied to the UEFI Boot Services.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::slice;
use core::{
    mem,
    ops::{Deref, DerefMut},
    ptr,
};

use super::BootServices;
use crate::efi_types::EfiMemoryType;

/// A boxed type to wrap a [BootServices] implementation
#[derive(Debug)]
pub struct BootServicesBox<'a, T: ?Sized, B: BootServices + ?Sized> {
    ptr: *mut T,
    boot_services: &'a B,
}

impl<'a, T, B: BootServices> BootServicesBox<'a, T, B> {
    /// Create a new BootServicesBox containing the provided value
    pub fn new(value: T, memory_type: EfiMemoryType, boot_services: &'a B) -> Self {
        let size = mem::size_of_val(&value);
        let ptr = boot_services.allocate_pool(memory_type, size).unwrap() as *mut T;
        unsafe { ptr::write(ptr, value) };
        Self { boot_services, ptr }
    }

    /// Create a BootServicesBox from the provided raw pointer
    ///
    /// # Safety
    /// ptr must be valid, and must be legal to call boot_services::free_pool(ptr). The easiest way to guarantee this
    /// is to only use from_raw on pointers created by BootServicesBox::into_raw* functions.
    pub unsafe fn from_raw(ptr: *mut T, boot_services: &'a B) -> Self {
        Self { boot_services, ptr }
    }

    /// Consumes the `BootServicesBox`, returning a raw pointer to the underlying data.
    pub fn into_raw(self) -> *const T {
        self.ptr as *const T
    }

    /// Consumes the `BootServicesBox`, returning a mutable raw pointer to the underlying data.
    pub fn into_raw_mut(self) -> *mut T {
        self.ptr
    }

    /// Leaks the box, such that the memory will not be freed even if all references to it are dropped.
    pub fn leak(self) -> &'a mut T {
        let leak = unsafe { self.ptr.as_mut() }.unwrap();
        mem::forget(self);
        leak
    }
}

impl<'a, T, B: BootServices> BootServicesBox<'a, [T], B> {
    /// Create a boot services box from raw pointer and length.
    ///
    /// # Safety
    ///
    /// Caller must ensure that the pointer and len are correct and that rust pointer invariants (e.g. no aliasing) are respected.
    pub unsafe fn from_raw_parts_mut(ptr: *mut T, len: usize, boot_services: &'a B) -> Self {
        let ptr = unsafe { slice::from_raw_parts_mut(ptr, len) };
        Self { boot_services, ptr }
    }
}

impl<T: ?Sized, B: BootServices + ?Sized> Drop for BootServicesBox<'_, T, B> {
    fn drop(&mut self) {
        let _ = self.boot_services.free_pool(self.ptr as *mut u8);
    }
}

impl<T: ?Sized, B: BootServices> Deref for BootServicesBox<'_, T, B> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }.unwrap()
    }
}

impl<T: ?Sized, B: BootServices> DerefMut for BootServicesBox<'_, T, B> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.ptr.as_mut() }.unwrap()
    }
}

impl<T: ?Sized, B: BootServices> AsRef<T> for BootServicesBox<'_, T, B> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}

impl<T: ?Sized, B: BootServices> AsMut<T> for BootServicesBox<'_, T, B> {
    fn as_mut(&mut self) -> &mut T {
        self.deref_mut()
    }
}
