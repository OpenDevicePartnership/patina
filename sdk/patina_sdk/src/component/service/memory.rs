//! Memory Related Service Defintions.
//!
//! This module contains traits and types for services related memory management
//! and access for use in services. See [MemoryManager] for the primary interface.
//!
//! ## Testing
//!
//! This module contains a std implementation of the [MemoryManager] trait called `StdMemoryManager` to enable testing.
//! It provides a fully working implementation of the [MemoryManager] based off of the std global allocator.
//!
//! ```rust
//! use patina_sdk::component::service::{ Service, memory::*};
//!
//! use std::boxed::Box;
//!
//! fn test_that_needs_memory_manager() {
//!     let memory_manager = StdMemoryManager::new();
//!
//!     let service: Service<dyn MemoryManager> = Service::mock(Box::new(memory_manager));
//! }
//! ```
//!
//! As always, if you need a fallable mock for testing your code's ability to handle allocation errors, A `mockall` mock
//! is available for you to use (`MockMemoryManager`).
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use core::{
    mem::{ManuallyDrop, MaybeUninit},
    ptr::{NonNull, with_exposed_provenance_mut},
};

use r_efi::efi;

use crate::{base::UEFI_PAGE_SIZE, efi_types::EfiMemoryType, error::EfiError};

#[cfg(any(test, feature = "alloc"))]
use core::alloc::Allocator;

#[cfg(any(test, feature = "alloc"))]
use alloc::boxed::Box;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// The `MemoryManager` trait provides an interface for allocating, freeing,
/// and manipulating access to memory. This trait is intended to be implemented
/// by the core and serve as the API by which both internal code and external
/// components can access memory services.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait MemoryManager {
    /// Allocates pages of memory.
    ///
    /// Allocates the specified number of pages of the memory type requested.
    /// The [`UEFI_PAGE_SIZE`] constant  should be used for referencing the page
    /// size.
    ///
    /// Allocations made through the this function are not tracked or automatically
    /// freed. It is the caller's responsibility to track and free the allocated pages.
    ///
    /// Allocated pages will by default have the [`AccessType::ReadWrite`] access
    /// attribute. The attributes may be changed using the `set_page_attributes`
    /// function. Even if the [`AccessType::NoAccess`] is set, this page will not
    /// be considered freed until the `free_pages` function is called.
    ///
    /// The return type of page allocations as a [`PageAllocation`] structure. This
    /// services as a tracker for the page allocation that can be converted into
    /// either a direct pointer or a managed type. See [`PageAllocation`] for more
    /// details.
    ///
    /// # Parameters
    ///
    /// - `pages_count`: The number of pages to allocate.
    /// - `options`: The [`AllocationOptions`] to use for the allocation.
    ///
    /// # Returns
    ///
    /// - `Ok(PageAllocation)` if the allocation was successful. the [`PageAllocation`]
    ///   which must then be converted into a usable type to access the allocation.
    /// - `Err(MemoryError)` if the allocation failed. See [`MemoryError`] for
    ///   more details on the error.
    ///
    /// # Example
    ///
    /// Page allocations made with `allocate_pages` can then be used in several
    /// different ways. More information on these can be found in [`PageAllocation`].
    ///
    /// ```rust
    /// #![cfg_attr(feature = "alloc", feature(allocator_api))]
    /// # use patina_sdk::{efi_types::*, component::service::memory::*};
    ///
    /// fn component(memory_manager: &dyn MemoryManager) -> Result<(), MemoryError> {
    ///     // Allocate a page of memory and leak it.
    ///     let alloc = memory_manager.allocate_pages(1, AllocationOptions::new())?;
    ///     let static_u64 = alloc.leak_as(42).unwrap();
    ///
    ///     // Allocate a page and convert it into a Box.
    ///     let alloc = memory_manager.allocate_pages(1, AllocationOptions::new())?;
    ///     #[cfg(feature = "alloc")]
    ///     let boxed_value = alloc.into_box(42).unwrap();
    ///     // Memory will be safely freed when the Box is dropped.
    ///
    ///     // Allocate a page of memory and manually manage it, making sure
    ///     // to free it.
    ///     let alloc = memory_manager.allocate_pages(1, AllocationOptions::new())?;
    ///     let ptr = alloc.into_raw_ptr::<u8>().unwrap();
    ///     unsafe { memory_manager.free_pages(ptr as usize, 1)? };
    ///
    ///     Ok(())
    /// }
    /// ```
    ///
    /// Some page allocations might require additional restraints such as address,
    /// alignment, or memory type. These can be specified using the [`AllocationOptions`]
    /// structure. With the `AllocationOptions` structure, the caller may call the
    /// `.with_` functions to specify the options they wish to set. Unspecified
    /// options will be interpretted as the default for the implementation.
    ///
    /// ```rust
    /// # use patina_sdk::{efi_types::*, component::service::memory::*};
    ///
    /// fn component(memory_manager: &dyn MemoryManager) -> Result<(), MemoryError> {
    ///     let options = AllocationOptions::new()
    ///         .with_memory_type(EfiMemoryType::BootServicesData)
    ///         .with_alignment(0x200000);
    ///
    ///     let alloc = memory_manager.allocate_pages(1, options)?;
    ///     Ok(())
    /// }
    /// ```
    ///
    fn allocate_pages(&self, page_count: usize, options: AllocationOptions) -> Result<PageAllocation, MemoryError>;

    /// Allocates pages and zeroes them.
    ///
    /// Allocates memory with the same semantics as `allocate_pages`, but also
    /// initializes the all bytes of the pages to zero.
    ///
    /// See [`MemoryManager::allocate_pages`] for more details.
    ///
    fn allocate_zero_pages(
        &self,
        page_count: usize,
        options: AllocationOptions,
    ) -> Result<PageAllocation, MemoryError> {
        let allocation = self.allocate_pages(page_count, options)?;
        allocation.zero_pages();
        Ok(allocation)
    }

    /// Frees previously allocated pages.
    ///
    /// Frees the specified number of pages of memory at the given address. This
    /// must be an address and page count that was previously allocated using the memory
    /// manager by this caller.
    ///
    /// Once memory has been freed, it will no longer be accessible.
    ///
    /// See [`MemoryManager::allocate_pages`] for more details on page allocations.
    ///
    /// # Parameters
    ///
    /// - `address`: The aligned base address of the pages to be freed.
    /// - `pages_count`: The number of pages to be freed.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if the pages were successfully freed.
    /// - `Err(MemoryError)` if the pages could not be freed.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the address and size are valid
    /// and for ensuring that all references and pointers to this memory have been
    /// dropped. The memory will be freed and any access after this call will result
    /// in undefined behavior.
    ///
    unsafe fn free_pages(&self, address: usize, page_count: usize) -> Result<(), MemoryError>;

    /// Gets a heap allocator for the specified memory type.
    ///
    /// Retrieves a reference to an allocator that makes heap allocations from a
    /// heap of the specified memory type. This allocator should not be used directly
    /// but should be used in smart pointers to properly track and initialize memory.
    /// The most common usecase of this is using the `Box::new_in` function to
    /// allocate a boxed value in the specified memory type.
    ///
    /// **Note:** If the caller does not need to specify the memory type (i.e. is
    /// allocating from `EfiMemoryType::BootServicesData`), the global allocator
    /// should be used instead. This is done through standard allocation APIs
    /// such as `Box::new` or `Vec::new`.
    ///
    /// # Parameters
    ///
    /// - `memory_type`: The memory type to use for the allocation.
    ///
    /// # Returns
    ///
    /// - `Ok(&dyn Allocator)` if the allocator was successfully retrieved. See
    ///   the [`Allocator`] trait for more details on the allocator.
    /// - `Err(MemoryError)` if the allocator could not be retrieved.
    ///
    /// # Example
    ///
    /// ```rust
    /// #![feature(allocator_api)]
    /// # use patina_sdk::{efi_types::*, component::service::memory::*};
    ///
    /// fn component(memory_manager: &dyn MemoryManager) -> Result<(), MemoryError> {
    ///     // Acquire the heap allocator for the specified memory type.
    ///     let allocator = memory_manager.get_allocator(EfiMemoryType::BootServicesCode)?;
    ///
    ///     // Allocate new heap objects.
    ///     let boxed_value = Box::new_in(42, allocator);
    ///
    ///     Ok(())
    /// }
    /// ```
    ///
    #[cfg(feature = "alloc")]
    fn get_allocator(&self, memory_type: EfiMemoryType) -> Result<&'static dyn Allocator, MemoryError>;

    /// Sets the attributes of a page.
    ///
    /// Sets the hardware attributes for the provided page range to the specified
    /// access and caching types.
    ///
    /// # Parameters
    ///
    /// - `address`: The page-aligned address of the page to set attributes for.
    /// - `page_count`: The number of pages to set attributes for.
    /// - `access`: The access type to set for the page.
    /// - `caching`: The caching type to set for the page. If `None`, the caching
    ///   attributes will be set to the existing values or default caching type.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if the attributes were successfully set.
    /// - `Err(MemoryError)` if the attributes could not be set.
    ///
    /// # Safety
    ///
    /// Changing tha attributes of a page of memory can result in undefined behavior
    /// if the attributes are not correct for the memory usage. The caller is responsible
    /// for understanding the use of the memory and verifying that all current and
    /// future accesses of the memory align to the attributes configured.
    ///
    unsafe fn set_page_attributes(
        &self,
        address: usize,
        page_count: usize,
        access: AccessType,
        caching: Option<CachingType>,
    ) -> Result<(), MemoryError>;

    /// Gets the attributes of a page.
    ///
    /// Gets the hardware attributes for the provided page range to the specified
    /// access and caching types.
    ///
    /// # Parameters
    ///
    /// - `address`: The page-aligned address of the page to get attributes for.
    /// - `page_count`: The number of pages to get attributes for.
    /// - `access`: The access type to get for the page.
    /// - `caching`: The caching type to get for the page. If `None`, the caching
    ///   attributes will be get to the existing values or default caching type.
    ///
    /// # Returns
    ///
    /// - `Ok((access, caching))` if the attributes were successfully retrieved.
    /// - `Err(MemoryError::InconsistentRangeAttributes)` if the range provided
    ///   does not have consistent attributes accross all pages.
    /// - `Err(MemoryError::InvalidAddress)` if the address does not correspond
    ///   to a valid page of memory.
    /// - `Err(MemoryError)` if the request failed for other reasons.
    ///
    fn get_page_attributes(&self, address: usize, page_count: usize) -> Result<(AccessType, CachingType), MemoryError>;
}

