//! EFI_DEBUG_IMAGE_INFO_TABLE Support
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{
    alloc::Layout,
    fmt,
    ptr::{self, NonNull},
};
use r_efi::efi;
use spin::rwlock::RwLock;

/// Default allocation size for the debug image info table.
const DEFAULT_CAPACITY: usize = 16;
const _: () = assert!(DEFAULT_CAPACITY > 0);

/// Error returned when growing the debug image info table fails.
#[derive(Debug)]
enum GrowError {
    /// The requested layout was invalid.
    InvalidLayout(alloc::alloc::LayoutError),
    /// The allocator returned a null pointer.
    AllocFailed,
}

impl fmt::Display for GrowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrowError::InvalidLayout(e) => write!(f, "invalid layout: {e}"),
            GrowError::AllocFailed => write!(f, "allocation returned null"),
        }
    }
}

impl From<alloc::alloc::LayoutError> for GrowError {
    fn from(e: alloc::alloc::LayoutError) -> Self {
        GrowError::InvalidLayout(e)
    }
}

/// The type of debug image info entry.
pub(super) enum ImageInfoType {
    /// A normal debug image info entry.
    Normal,
}

impl From<ImageInfoType> for u32 {
    fn from(value: ImageInfoType) -> Self {
        match value {
            ImageInfoType::Normal => efi::DEBUG_IMAGE_INFO_TYPE_NORMAL,
        }
    }
}

/// Represents a table of debug image information entries.
///
/// ## Invariants
///
/// - `header.table_size` and `capacity` always reflect the actual state of the allocated buffer pointed to by
///   `header.debug_image_info_table`, ensuring no out-of-bounds access can occur.
///
/// ## Warning
///
/// The above invariants are only upheld on the assumption that this struct is the sole modifier of the underlying
/// table. This cannot be guaranteed due to the fact that the table header (`efi::DebugImageInfoTableHeader`) is
/// exposed publicly via the UEFI configuration table mechanism. It is expected that this table is read-only when
/// accessed via this mechanism, but this cannot be enforced.
pub(super) struct DebugImageInfoData {
    /// The header of the debug image info table, which is registered as a UEFI configuration table.
    header: efi::DebugImageInfoTableHeader,
    /// The total number of `efi::DebugImageInfo` entries able to be added to the table before a reallocation is
    /// needed.
    capacity: usize,
}

// SAFETY: Access to the raw table pointer in the header is gated behind methods that require `&mut self`.
unsafe impl Send for DebugImageInfoData {}
// SAFETY: Access to the raw table pointer in the header is gated behind methods that require `&mut self`.
unsafe impl Sync for DebugImageInfoData {}

impl DebugImageInfoData {
    /// Creates a new, empty Debug Image Info Table.
    const fn new() -> Self {
        Self {
            header: efi::DebugImageInfoTableHeader {
                volatile_update_status: 0,
                table_size: 0,
                debug_image_info_table: ptr::null_mut(),
            },
            capacity: 0,
        }
    }

    /// Creates a new, empty Debug Image Info Table wrapped in a RwLock.
    pub(super) const fn new_locked() -> RwLock<Self> {
        RwLock::new(Self::new())
    }

    /// Returns a reference to the header of the debug image info table.
    pub(super) fn header(&self) -> &efi::DebugImageInfoTableHeader {
        &self.header
    }

    /// Returns an immutable slice of table entries.
    ///
    /// This should be used for read-only access.
    fn table(&self) -> &[efi::DebugImageInfo] {
        if self.header.table_size == 0 {
            return &[];
        }

        // SAFETY: `debug_image_info_table` is non-null due to the table_size check above, and the invariants of this
        //   struct ensure it is valid for reads of `table_size` entries within a single allocation.
        unsafe { core::slice::from_raw_parts(self.header.debug_image_info_table, self.header.table_size as usize) }
    }

    /// Returns a mutable pointer to the start of the debug image info table.
    ///
    /// This should be used for read-write access.
    fn table_mut(&mut self) -> *mut efi::DebugImageInfo {
        self.header.debug_image_info_table
    }

    /// Returns the current number of entries in the table.
    fn len(&self) -> usize {
        self.header.table_size as usize
    }

    /// Returns the current update status of the table header.
    fn update_status(&self) -> u32 {
        // SAFETY: This is a field owned by this struct and is valid for reads.
        unsafe { ptr::read_volatile(&self.header.volatile_update_status) }
    }

