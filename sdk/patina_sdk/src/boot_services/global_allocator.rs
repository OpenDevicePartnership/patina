use core::{
    alloc::{GlobalAlloc, Layout},
    ops::Deref,
    ptr,
};

use crate::boot_services::{allocation::MemoryType, BootServices};

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
            0..=8 => self.allocate_pool(MemoryType::BOOT_SERVICES_DATA, layout.size()).unwrap_or(ptr::null_mut()),
            _ => {
                let Ok((extended_layout, tracker_offset)) = layout.extend(Layout::new::<*mut *mut u8>()) else {
                    return ptr::null_mut();
                };
                let alloc_size = extended_layout.align() + extended_layout.size();
                let Ok(original_ptr) = self.allocate_pool(MemoryType::BOOT_SERVICES_DATA, alloc_size) else {
                    return ptr::null_mut();
                };
                let ptr = original_ptr.add(original_ptr.align_offset(extended_layout.align()));
                let tracker_ptr = ptr.add(tracker_offset) as *mut *mut u8;
                ptr::write(tracker_ptr, original_ptr);
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
                let tracker_ptr = ptr.add(tracker_offset) as *mut *mut u8;
                let original_ptr = ptr::read(tracker_ptr);
                debug_assert_eq!(ptr, original_ptr.add(original_ptr.align_offset(extended_layout.align())));
                let _ = self.free_pool(original_ptr);
            }
        }
    }
}

unsafe impl<T: BootServices> GlobalAlloc for BootServicesGlobalAllocator<T> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        BootServicesGlobalAllocator::alloc(self, layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        BootServicesGlobalAllocator::dealloc(self, ptr, layout)
    }
}