/// The `AllocationOptions` structure allows for the caller to  specify
/// additional constraints on the allocation. This can be used to specify the type
/// of memory to allocate, alignment requirements, and allocation strategy. Users
/// should always start with `AllocationOptions::new()` and then call the `.with_`
/// functions to set the options they wish to use. see [`AllocationOptions::new()`]
/// for more details on the defaults.
///
#[derive(Debug)]
pub struct AllocationOptions {
    allocation_strategy: PageAllocationStrategy,
    alignment: usize,
    memory_type: EfiMemoryType,
}

impl Default for AllocationOptions {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl AllocationOptions {
    /// Creates a new `AllocationOptions` structure with the default values.
    /// This is equivalent to calling `AllocationOptions::default()`.
    ///
    /// # Defaults
    ///
    /// - `allocation_strategy`: [`PageAllocationStrategy::Any`]
    /// - `alignment`: [`UEFI_PAGE_SIZE`]
    /// - `memory_type`: [`EfiMemoryType::BootServicesData`]
    ///
    #[inline(always)]
    pub const fn new() -> Self {
        Self {
            allocation_strategy: PageAllocationStrategy::Any,
            alignment: UEFI_PAGE_SIZE,
            memory_type: EfiMemoryType::BootServicesData,
        }
    }

    /// Specifies the allocation strategy to use for the allocation. See [`PageAllocationStrategy`]
    /// for more details.
    #[inline(always)]
    pub const fn with_strategy(mut self, allocation_strategy: PageAllocationStrategy) -> Self {
        self.allocation_strategy = allocation_strategy;
        self
    }

    /// Specifies the alignment to use for the allocation. This must be a power
    /// of two and greater then the page size.
    ///
    /// Alignment will be ignored if the allocation strategy is [`PageAllocationStrategy::Address`].
    #[inline(always)]
    pub const fn with_alignment(mut self, alignment: usize) -> Self {
        self.alignment = alignment;
        self
    }

    /// Specifies the memory type to use for the allocation. See [`EfiMemoryType`]
    /// for more details.
    #[inline(always)]
    pub const fn with_memory_type(mut self, memory_type: EfiMemoryType) -> Self {
        self.memory_type = memory_type;
        self
    }