    /// Sets the update status of the table header.
    fn set_update_status(&mut self, status: u32) {
        // SAFETY: This is a field owned by this struct and is valid for writes.
        unsafe { ptr::write_volatile(&mut self.header.volatile_update_status, status) }
    }

    /// Returns the current capacity of the table.
    fn capacity(&self) -> usize {
        self.capacity
    }

    /// Sets the update-in-progress flag.
    fn set_update_in_progress(&mut self) {
        self.set_update_status(self.update_status() | efi::DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS)
    }

    fn clear_update_in_progress(&mut self) {
        self.set_update_status(self.update_status() & !efi::DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS);
    }

    /// Sets the table-modified flag.
    fn set_modified(&mut self) {
        self.set_update_status(self.update_status() | efi::DEBUG_IMAGE_INFO_TABLE_MODIFIED)
    }

    /// Allows for safe modification of the table while managing update flags.
    fn modify(&mut self, f: impl FnOnce(&mut Self)) {
        self.set_update_in_progress();
        f(self);
        self.set_modified();
        self.clear_update_in_progress();
    }

    /// Adds a new entry to the debug image info table.
    pub(super) fn add_entry(
        &mut self,
        image_info_type: ImageInfoType,
        protocol: NonNull<efi::protocols::loaded_image::Protocol>,
        handle: efi::Handle,
    ) {
        self.modify(|s| {
            if s.len() == s.capacity()
                && let Err(e) = s.grow()
            {
                log::error!("Failed to add Debug Image Entry: Err [{e:?}]");
                return;
            }

            let entry = new_debug_image_info(image_info_type, protocol, handle);

            // SAFETY: Invariants of this struct ensure this addition is within bounds and is aligned for
            //   efi::DebugImageInfo.
            unsafe { s.table_mut().add(s.len()).write(entry) }
            s.header.table_size += 1;
        });
    }

    /// Removes the first entry matching the specified handle.
    pub(super) fn remove_entry(&mut self, handle: efi::Handle) {
        self.modify(|s| {
            if let Some(index) = s.find(handle)
                && let Some(mut entry) = s.swap_remove(index)
            {
                drop_debug_image_info(&mut entry);
            }
        });
    }

    /// Finds the index of the first entry matching the specified handle.
    fn find(&self, handle: efi::Handle) -> Option<usize> {
        for (index, entry) in self.table().iter().enumerate() {
            if let Some(entry_handle) = debug_image_info_handle(entry)
                && entry_handle == handle
            {
                return Some(index);
            }
        }

        None
    }

    /// Removes and returns the entry at the specified index, replacing it with the last entry.
    ///
    /// Returns `None` if the index is out of bounds.
    fn swap_remove(&mut self, index: usize) -> Option<efi::DebugImageInfo> {
        if index >= self.len() {
            return None;
        }

        let data = self.table_mut();

        // SAFETY: Invariants of this struct ensure that index is within bounds and aligned for efi::DebugImageInfo.
        let value = unsafe { core::ptr::read(data.add(index)) };

        let last = self.len() - 1;
        if index != last {
            // SAFETY: data pointers are within allocated table bounds and non-overlapping for a single element copy.
            unsafe { ptr::copy_nonoverlapping(data.add(last), data.add(index), 1) };
        }

        self.header.table_size -= 1;
        Some(value)
    }

    /// Doubles the current capacity of the table.
    ///
    /// If the current capacity is zero, sets it to a default initial capacity.
    fn grow(&mut self) -> Result<(), GrowError> {
        let (data, new_capacity) = if self.capacity == 0 {
            let layout = Layout::array::<efi::DebugImageInfo>(DEFAULT_CAPACITY)?;

            // SAFETY: layout is non-zero sized due to DEFAULT_CAPACITY being non-zero
            let data = unsafe { alloc::alloc::alloc_zeroed(layout) };
            (data, DEFAULT_CAPACITY)
        } else {
            let old_layout = Layout::array::<efi::DebugImageInfo>(self.capacity)?;
            let new_capacity = self.capacity * 2;
            let new_layout = Layout::array::<efi::DebugImageInfo>(new_capacity)?;
            // SAFETY: layout is the same layout that was used to allocate the original buffer due to the invariants
            //   of this struct.
            // SAFETY: new_size is greater than zero due to the if branch above ensuring capacity is non-zero.
            // SAFETY: new_size does not exceed isize::MAX as the `Layout` call would have failed.
            let data = unsafe { alloc::alloc::realloc(self.table_mut().cast::<u8>(), old_layout, new_layout.size()) };
            (data, new_capacity)
        };

        if data.is_null() {
            return Err(GrowError::AllocFailed);
        }

        self.capacity = new_capacity;
        self.header.debug_image_info_table = data as *mut efi::DebugImageInfo;
        Ok(())
    }
}

