//! This module provides an implementation of a global allocator using UEFI Boot Services.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{
    alloc::{GlobalAlloc, Layout},
    ops::Deref,
    ptr,
};

use crate::{boot_services::BootServices, efi_types::EfiMemoryType};

pub struct BootServicesGlobalAllocator<T: BootServices + 'static>(pub &'static T);

impl<T: BootServices> Deref for BootServicesGlobalAllocator<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<T: BootServices> BootServicesGlobalAllocator<T> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match layout.align() {
            0..=8 => self.allocate_pool(EfiMemoryType::BootServicesData, layout.size()).unwrap_or(ptr::null_mut()),
            _ => {
                let Ok((extended_layout, tracker_offset)) = layout.extend(Layout::new::<*mut *mut u8>()) else {
                    return ptr::null_mut();
                };
                let alloc_size = extended_layout.align() + extended_layout.size();
                let Ok(original_ptr) = self.allocate_pool(EfiMemoryType::BootServicesData, alloc_size) else {
                    return ptr::null_mut();
                };
                let ptr = unsafe { original_ptr.add(original_ptr.align_offset(extended_layout.align())) };
                let tracker_ptr = unsafe { ptr.add(tracker_offset) as *mut *mut u8 };
                unsafe { ptr::write(tracker_ptr, original_ptr) };
                ptr
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        match layout.align() {
            0..=8 => _ = self.free_pool(ptr),
            _ => {
                let Ok((extended_layout, tracker_offset)) = layout.extend(Layout::new::<*mut *mut u8>()) else {
                    return;
                };
                let tracker_ptr = unsafe { ptr.add(tracker_offset) as *mut *mut u8 };
                let original_ptr = unsafe { ptr::read(tracker_ptr) };
                debug_assert_eq!(ptr, unsafe { original_ptr.add(original_ptr.align_offset(extended_layout.align())) });
                let _ = self.free_pool(original_ptr);
            }
        }
    }
}

unsafe impl<T: BootServices> GlobalAlloc for BootServicesGlobalAllocator<T> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { BootServicesGlobalAllocator::alloc(self, layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { BootServicesGlobalAllocator::dealloc(self, ptr, layout) }
    }
}