    /// Gets the strategy for the allocation.
    #[inline(always)]
    pub fn strategy(&self) -> PageAllocationStrategy {
        self.allocation_strategy
    }

    /// Gets the alignment for the allocation.
    #[inline(always)]
    pub fn alignment(&self) -> usize {
        self.alignment
    }

    /// Gets the memory type for the allocation.
    #[inline(always)]
    pub fn memory_type(&self) -> EfiMemoryType {
        self.memory_type
    }
}

#[repr(align(4096))]
#[allow(dead_code)]
struct UefiPage([u8; UEFI_PAGE_SIZE]);

/// The `PageAllocation` struct represents a block of memory allocated in pages.
/// This struct provides the caller the ability to convert that block of memory
/// into whatever structure best fits the use case. These structures fall into the
/// use cases below.
///
/// # Manual Management
///
/// For cases where the caller wishes to directly manage the usage and freeing of
/// the page allocation, manual management should be used. This is done by converting
/// the `PageAllocation` struct into a raw pointer. The caller is responsible for
/// ensuring the safety of accessing the memory and for freeing the memory appropriately.
/// Failure to free the memory will result in the memory being leaked.
///
/// # Smart Pointers
///
/// Smart pointers allow safely converting the pages of memory into a wrapper type.
/// Currently the only wrapper type supported is the [Box] type. When these pointers
/// go out of scope, the pages of memory will be automatically freed. This is useful
/// for cases where the memory is only needed for a short period or within the scope
/// of a function.
///
/// # Leaked Static
///
/// Leaking the memory is similar to the Smart Pointer use case, but acknowledges
/// that the memory will never be freed. This allows for the caller to obtain a
/// `&'static T` reference to the memory that can be used for the lifetime of the
/// program. This can be useful for global structures that need to be shared between
/// multiple entities.
///
/// # Panics
///
/// If this structure is dropped without being used, it will invoke a [`debug_assert`]
/// which will panic in debug builds. This is to ensure there are no unnecessary
/// memory allocations. If the debug_asserts are disabled, this will free the pages.
///
#[must_use]
pub struct PageAllocation {
    blob: NonNull<u8>,
    page_count: usize,
    memory_manager: &'static dyn MemoryManager,
}

impl PageAllocation {
    /// Creates a new page allocation. The address and page count provided must
    /// be valid and accessible with the `ReadWrite` access type.
    ///
    /// Returns an appropriate `Err` if the address is not page aligned or the
    /// page count is zero.
    ///
    /// ## Pointer Provenance
    ///
    /// As the function interface does not take a pointer, and instead takes a usize
    /// representing the address, there is no pointer provenance metadata during
    /// build by default. This function uses [with_exposed_provenance_mut] to allow
    /// the compiler / other tools to attempt to associate the address with the
    /// original pointer's provenance. It is imperative that the caller exposes the
    /// original pointer's provenance before passing the address to this function via
    /// one of the means described in [pointer provenance](https://doc.rust-lang.org/std/ptr/index.html#provenance)
    /// such as `expose_provenance`.
    ///
    /// ## Safety
    ///
    /// The caller is responsible for ensuring the provided address and page count
    /// are valid and are read/write accessible. Producing a `PageAllocation` will
    /// allow this struct to be safely converted into other types that will access
    /// the memory. Failure to ensure the memory is correct will cause undefined
    /// behavior.
    ///
    pub unsafe fn new(
        addr: usize,
        page_count: usize,
        memory_manager: &'static dyn MemoryManager,
    ) -> Result<Self, MemoryError> {
        let Some(blob) = NonNull::new(with_exposed_provenance_mut(addr)) else {
            return Err(MemoryError::InvalidAddress);
        };

        if !blob.cast::<UefiPage>().is_aligned() {
            return Err(MemoryError::UnalignedAddress);
        }

        if page_count == 0 {
            return Err(MemoryError::InvalidPageCount);
        }

        Ok(Self { blob, page_count, memory_manager })
    }

    /// Gets the number of pages in the allocation.
    #[inline(always)]
    pub fn page_count(&self) -> usize {
        self.page_count
    }

    /// Gets the length of the allocation in bytes.
    #[inline(always)]
    pub fn byte_length(&self) -> usize {
        uefi_pages_to_size!(self.page_count)
    }

    /// Internal routine for zeroing a page allocation. This is not intended for
    /// public use, but serves as a convenience for the `MemoryManager` to zero
    /// the memory without having to convert it to a pointer.
    fn zero_pages(&self) {
        // SAFETY: The memory is allocated and valid for writing through the
        //         provided page count. This is the responsibility of the caller
        //         of PageAllocation::new().
        unsafe { self.blob.write_bytes(0, self.byte_length()) };
    }

    /// Internal function for creating the `PageFree` struct for this allocation.
    #[inline(always)]
    #[cfg(any(test, feature = "alloc"))]
    fn get_page_free(&self) -> PageFree {
        PageFree { blob: self.blob, page_count: self.page_count, memory_manager: self.memory_manager }
    }

    /// Consumes the allocation and returns the raw address which must be manually
    /// freed using the [MemoryManager::free_pages] routine. If the caller fails
    /// to free the memory, it will leak. The caller is responsible for assuring
    /// the type fits in the allocation before dereferencing. The memory will not
    /// be initialized and the caller is responsible ensuring the type is valid.
    ///
    /// # Errors
    ///
    /// If the size of the type `T` is larger than the allocation, this function
    /// will return `None` and the pages will be freed.
    #[must_use]
    pub fn into_raw_ptr<T>(mut self) -> Option<*mut T> {
        if self.byte_length() < size_of::<T>() {
            // This is an intentional case where the struct is being dropped,
            // but we want to avoid triggering the panic in its `Drop` implementation.
            // To handle this safely, we manually free the pages and then call `forget`
            // to prevent `drop` from running.
            self.free_pages();
            core::mem::forget(self);
            return None;
        }

        // Move this struct to manual management and return the address.
        Some(ManuallyDrop::new(self).blob.cast::<T>().as_ptr())
    }