impl Drop for DebugImageInfoData {
    fn drop(&mut self) {
        // Free the heap allocation backing each entry in the table.
        let len = self.len();
        let data = self.table_mut();
        for i in 0..len {
            // SAFETY: Invariants of this struct ensure each entry is a valid, owned `efi::DebugImageInfo` allocated
            //   by this struct.
            unsafe { drop_debug_image_info(&mut *data.add(i)) };
        }

        // Deallocate the data
        if self.capacity > 0 {
            let layout = Layout::array::<efi::DebugImageInfo>(self.capacity).unwrap();
            // SAFETY: Invariants of this struct ensure that `data` was allocated with this layout.
            unsafe {
                alloc::alloc::dealloc(self.table_mut().cast::<u8>(), layout);
            }
        }
    }
}

/// Allocates a new normal debug image info entry, returning an `efi::DebugImageInfo` whose `normal_image` pointer
/// owns a heap-allocated `efi::DebugImageInfoNormal`.
fn new_debug_image_info(
    image_info_type: ImageInfoType,
    protocol: NonNull<efi::protocols::loaded_image::Protocol>,
    handle: efi::Handle,
) -> efi::DebugImageInfo {
    efi::DebugImageInfo {
        normal_image: alloc::boxed::Box::into_raw(alloc::boxed::Box::new(efi::DebugImageInfoNormal {
            image_info_type: image_info_type.into(),
            loaded_image_protocol_instance: protocol.as_ptr(),
            image_handle: handle,
        })),
    }
}

/// Returns the image handle associated with a debug image info entry.
fn debug_image_info_handle(info: &efi::DebugImageInfo) -> Option<efi::Handle> {
    // SAFETY: Every entry created by this module is a normal entry whose `normal_image` pointer was produced by
    //   `new_debug_image_info` and remains valid until it is dropped.
    let normal = unsafe { &*info.normal_image };
    Some(normal.image_handle)
}

