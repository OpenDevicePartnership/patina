#![no_std]

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicUsize, Ordering};

pub struct BumpAllocator {
    free_memory_top: AtomicUsize, // Top of free memory
    bump_ptr: AtomicUsize,        // Atomic bump pointer
    lock: AtomicUsize,            // Spinlock for synchronizing memory allocations
}

impl BumpAllocator {
    /// Creates a new bump allocator for a specific memory range
    pub const fn new() -> Self {
        Self {
            free_memory_top: AtomicUsize::new(0),
            bump_ptr: AtomicUsize::new(0),
            lock: AtomicUsize::new(0), // 0 indicates unlocked
        }
    }

    // the idea is that we won't know free memory ranges until we read the PHIT HOB
    // so there will be a brief moment between program initialization / reading the PHIT hob
    // in which we cannot use the allocator
    // thus, init needs to be called after reading the PHIT hob, before we can have memory allocations
    pub fn init(&mut self, free_memory_bottom: usize, free_memory_top: usize) {
        if !ALLOCATOR_INITIALIZED.load(Ordering::SeqCst) {
            self.bump_ptr.store(free_memory_bottom, Ordering::SeqCst);
            self.free_memory_top.store(free_memory_top, Ordering::SeqCst); // i don't actually know if this one needs to be atomic but wtv
        } else {
            panic!("Allocator already initialized");
        }
    }

    // this necessarily only allows one allocation to happen at at time
    // if other threads try to allocate they'll spin until the allocating thread is done
    // i don't know if thread safety is actually necessary here but if it's not we can take out the spinlock/atomic stuff later
    pub fn alloc(&self, layout: Layout) -> *mut u8 {
        if !ALLOCATOR_INITIALIZED.load(Ordering::SeqCst) {
            panic!("Allocator not initialized");
        }

        let size = layout.size();
        let align = layout.align();

        loop {
            // Try to acquire the spinlock (lock == 0 means unlocked)
            if self.lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                let current = self.bump_ptr.load(Ordering::Relaxed);

                // Align the allocation address
                let aligned = (current + align - 1) & !(align - 1);
                let next = aligned + size;

                // Check if we have enough space
                if next > self.free_memory_top.load(Ordering::Relaxed) {
                    self.lock.store(0, Ordering::Release); // Release the lock
                    panic!("Out of memory");
                    // at least as of now we have no way to recover from the allocator
                    // being out of memory since it doesn't ever deallocate. so we just panic
                }

                // Try to atomically update the bump pointer
                if self.bump_ptr.compare_exchange(current, next, Ordering::SeqCst, Ordering::Relaxed).is_ok() {
                    self.lock.store(0, Ordering::Release); // Release the lock
                    return aligned as *mut u8;
                }

                // If allocation failed, release the lock
                self.lock.store(0, Ordering::Release);
            }

            // If we couldn't acquire the lock, keep trying
        }
    }
}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.alloc(layout)
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // No dealloc (is this okay?)
    }
}

// Flag for whether allocator is initialized (should only happen once)
static ALLOCATOR_INITIALIZED: AtomicBool = AtomicBool::new(false);

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator::new();