    /// Consumes the allocation and returns the raw address as a slice of type `T`.
    /// The slice must be manually freed using the [MemoryManager::free_pages] routine.
    /// If the caller fails to free the memory, it will leak. The memory will not
    /// be initialized and the caller is responsible ensuring the type is valid.
    /// The length of the slice is the number of bytes in the allocation divided
    /// by the size of `T`.
    #[must_use]
    pub fn into_raw_slice<T>(self) -> *mut [T] {
        let count = self.byte_length() / size_of::<T>();
        let ptr = ManuallyDrop::new(self).blob.cast::<T>().as_ptr();
        core::ptr::slice_from_raw_parts_mut(ptr, count)
    }

    /// Converts the allocation into a `Box<T>` smart pointer, and initializes the
    /// memory to the provided value.
    ///
    /// # Returns
    ///
    /// - `None` if the size of the value is larger than the allocation.
    /// - `Some(Box<T, _>)` of the initialized value.
    ///
    /// # Errors
    ///
    /// If the size of the type `T` is larger than the allocation, this function
    /// will return `None` and the pages will be freed.
    #[must_use]
    #[cfg(any(test, feature = "alloc"))]
    pub fn into_box<T>(self, value: T) -> Option<Box<T, PageFree>> {
        // Create the struct to de-allocate the memory when the smart pointer is
        // dropped.
        let page_free = self.get_page_free();

        // Get the raw pointer. This will cause the page to be manually managed
        // which will be done by the Box and through the PageFree struct.
        let ptr: *mut T = self.into_raw_ptr()?;

        // SAFETY: The memory is allocated and valid for writing through the length.
        unsafe {
            ptr.write(value);
            Some(Box::from_raw_in(ptr, page_free))
        }
    }

    /// Converts the allocation into a `Box<[T]>` smart pointer, and initializes the
    /// memory to the default value of `T`. The length of the slice is the number
    /// of bytes in the allocation divided by the size of `T`.
    #[must_use]
    #[cfg(any(test, feature = "alloc"))]
    pub fn into_boxed_slice<T: Default>(self) -> Box<[T], PageFree> {
        let page_free = self.get_page_free();
        let slice = self.leak_as_slice::<T>();

        // SAFETY: This function has sole ownership of the underlying memory, so
        //         the memory is safe from being converted into a Box multiple times,
        //         which would result in a double free.
        unsafe { Box::from_raw_in(slice as *mut _, page_free) }
    }

    /// Converts the allocation and leaks the memory as a mutable `T`.
    ///
    /// This function is similar to [Box::leak] in terms of caller responsibility for memory
    /// management. Dropping the returned reference will cause a memory leak.
    ///
    /// # Returns
    ///
    /// - `None` if the size of the value is larger than the allocation.
    /// - `Some(&mut T)` of the initialized value.
    ///
    /// # Errors
    ///
    /// If the size of the type `T` is larger than the allocation, this function
    /// will return `None` and the pages will be freed.
    #[must_use]
    pub fn leak_as<'a, T>(self, value: T) -> Option<&'a mut T> {
        let ptr = self.into_raw_ptr::<T>()?;

        // SAFETY: The memory is allocated and valid for writing through the length.
        unsafe {
            ptr.write(value);
            ptr.as_mut()
        }
    }

    /// Converts the allocation and leaks the memory as a mutable slice of type `T`.
    ///
    /// This function is similar to [Box::leak] in terms of caller responsibility for memory
    /// management. Dropping the returned reference will cause a memory leak.
    #[must_use]
    pub fn leak_as_slice<'a, T: Default>(self) -> &'a mut [T] {
        let slice = self.into_raw_slice::<MaybeUninit<T>>();
        unsafe {
            (*slice).fill_with(|| MaybeUninit::new(Default::default()));
            (slice as *mut [T]).as_mut().expect("Slice Pointer just created and is not null")
        }
    }

    /// Frees the allocated pages of memory this struct manages.
    ///
    /// This is not a public method as it invalidates `Self` without consuming `self`.
    /// This should only be used internally to free the memory when dropping `self`.
    fn free_pages(&mut self) {
        let address = self.blob.addr().get();
        // SAFETY: The allocation was never converted into a usable type, so
        //         this structure contains the only reference to the memory and
        //         the memory is safe to free.
        unsafe {
            if self.memory_manager.free_pages(address, self.page_count).is_err() {
                log::error!("Failed to free page allocation at {:x}!", address);
                debug_assert!(false, "Failed to free page allocation!");
            }
        }
    }
}

impl Drop for PageAllocation {
    fn drop(&mut self) {
        self.free_pages();

        // Allocating memory that is never used before being freed is treated as
        // a bug.
        debug_assert!(false, "Page allocation was never used!");
    }
}

impl core::fmt::Display for PageAllocation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PageAllocation").field("address", &self.blob).field("page_count", &self.page_count).finish()
    }
}

/// The `PageFree` struct is a wrapper around a page allocation that allows
/// the memory to be freed when a smart pointer is dropped. This cannot be used to
/// allocate memory, and should only be used to free the specific memory it tracks.
#[cfg(any(test, feature = "alloc"))]
pub struct PageFree {
    blob: NonNull<u8>,
    page_count: usize,
    memory_manager: &'static dyn MemoryManager,
}

#[cfg(any(test, feature = "alloc"))]
unsafe impl Allocator for PageFree {
    fn allocate(&self, _layout: core::alloc::Layout) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        Err(core::alloc::AllocError)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, _layout: core::alloc::Layout) {
        if self.blob != ptr {
            log::error!(
                "PageFree was not used to free the correct memory! Leaking memory at {:?}!",
                self.blob.as_ptr()
            );
            debug_assert!(false, "PageFree was not used to free the correct memory!");
            return;
        }

        let address = self.blob.addr().get();
        // SAFETY: PageFree structures are only created when the memory is converted
        //         into a smart pointer. The smart pointers themselves will ensure
        //         that the memory is safe to free.
        if unsafe { self.memory_manager.free_pages(address, self.page_count).is_err() } {
            log::error!("Failed to free page allocation at {:x}!", address);
            debug_assert!(false, "Failed to free page allocation!");
        }
    }
}

/// The `AccessType` enum represents the different types of access that can be
/// requested for a page of memory. This reflects the access types that can be
/// set in the CPU page table structures.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum AccessType {
    /// No access. Any access to the page would result in an exception.
    NoAccess,
    /// Read-only access. Only read access is allowed. Write or Execution would
    /// result in an exception.
    ReadOnly,
    /// Read-write access. Both read and write access are allowed. Execution would
    /// result in an exception.
    ReadWrite,
    /// Read-Execute access. Only read and execute access are allowed. Write access
    /// would result in an exception.
    ReadExecute,
    /// Read-write-execute access. This type of access is generally unsafe and
    /// may not be supported for all operations.
    ReadWriteExecute,
}