/// Frees the heap allocation backing a debug image info entry.
fn drop_debug_image_info(info: &mut efi::DebugImageInfo) {
    // SAFETY: `normal_image` was produced by `Box::into_raw` in `new_debug_image_info` and has not yet been freed.
    let normal = unsafe { alloc::boxed::Box::from_raw(info.normal_image) };
    drop(normal);
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::test_support;

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            test_support::init_test_logger();
            f();
        })
        .unwrap();
    }

    #[test]
    fn test_init_simple() {
        with_locked_state(|| {
            let table = DebugImageInfoData::new();
            assert_eq!(table.len(), 0);
            assert_eq!(table.capacity(), 0);
            assert!(table.table().is_empty());

            let locked_table = DebugImageInfoData::new_locked();
            let table_ref = locked_table.read();
            assert_eq!(table_ref.len(), 0);
            assert_eq!(table_ref.capacity(), 0);
            assert!(table_ref.table().is_empty());
        });
    }

    #[test]
    fn test_add_entry() {
        with_locked_state(|| {
            let mut table = DebugImageInfoData::new();

            assert_eq!(table.header.table_size, 0);
            assert_eq!(table.len(), 0);
            assert_eq!(table.capacity(), 0);

            table.add_entry(ImageInfoType::Normal, NonNull::dangling(), 0x1234 as efi::Handle);

            assert_eq!(table.header.table_size, 1);
            assert_eq!(table.len(), 1);
            assert_eq!(table.capacity(), DEFAULT_CAPACITY);
        });
    }

    #[test]
    fn test_add_entry_require_grow() {
        with_locked_state(|| {
            let mut table = DebugImageInfoData::new();

            let count = DEFAULT_CAPACITY + 1;
            for i in 0..count {
                table.add_entry(ImageInfoType::Normal, NonNull::dangling(), (0x1000 + i) as efi::Handle);
            }

            assert_eq!(table.header.table_size, count as u32);
            assert_eq!(table.len(), count);
            assert_eq!(table.capacity(), DEFAULT_CAPACITY * 2);
        });
    }

    #[test]
    fn test_search_entry() {
        with_locked_state(|| {
            let mut table = DebugImageInfoData::new();

            for i in 0..3 {
                table.add_entry(ImageInfoType::Normal, NonNull::dangling(), (0x2000 + i) as efi::Handle);
            }

            // Table has been modified
            assert_eq!(
                table.update_status() & efi::DEBUG_IMAGE_INFO_TABLE_MODIFIED,
                efi::DEBUG_IMAGE_INFO_TABLE_MODIFIED
            );
            assert_eq!(table.find(0x2000 as efi::Handle), Some(0));
            assert_eq!(table.find(0x2001 as efi::Handle), Some(1));
            assert_eq!(table.find(0x2002 as efi::Handle), Some(2));
            assert_eq!(table.find(0x3000 as efi::Handle), None);
        });
    }

    #[test]
    fn test_remove_entry_still_find_others() {
        with_locked_state(|| {
            let mut table = DebugImageInfoData::new();

            for i in 0..5 {
                table.add_entry(ImageInfoType::Normal, NonNull::dangling(), (0x4000 + i) as efi::Handle);
            }

            assert_eq!(table.len(), 5);

            // Find all entries
            assert_eq!(table.find(0x4000 as efi::Handle), Some(0));
            assert_eq!(table.find(0x4001 as efi::Handle), Some(1));
            assert_eq!(table.find(0x4002 as efi::Handle), Some(2));
            assert_eq!(table.find(0x4003 as efi::Handle), Some(3));
            assert_eq!(table.find(0x4004 as efi::Handle), Some(4));

            // Remove 0x4001, get swapped with 0x4005
            table.remove_entry(0x4001 as efi::Handle);
            assert_eq!(table.len(), 4);
            assert_eq!(table.find(0x4000 as efi::Handle), Some(0));
            assert_eq!(table.find(0x4004 as efi::Handle), Some(1));
            assert_eq!(table.find(0x4002 as efi::Handle), Some(2));
            assert_eq!(table.find(0x4003 as efi::Handle), Some(3));
            assert_eq!(table.find(0x4001 as efi::Handle), None);

            // Remove 0x4000, get swapped with 0x4003
            table.remove_entry(0x4000 as efi::Handle);
            assert_eq!(table.len(), 3);
            assert_eq!(table.find(0x4003 as efi::Handle), Some(0));
            assert_eq!(table.find(0x4004 as efi::Handle), Some(1));
            assert_eq!(table.find(0x4002 as efi::Handle), Some(2));
            assert_eq!(table.find(0x4000 as efi::Handle), None);
            assert_eq!(table.find(0x4001 as efi::Handle), None);

            // Remove 0x4002, does not swap as it's the last entry
            table.remove_entry(0x4002 as efi::Handle);
            assert_eq!(table.len(), 2);
            assert_eq!(table.find(0x4003 as efi::Handle), Some(0));
            assert_eq!(table.find(0x4004 as efi::Handle), Some(1));
            assert_eq!(table.find(0x4002 as efi::Handle), None);
            assert_eq!(table.find(0x4000 as efi::Handle), None);
            assert_eq!(table.find(0x4001 as efi::Handle), None);

            // Remove 0x4003, swaps with 0x4004
            table.remove_entry(0x4003 as efi::Handle);
            assert_eq!(table.len(), 1);
            assert_eq!(table.find(0x4004 as efi::Handle), Some(0));
            assert_eq!(table.find(0x4003 as efi::Handle), None);
            assert_eq!(table.find(0x4002 as efi::Handle), None);
            assert_eq!(table.find(0x4000 as efi::Handle), None);
            assert_eq!(table.find(0x4001 as efi::Handle), None);

            // Remove 0x4004, table is now empty
            table.remove_entry(0x4004 as efi::Handle);
            assert_eq!(table.len(), 0);
            assert_eq!(table.find(0x4004 as efi::Handle), None);
            assert_eq!(table.find(0x4003 as efi::Handle), None);
            assert_eq!(table.find(0x4002 as efi::Handle), None);
            assert_eq!(table.find(0x4000 as efi::Handle), None);
            assert_eq!(table.find(0x4001 as efi::Handle), None);
        });
    }

    #[test]
    fn test_swap_remove_out_of_bounds() {
        with_locked_state(|| {
            let mut table = DebugImageInfoData::new();

            for i in 0..3 {
                table.add_entry(ImageInfoType::Normal, NonNull::dangling(), (0x5000 + i) as efi::Handle);
            }

            assert_eq!(table.len(), 3);

            // Attempt to remove an out-of-bounds index
            assert!(table.swap_remove(3).is_none());
        });
    }

    #[test]
    fn test_flag_setting() {
        with_locked_state(|| {
            let mut table = DebugImageInfoData::new();

            assert_eq!(table.update_status(), 0);

            table.set_update_in_progress();
            assert_eq!(
                table.update_status() & efi::DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS,
                efi::DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS
            );

            table.clear_update_in_progress();
            assert_eq!(table.update_status() & efi::DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS, 0);

            table.set_modified();
            assert_eq!(
                table.update_status() & efi::DEBUG_IMAGE_INFO_TABLE_MODIFIED,
                efi::DEBUG_IMAGE_INFO_TABLE_MODIFIED
            );

            table.set_update_in_progress();
            assert_eq!(
                table.update_status() & efi::DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS,
                efi::DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS
            );
            assert_eq!(
                table.update_status() & efi::DEBUG_IMAGE_INFO_TABLE_MODIFIED,
                efi::DEBUG_IMAGE_INFO_TABLE_MODIFIED
            );

            table.clear_update_in_progress();
            assert_eq!(table.update_status() & efi::DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS, 0);
            assert_eq!(
                table.update_status() & efi::DEBUG_IMAGE_INFO_TABLE_MODIFIED,
                efi::DEBUG_IMAGE_INFO_TABLE_MODIFIED
            );
        });
    }

    #[test]
    fn test_grow_error_display_alloc_failed_msg() {
        with_locked_state(|| {
            let error = GrowError::AllocFailed;
            let msg = alloc::format!("{error}");
            assert_eq!(msg, "allocation returned null");
        });
    }

    #[test]
    fn test_grow_error_display_invalid_layout_msg() {
        with_locked_state(|| {
            // Note: Zero alignment will result in a LayoutError.
            let layout_err = Layout::from_size_align(1, 0).unwrap_err();
            let error = GrowError::InvalidLayout(layout_err);
            let msg = alloc::format!("{error}");
            assert!(msg.starts_with("invalid layout:"));
        });
    }

    #[test]
    fn test_grow_error_debug_msgs() {
        with_locked_state(|| {
            let error = GrowError::AllocFailed;
            let msg = alloc::format!("{error:?}");
            assert_eq!(msg, "AllocFailed");

            let layout_err = Layout::from_size_align(1, 0).unwrap_err();
            let error = GrowError::from(layout_err);
            let msg = alloc::format!("{error:?}");
            assert!(msg.starts_with("InvalidLayout("));
        });
    }

    #[test]
    fn test_grow_error_from_layout_error() {
        with_locked_state(|| {
            let layout_err = Layout::from_size_align(1, 0).unwrap_err();
            let error: GrowError = layout_err.into();
            assert!(matches!(error, GrowError::InvalidLayout(_)));
        });
    }

    #[test]
    fn test_grow_layout_overflow_preserves_state() {
        with_locked_state(|| {
            let mut table = DebugImageInfoData::new();

            // Add one entry to trigger the initial allocation (capacity = DEFAULT_CAPACITY).
            table.add_entry(ImageInfoType::Normal, NonNull::dangling(), 0x6000 as efi::Handle);
            assert_eq!(table.capacity(), DEFAULT_CAPACITY);
            assert_eq!(table.len(), 1);

            // Force capacity to a value that will overflow in grow().
            //   - new_capacity = capacity * 2
            //   - Layout::array::<efi::DebugImageInfo>(usize::MAX) will fail with LayoutError (since it is > isize::MAX)
            table.capacity = usize::MAX / 2;

            // grow() should fail with InvalidLayout and leave state unchanged.
            let result = table.grow();
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), GrowError::InvalidLayout(_)));

            // Verify that capacity was not modified on failure.
            assert_eq!(table.capacity, usize::MAX / 2);

            // Restore valid capacity so Drop doesn't dealloc with wrong layout.
            table.capacity = DEFAULT_CAPACITY;

            // The original entry should still be accessible.
            assert_eq!(table.len(), 1);
            assert_eq!(table.find(0x6000 as efi::Handle), Some(0));
        });
    }
}