impl AccessType {
    /// Converts the EFI attributes to an `AccessType`. This will only check the
    /// access flags. Other flags will need to be checked separately.
    ///
    /// If the ReadProtect flag is set, the page will be marked as NoAccess
    /// regardless of the presence of other flags. This is because Read Protect
    /// is a EFI construct that will just result in the page being marked invalid.
    /// However, this may have implications if callers are expecting to be able to
    /// persist other flags through the "ReadProtect" transition. This is a intentional
    /// decision and callers who wish to allow such transitions should manually
    /// convert the types.
    ///
    pub fn from_efi_attributes(attributes: u64) -> AccessType {
        if attributes & efi::MEMORY_RP != 0 {
            AccessType::NoAccess
        } else {
            let readable_attr = attributes & (efi::MEMORY_RO | efi::MEMORY_XP);
            if readable_attr == efi::MEMORY_RO {
                AccessType::ReadExecute
            } else if readable_attr == efi::MEMORY_XP {
                AccessType::ReadWrite
            } else if readable_attr == (efi::MEMORY_RO | efi::MEMORY_XP) {
                AccessType::ReadOnly
            } else {
                AccessType::ReadWriteExecute
            }
        }
    }
}

/// The `CachingType` enum represents the different types of caching that can be
/// requested for a page of memory. This reflects the caching types that can be
/// set in the CPU page table or MTRR structures.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CachingType {
    /// Uncached.
    Uncached,
    /// Write-combining caching.
    WriteCombining,
    /// Write-back caching.
    WriteBack,
    /// Write-through caching.
    WriteThrough,
    /// Write-protect caching. This is not supported on all platforms, and using
    /// the access attribute to accomplish this is preferred.
    WriteProtect,
}

impl CachingType {
    /// Converts the EFI attributes to a `CachingType`. This will only check the
    /// caching flags. Other flags will need to be checked separately.
    ///
    /// This function will return `None` if the attributes do not match any of
    /// the known caching types or it has conflicting attributes.
    ///
    /// This function will not check for the type EFI_MEMORY_UCE as it is generally
    /// unused.
    ///
    pub fn from_efi_attributes(attributes: u64) -> Option<CachingType> {
        match attributes & efi::CACHE_ATTRIBUTE_MASK {
            efi::MEMORY_WB => Some(CachingType::WriteBack),
            efi::MEMORY_WC => Some(CachingType::WriteCombining),
            efi::MEMORY_WT => Some(CachingType::WriteThrough),
            efi::MEMORY_UC => Some(CachingType::Uncached),
            efi::MEMORY_WP => Some(CachingType::WriteProtect),
            _ => None,
        }
    }
}

/// The `MemoryError` enum represents the different types of errors that can occur
/// when using the memory allocation services.
#[derive(Debug)]
pub enum MemoryError {
    /// The memory manager hit an internal error.
    InternalError,
    /// No available memory for allocation with the provided parameters.
    NoAvailableMemory,
    /// The address provided is not aligned to the default or provided alignment.
    UnalignedAddress,
    /// The alignment provided is not page aligned.
    InvalidAlignment,
    /// The provided address is not a valid address for the given operation.
    InvalidAddress,
    /// The provided page range does not contain consistent attributes.
    InconsistentRangeAttributes,
    /// The provided page range is not valid for the given operation.
    InvalidPageCount,
    /// The requested memory type is not supported by the allocator for the given operation.
    UnsupportedMemoryType,
    /// The provided attributes are not supported. This may be a hardware or safety limitation.
    UnsupportedAttributes,
}

impl From<MemoryError> for EfiError {
    fn from(value: MemoryError) -> Self {
        match value {
            MemoryError::NoAvailableMemory => EfiError::OutOfResources,
            MemoryError::UnsupportedAttributes | MemoryError::UnsupportedMemoryType => EfiError::Unsupported,
            _ => EfiError::InvalidParameter,
        }
    }
}

/// The strategy to use for page allocation in the `PageAllocator` trait.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PageAllocationStrategy {
    /// The allocation may be made from any address. The underlying algorithm for
    /// selecting the address is implementation defined.
    Any,
    /// Allocate at the specified address. The wrapped address must be page aligned.
    /// If the memory starting at this address through the requested length is not
    /// available, an error will be returned.
    Address(usize),
    /// Allocate at an address no larger than the specified address (inclusive).
    MaxAddress(usize),
}

#[cfg(any(test, feature = "mockall"))]
pub use mock::StdMemoryManager;

#[cfg(any(test, feature = "mockall"))]
#[coverage(off)]
mod mock {
    extern crate std;
    use std::{
        alloc::{Layout, alloc, dealloc},
        collections::HashMap,
        sync::Mutex,
    };

    use super::*;
    /// A fully working mock [MemoryManager] based off of the std global allocator.
    ///
    /// This mock [MemoryManager] implementation should be used when you expect allocations to succeed. If you wish to
    /// create a mock that will fail, use `mockall` to create a mock implementation of [MemoryManager] with functions
    /// that return errors that you specify.
    #[derive(Default)]
    pub struct StdMemoryManager {
        memory_attributes: Mutex<HashMap<usize, (AccessType, CachingType)>>,
    }

    impl StdMemoryManager {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl MemoryManager for StdMemoryManager {
        fn allocate_pages(&self, page_count: usize, options: AllocationOptions) -> Result<PageAllocation, MemoryError> {
            let Ok(layout) = Layout::from_size_align(page_count * UEFI_PAGE_SIZE, options.alignment()) else {
                return Err(MemoryError::InvalidAlignment);
            };

            let blob = unsafe { NonNull::new(alloc(layout)).expect("Test has sufficient memory to allocate pages") };

            unsafe {
                PageAllocation::new(blob.as_ptr().expose_provenance(), page_count, Box::leak(Box::new(Self::new())))
            }
        }

        unsafe fn free_pages(&self, address: usize, page_count: usize) -> Result<(), MemoryError> {
            let ptr = address as *mut u8;
            let layout = Layout::from_size_align(page_count * UEFI_PAGE_SIZE, UEFI_PAGE_SIZE).unwrap();
            unsafe { dealloc(ptr, layout) };
            Ok(())
        }

        unsafe fn set_page_attributes(
            &self,
            address: usize,
            _page_count: usize,
            access: AccessType,
            caching: Option<CachingType>,
        ) -> Result<(), MemoryError> {
            let caching = caching.unwrap_or(CachingType::WriteBack);
            self.memory_attributes.lock().expect("This is not actually shared.").insert(address, (access, caching));
            Ok(())
        }

        fn get_page_attributes(
            &self,
            address: usize,
            _page_count: usize,
        ) -> Result<(AccessType, CachingType), MemoryError> {
            if let Some((access, caching)) =
                self.memory_attributes.lock().expect("This is not actually shared.").get(&address)
            {
                Ok((*access, *caching))
            } else {
                Err(MemoryError::InvalidAddress)
            }
        }

        #[cfg(feature = "alloc")]
        fn get_allocator(&self, _memory_type: EfiMemoryType) -> Result<&'static dyn Allocator, MemoryError> {
            Ok(&std::alloc::System)
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use core::{
        alloc::Layout,
        sync::atomic::{AtomicBool, AtomicUsize},
    };

    use super::*;
    use crate::component::service::Service;

    #[test]
    fn test_custom_mock_with_failing() {
        let mut mock = MockMemoryManager::new();
        mock.expect_allocate_pages().returning(|_, _| Err(MemoryError::NoAvailableMemory));
        mock.expect_free_pages().returning(|_, _| Err(MemoryError::InvalidAddress));
        mock.expect_set_page_attributes().returning(|_, _, _, _| Err(MemoryError::UnsupportedAttributes));
        mock.expect_get_page_attributes().returning(|_, _| Err(MemoryError::InvalidAddress));
        let service = Service::mock(Box::new(mock));

        assert!(service.allocate_pages(5, AllocationOptions::new()).is_err());
        assert!(unsafe { service.free_pages(0, 5).is_err() });
        assert!(unsafe { service.set_page_attributes(0, 5, AccessType::ReadOnly, None).is_err() });
        assert!(service.get_page_attributes(0, 5).is_err());
    }

    #[test]
    fn test_error_to_efi_error_conversion_not_changed() {
        // enumerate all the errors and ensure they convert to the expected EFI error codes.
        let error = MemoryError::NoAvailableMemory;
        assert_eq!(Into::<EfiError>::into(error), EfiError::OutOfResources);

        let error = MemoryError::UnsupportedAttributes;
        assert_eq!(Into::<EfiError>::into(error), EfiError::Unsupported);

        let error = MemoryError::InvalidAddress;
        assert_eq!(Into::<EfiError>::into(error), EfiError::InvalidParameter);

        let error = MemoryError::InternalError;
        assert_eq!(Into::<EfiError>::into(error), EfiError::InvalidParameter);

        let error = MemoryError::InconsistentRangeAttributes;
        assert_eq!(Into::<EfiError>::into(error), EfiError::InvalidParameter);

        let error = MemoryError::InvalidPageCount;
        assert_eq!(Into::<EfiError>::into(error), EfiError::InvalidParameter);

        let error = MemoryError::InvalidAlignment;
        assert_eq!(Into::<EfiError>::into(error), EfiError::InvalidParameter);

        let error = MemoryError::UnsupportedMemoryType;
        assert_eq!(Into::<EfiError>::into(error), EfiError::Unsupported);

        let error = MemoryError::UnalignedAddress;
        assert_eq!(Into::<EfiError>::into(error), EfiError::InvalidParameter);
    }

    #[test]
    fn test_access_type_rp_always_no_access() {
        let access = AccessType::from_efi_attributes(efi::MEMORY_RP | efi::MEMORY_RO);
        assert_eq!(access, AccessType::NoAccess);

        let access = AccessType::from_efi_attributes(efi::MEMORY_RP | efi::MEMORY_XP);
        assert_eq!(access, AccessType::NoAccess);

        let access = AccessType::from_efi_attributes(efi::MEMORY_RP | 0x50000);
        assert_eq!(access, AccessType::NoAccess);
    }

    #[test]
    fn test_access_type_logic_matches_expectations() {
        // Memory is not Execute protected (MEMORY_XP) so it is read and execute
        let access = AccessType::from_efi_attributes(efi::MEMORY_RO);
        assert_eq!(access, AccessType::ReadExecute);

        // Memory is execute protected (MEMORY_XP) so it is read and write
        let access = AccessType::from_efi_attributes(efi::MEMORY_XP);
        assert_eq!(access, AccessType::ReadWrite);

        // Memory is read only (MEMORY_RO) and execute protected (MEMORY_XP), so it is read only
        let access = AccessType::from_efi_attributes(efi::MEMORY_RO | efi::MEMORY_XP);
        assert_eq!(access, AccessType::ReadOnly);

        // Memory is neither read only (MEMORY_RO) nor execute protected (MEMORY_XP), so it is read write execute
        let access = AccessType::from_efi_attributes(0);
        assert_eq!(access, AccessType::ReadWriteExecute);
    }

    #[test]
    fn test_conflicting_caching_types() {
        // Test that conflicting caching types return None.
        let caching = CachingType::from_efi_attributes(efi::MEMORY_WB | efi::MEMORY_WC);
        assert_eq!(caching, None);

        let caching = CachingType::from_efi_attributes(efi::MEMORY_WT | efi::MEMORY_UC);
        assert_eq!(caching, None);

        let caching = CachingType::from_efi_attributes(0x50000);
        assert_eq!(caching, None);
    }

    #[test]
    fn test_caching_type_hardcoded_conversion_has_not_changed() {
        // Fully test all the caching types to ensure they match the expected values.
        let caching = CachingType::from_efi_attributes(efi::MEMORY_WB);
        assert_eq!(caching, Some(CachingType::WriteBack));

        let caching = CachingType::from_efi_attributes(efi::MEMORY_WC);
        assert_eq!(caching, Some(CachingType::WriteCombining));

        let caching = CachingType::from_efi_attributes(efi::MEMORY_WT);
        assert_eq!(caching, Some(CachingType::WriteThrough));

        let caching = CachingType::from_efi_attributes(efi::MEMORY_UC);
        assert_eq!(caching, Some(CachingType::Uncached));

        let caching = CachingType::from_efi_attributes(efi::MEMORY_WP);
        assert_eq!(caching, Some(CachingType::WriteProtect));

        // Test an unsupported caching type.
        let caching = CachingType::from_efi_attributes(0x50000);
        assert_eq!(caching, None);
    }

    #[test]
    fn test_page_free_allocate_errors() {
        let pf = PageFree {
            blob: NonNull::dangling(),
            page_count: 1,
            memory_manager: Box::leak(Box::new(StdMemoryManager::new())),
        };

        assert!(pf.allocate(Layout::new::<u8>()).is_err_and(|e| matches!(e, core::alloc::AllocError)));
    }

    #[test]
    #[should_panic(expected = "PageFree was not used to free the correct memory!")]
    fn test_page_free_mismatched_address_should_assert() {
        let mut value: u8 = 5;
        let data = NonNull::new(&mut value).unwrap();
        let pf = PageFree {
            // SAFETY: Intentionally using a bad address
            blob: unsafe { data.add(0x1000) },
            page_count: 1,
            memory_manager: Box::leak(Box::new(StdMemoryManager::new())),
        };

        unsafe { pf.deallocate(data, Layout::new::<u8>()) };
    }

    #[test]
    #[should_panic(expected = "Failed to free page allocation!")]
    fn test_page_free_should_bubble_update_page_dealloc_error() {
        let mut value: u8 = 5;
        let blob = NonNull::new(&mut value).unwrap();
        let mut mock = MockMemoryManager::new();
        mock.expect_free_pages().returning(|_, _| Err(MemoryError::InvalidAddress));
        let pf = PageFree { blob, page_count: 1, memory_manager: Box::leak(Box::new(mock)) };

        // This will panic because the mock returns an error.
        unsafe { pf.deallocate(blob, Layout::new::<u8>()) };
    }

    #[test]
    #[should_panic(expected = "Failed to free page allocation")]
    fn test_bubble_up_free_pages_err() {
        let mut pa = StdMemoryManager::new().allocate_pages(1, AllocationOptions::new()).unwrap();
        let mut mock = MockMemoryManager::new();
        mock.expect_free_pages().returning(|_, _| Err(MemoryError::InvalidAddress));
        pa.memory_manager = Box::leak(Box::new(mock));
        // When pa goes out of scope, free_pages will be called, which will panic due to the mock returning an error.
    }

    #[test]
    fn test_page_allocation_display() {
        let mm = StdMemoryManager::new();

        let page = mm.allocate_pages(1, AllocationOptions::new()).unwrap();
        let address = page.blob.as_ptr() as usize;

        let display = format!("{}", page);
        let _ = page.into_raw_ptr::<u8>(); // Consume the pa to avoid the debug_assert in drop.
        let expected = format!("PageAllocation {{ address: 0x{:x}, page_count: 1 }}", address);

        assert_eq!(display, expected);
    }

    #[test]
    fn test_page_allocation() {
        let mock = StdMemoryManager::new();

        let service = Service::mock(Box::new(mock));

        let page = service.allocate_pages(1, AllocationOptions::new()).unwrap();
        assert_eq!(page.page_count(), 1);
        assert_eq!(page.byte_length(), UEFI_PAGE_SIZE);
        let my_thing = page.leak_as(42).unwrap();
        assert_eq!(*my_thing, 42);
    }

    #[test]
    fn test_failed_into_box_does_not_panic() {
        let mm = Service::mock(Box::new(StdMemoryManager::new()));

        let page = mm
            .allocate_pages(1, AllocationOptions::default())
            .unwrap_or_else(|e| panic!("Failed to allocate pages: {:?}", e));

        assert!(
            page.into_box([42_u8; UEFI_PAGE_SIZE + 1]).is_none(),
            "Expected allocation to fail due to insufficient size for Box<[u8]>"
        );
    }

    #[test]
    fn test_failed_leak_as_does_not_panic() {
        let mm = Service::mock(Box::new(StdMemoryManager::new()));

        let page = mm
            .allocate_pages(1, AllocationOptions::default())
            .unwrap_or_else(|e| panic!("Failed to allocate pages: {:?}", e));

        assert!(
            page.leak_as([42_u8; UEFI_PAGE_SIZE + 1]).is_none(),
            "Expected allocation to fail due to insufficient size for [u8]"
        );
    }

    #[test]
    fn test_into_boxed_slice_will_call_drop_properly() {
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
        static COUNT: AtomicUsize = AtomicUsize::new(0);

        struct MyStruct(usize);
        impl MyStruct {
            fn value(&self) -> usize {
                self.0
            }
        }

        impl Default for MyStruct {
            fn default() -> Self {
                MyStruct(COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
            }
        }

        impl Drop for MyStruct {
            fn drop(&mut self) {
                DROP_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let mm = Box::leak(Box::new(StdMemoryManager::new()));

        let pa = mm.allocate_pages(1, AllocationOptions::default()).expect("Should not fail for test.");
        {
            let boxed_slice = pa.into_boxed_slice::<MyStruct>();
            assert_eq!(boxed_slice.len(), UEFI_PAGE_SIZE / size_of::<MyStruct>());

            let mut i = 0;
            boxed_slice.iter().for_each(|item| {
                assert_eq!(item.value(), i, "Default value of MyStruct should be {i}");
                i += 1;
            });
        }
        assert_eq!(
            DROP_COUNT.load(std::sync::atomic::Ordering::SeqCst),
            UEFI_PAGE_SIZE / size_of::<MyStruct>(),
            "Drop should be called for each item in the boxed slice"
        );
    }

    #[test]
    fn test_allocation_options_config_sticks() {
        let options = AllocationOptions::default()
            .with_alignment(0x200)
            .with_memory_type(EfiMemoryType::PalCode)
            .with_strategy(PageAllocationStrategy::Address(0x1000_0000_0000_0004));

        assert_eq!(options.alignment(), 0x200);
        assert_eq!(options.memory_type(), EfiMemoryType::PalCode);
        assert_eq!(options.strategy(), PageAllocationStrategy::Address(0x1000_0000_0000_0004));
    }

    #[test]
    fn test_bad_page_allocation() {
        let mm = Box::leak(Box::new(StdMemoryManager::new()));

        let address = UefiPage([0u8; UEFI_PAGE_SIZE]).0.as_mut_ptr() as usize;

        // Catch unaligned address
        assert!(
            unsafe { PageAllocation::new(address + 1, 1, mm) }
                .is_err_and(|e| matches!(e, MemoryError::UnalignedAddress))
        );
        assert!(
            unsafe { PageAllocation::new(address - 1, 1, mm) }
                .is_err_and(|e| matches!(e, MemoryError::UnalignedAddress))
        );

        // Catch zero page count
        assert!(
            unsafe { PageAllocation::new(address, 0, mm) }.is_err_and(|e| matches!(e, MemoryError::InvalidPageCount))
        );
    }

    #[test]
    fn test_page_allocation_zeroing_all_pages_works() {
        let mm = Box::leak(Box::new(StdMemoryManager::new()));

        let pa = mm.allocate_pages(10, AllocationOptions::default()).expect("Should not fail for test.");

        // Write some data to the pages to ensure they are not zeroed.
        unsafe { pa.blob.cast::<UefiPage>().write(UefiPage([1u8; UEFI_PAGE_SIZE])) };

        pa.zero_pages();

        // check that all bytes are zeroed
        let a = pa.into_raw_ptr::<u8>().unwrap();
        for i in 0..(UEFI_PAGE_SIZE * 10) {
            assert_eq!(unsafe { *a.add(i) }, 0, "Byte at index {} is not zeroed", i);
        }
    }

    #[test]
    fn test_into_raw_slice() {
        let mm = Box::leak(Box::new(StdMemoryManager::new()));

        let pa = mm.allocate_pages(10, AllocationOptions::default()).expect("Should not fail for test.");
        let slice: *mut [u64] = pa.into_raw_slice();
        assert_eq!(unsafe { (*slice).len() }, (UEFI_PAGE_SIZE * 10) / size_of::<u64>());

        #[repr(C, packed(1))]
        struct TestWeirdSized {
            _a: u64,
            _b: u32,
            _c: u16,
        }

        // The intent is to ensure that the size of the struct is not evenly divisible by 4k page size. We want a weird size that does not fit into
        // the standard page size alignment evenly.
        assert_ne!(size_of::<TestWeirdSized>() % UEFI_PAGE_SIZE, 0);

        let pa = mm.allocate_pages(10, AllocationOptions::default()).expect("Should not fail for test.");
        let slice: *mut [TestWeirdSized] = pa.into_raw_slice();
        assert_eq!(unsafe { (*slice).len() }, (UEFI_PAGE_SIZE * 10) / size_of::<TestWeirdSized>());
    }

    #[test]
    fn test_allocate_zero_pages_bubbles_up_error() {
        let mm = Box::leak(Box::new(StdMemoryManager::new()));

        // Do a normal page allocation just to ensure it succeeds.
        let Ok(pa) = mm.allocate_zero_pages(10, AllocationOptions::default()) else {
            panic!("Expected allocation to succeed, but it failed.");
        };
        // use it so we don't panic for unused page allocation.
        let _ = pa.into_raw_ptr::<u8>();

        // Overflow isize::MAX to ensure that the allocation fails.
        let pages = 2usize.pow(63) / UEFI_PAGE_SIZE;
        assert!(mm.allocate_pages(pages, AllocationOptions::default()).is_err());
    }

    #[test]
    fn test_into_box_value_is_placed_properly() {
        let mm = Box::leak(Box::new(StdMemoryManager::new()));
        static DROPPED: AtomicBool = AtomicBool::new(false);

        struct MyStruct(usize);

        impl MyStruct {
            fn new(value: usize) -> Self {
                MyStruct(value)
            }

            fn value(&self) -> usize {
                self.0
            }
        }

        impl Drop for MyStruct {
            fn drop(&mut self) {
                DROPPED.store(true, core::sync::atomic::Ordering::SeqCst);
            }
        }

        let pa = mm.allocate_pages(1, AllocationOptions::default()).expect("Should not fail for test.");

        // Create the object for a limited time
        {
            let boxed = pa.into_box(MyStruct::new(42)).expect("Should convert to Box<T> successfully");
            assert_eq!(boxed.value(), 42);
        }

        // ensure drop was called
        assert!(DROPPED.load(core::sync::atomic::Ordering::SeqCst), "Drop was not called on MyStruct");
    }

    #[test]
    fn test_into_boxed_slice_with_missing_size_returns_slice_of_size_zero() {
        let mm = Box::leak(Box::new(StdMemoryManager::new()));

        let pa = mm.allocate_pages(1, AllocationOptions::default()).expect("Should not fail for test.");
        let slice = pa.into_raw_slice::<[u8; UEFI_PAGE_SIZE * 2]>();
        assert_eq!(unsafe { (*slice).len() }, 0);
    }

    #[test]
    fn test_leak_as_slice_does_not_drop_items() {
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
        static COUNT: AtomicUsize = AtomicUsize::new(0);

        struct MyStruct(usize);
        impl MyStruct {
            fn value(&self) -> usize {
                self.0
            }
        }

        impl Default for MyStruct {
            fn default() -> Self {
                MyStruct(COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
            }
        }

        impl Drop for MyStruct {
            fn drop(&mut self) {
                DROP_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let mm = Box::leak(Box::new(StdMemoryManager::new()));

        let pa = mm.allocate_pages(1, AllocationOptions::default()).expect("Should not fail for test.");
        {
            let boxed_slice = pa.leak_as_slice::<MyStruct>();
            assert_eq!(boxed_slice.len(), UEFI_PAGE_SIZE / size_of::<MyStruct>());

            let mut i = 0;
            boxed_slice.iter().for_each(|item| {
                assert_eq!(item.value(), i, "Default value of MyStruct should be {i}");
                i += 1;
            });
        }
        assert_eq!(
            DROP_COUNT.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "Slice is static, so individual items should not be dropped unless replaced"
        );
    }

    #[test]
    fn test_uefi_page_size_is_4k() {
        assert_eq!(UEFI_PAGE_SIZE, 4096, "UEFI page size should be 4k (4096 bytes)");
    }

    #[test]
    fn test_ptr_too_large() {
        let mm = Box::leak(Box::new(StdMemoryManager::new()));
        let pa = mm.allocate_pages(1, AllocationOptions::default()).expect("Should not fail for test.");
        let res = pa.into_raw_ptr::<[u8; UEFI_PAGE_SIZE + 1]>();
        assert!(res.is_none(), "Expected allocation to fail due to insufficient size for [u8; UEFI_PAGE_SIZE + 1]");
    }
}
