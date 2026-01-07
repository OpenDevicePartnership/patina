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
    ptr::{self, NonNull},
};
use r_efi::efi;
use spin::rwlock::RwLock;

/// GUID for the EFI_DEBUG_IMAGE_INFO_TABLE per section 18.4.3 of UEFI Spec 2.11
pub const EFI_DEBUG_IMAGE_INFO_TABLE_GUID: efi::Guid =
    efi::Guid::from_fields(0x49152e77, 0x1ada, 0x4764, 0xb7, 0xa2, &[0x7a, 0xfe, 0xfe, 0xd9, 0x5e, 0x8b]);

/// The global debug image info table instance.
pub static DEBUG_IMAGE_INFO_TABLE: RwLock<DebugImageInfoData> = DebugImageInfoData::new_locked();

/// The type of debug image info entry.
pub enum ImageInfoType {
    /// A normal debug image info entry.
    Normal,
}

impl ImageInfoType {
    /// The UEFI constant representing a normal debug image info entry.
    const EFI_DEBUG_IMAGE_INFO_TYPE_NORMAL: u32 = 0x1;
}

impl From<ImageInfoType> for u32 {
    fn from(value: ImageInfoType) -> Self {
        match value {
            ImageInfoType::Normal => ImageInfoType::EFI_DEBUG_IMAGE_INFO_TYPE_NORMAL,
        }
    }
}

/// Represents a table of debug image information entries.
///
/// ## Invariants
///
/// - `header.table_size` and `capacity` always reflect the actual state of the allocated bytes buffer pointed to by
///   `header.table`, ensuring no out-of-bounds access can occur.
///
/// ## Warning
///
/// The above invariants are only upheld on the assumption that this struct is the sole modifier of the underlying
/// table. This cannot be guaranteed due to the fact that the table pointer ([DebugImageInfoTableHeader]) is exposed
/// publicly via the UEFI configuration table mechanism. It is expected that this table is read-only when accessed
/// via this mechanism, but this cannot be enforced.
pub struct DebugImageInfoData {
    /// The header of the debug image info table, which is registered as a UEFI configuration table.
    header: DebugImageInfoTableHeader,
    /// The total number of [EfiDebugImageInfo] entries able to be added to the the table before an reallocation is
    /// needed.
    capacity: usize,
}

impl DebugImageInfoData {
    /// Creates a new, empty Debug Image Info Table.
    const fn new() -> Self {
        Self { header: DebugImageInfoTableHeader::new(), capacity: 0 }
    }

    /// Creates a new, empty Debug Image Info Table wrapped in a RwLock.
    const fn new_locked() -> RwLock<Self> {
        RwLock::new(Self::new())
    }

    /// Returns a reference to the header of the debug image info table.
    pub fn header(&self) -> &DebugImageInfoTableHeader {
        &self.header
    }

    /// Returns an immutable slice of table entries.
    ///
    /// This should be used for read-only access.
    fn table(&self) -> &[EfiDebugImageInfo] {
        self.header.table()
    }

    /// Returns a mutable pointer to the start of the debug image info table.
    ///
    /// This should be used for read-write access.
    fn table_mut(&mut self) -> *mut EfiDebugImageInfo {
        self.header.table_mut()
    }

    /// Returns the current number of entries in the table.
    fn len(&self) -> usize {
        self.header.len()
    }

    /// Returns the current capacity of the table.
    fn capacity(&self) -> usize {
        self.capacity
    }

    /// Sets the update-in-progress flag.
    fn set_update_in_progress(&mut self) {
        self.header.set_update_status(
            self.header.update_status() | DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS,
        )
    }

    fn clear_update_in_progress(&mut self) {
        self.header.set_update_status(
            self.header.update_status() & !DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS,
        );
    }

    /// Sets the table-modified flag.
    fn set_modified(&mut self) {
        let update_status = self.header.update_status();
        self.header.set_update_status(update_status | DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED)
    }

    /// Allows for safe modification of the table while managing update flags.
    fn modify(&mut self, f: impl FnOnce(&mut Self)) {
        self.set_update_in_progress();
        f(self);
        self.set_modified();
        self.clear_update_in_progress();
    }

    /// Adds a new entry to the debug image info table.
    pub fn add_entry(
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

            let entry = EfiDebugImageInfo::new(image_info_type, protocol, handle);

            // SAFETY: Invariants of this struct ensure this addition is within bounds and is aligned for
            //   EfiDebugImageInfo.
            unsafe { s.table_mut().cast::<EfiDebugImageInfo>().add(s.len()).write(entry) }
            s.header.table_size += 1;
        });
    }

    /// Removes the first entry matching the specified handle.
    pub fn remove_entry(&mut self, handle: efi::Handle) {
        self.modify(|s| {
            if let Some(index) = s.find(handle) {
                let _ = s.swap_remove(index);
            }
        });
    }

    /// Finds the index of the first entry matching the specified handle.
    fn find(&self, handle: efi::Handle) -> Option<usize> {
        for (index, entry) in self.table().iter().enumerate() {
            if let Some(entry_handle) = entry.handle()
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
    fn swap_remove(&mut self, index: usize) -> Option<EfiDebugImageInfo> {
        if index >= self.len() {
            return None;
        }

        let data = self.table_mut();

        // SAFETY: Invariants of this struct ensure that index is within bounds and aligned for EfiDebugImageInfo.
        let value = unsafe { core::ptr::read(data.add(index)) };

        let last = self.len() - 1;
        if index != last {
            unsafe { ptr::copy_nonoverlapping(data.add(last), data.add(index), 1) };
        }

        self.header.table_size -= 1;
        Some(value)
    }

    /// Doubles the current capacity of the table.
    ///
    /// If the current capacity is zero, sets it to a default initial capacity.
    fn grow(&mut self) -> Result<(), alloc::alloc::LayoutError> {
        // DEFAULT_CAPACITY must always be greater than zero.
        const DEFAULT_CAPACITY: usize = 16;
        const _: () = assert!(DEFAULT_CAPACITY > 0);

        let data = if self.capacity == 0 {
            let layout = Layout::array::<EfiDebugImageInfo>(DEFAULT_CAPACITY)?;
            self.capacity = DEFAULT_CAPACITY;

            // SAFETY: layout is non-zero sized due to DEFAULT_CAPACITY being non-zero
            unsafe { alloc::alloc::alloc_zeroed(layout) }
        } else {
            let old_layout = Layout::array::<EfiDebugImageInfo>(self.capacity)?;
            let new_capacity = self.capacity * 2;
            let new_layout = Layout::array::<EfiDebugImageInfo>(new_capacity)?;
            self.capacity = new_capacity;
            // SAFETY: layout is the same layout that was used to allocate the original buffer due to the invariants
            //   of this struct.
            // SAFETY: new_size is greater than zero due to the if branch above ensuring capacity is non-zero.
            // SAFETY: new_size does not exceed isize::MAX as the `Layout` call would have failed.
            unsafe { alloc::alloc::realloc(self.table_mut().cast::<u8>(), old_layout, new_layout.size()) }
        };

        self.header.table = data as *mut EfiDebugImageInfo;
        Ok(())
    }
}

impl Drop for DebugImageInfoData {
    fn drop(&mut self) {
        // Call drop on each entry in the table
        let data = self.table_mut();
        for i in 0..self.len() {
            // SAFETY: Invariants of this struct meet the requirements of drop_in_place. e.g.
            //   - data[i] is owned by this struct and is valid for both reads and writes.
            //   - data[i] is properly aligned for EfiDebugImageInfo.
            //   - data[i] is non-null.
            //   - data[i] is initialized and thus valid for dropping.
            unsafe { core::ptr::drop_in_place(data.add(i)) };
        }

        // Deallocate the data
        if self.capacity > 0 {
            let layout = Layout::array::<EfiDebugImageInfo>(self.capacity).unwrap();
            unsafe {
                // SAFETY: Invariants of this struct ensure that `data` was allocated with this exact layout.
                alloc::alloc::dealloc(self.table_mut().cast::<u8>(), layout);
            }
        }
    }
}

/// The header structure for the UEFI Debug Image Info Table.
///
/// ## Invariants
///
/// - `table` is either null or points to a valid array of `EfiDebugImageInfo` entries of length `table_size`.
/// - `table_size` accurately reflects the number of valid entries in `table`.
#[repr(C)]
pub struct DebugImageInfoTableHeader {
    update_status: u32,
    table_size: u32,
    table: *mut EfiDebugImageInfo,
}

impl DebugImageInfoTableHeader {
    /// Status flag indicating an update is in progress.
    pub const EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS: u32 = 0x1;

    /// Status flag indicating the table has been modified.
    pub const EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED: u32 = 0x2;

    /// Creates a new, empty Debug Image Info Table Header.
    const fn new() -> Self {
        Self { update_status: 0, table_size: 0, table: ptr::null_mut() }
    }

    /// Returns the current update status.
    fn update_status(&self) -> u32 {
        // SAFETY: This is a field owned by this struct and is valid for reads.
        unsafe { ptr::read_volatile(&self.update_status) }
    }

    /// Sets the update status.
    fn set_update_status(&mut self, status: u32) {
        // SAFETY: This is a field owned by this struct and is valid for writes.
        unsafe { ptr::write_volatile(&mut self.update_status, status) }
    }

    /// Returns the current number of entries in the table.
    fn len(&self) -> usize {
        self.table_size as usize
    }

    /// Returns a mutable pointer to the start of the debug image info table.
    fn table_mut(&mut self) -> *mut EfiDebugImageInfo {
        // SAFETY: This is a field owned by this struct and is valid for reads.
        self.table
    }

    /// Returns a reference to the debug image info table.
    fn table(&self) -> &[EfiDebugImageInfo] {
        if self.table_size == 0 {
            return &[];
        }

        // SAFETY: self.table is non-null due to the table_size check above.
        // SAFETY: self.table is valid for reads of length table_size and is within a single allocation due to the
        //   invariants of this struct
        unsafe { core::slice::from_raw_parts(self.table, self.table_size as usize) }
    }
}

// SAFETY: Access to the mutable pointer is gated behind methods that requires &mut self.
unsafe impl Send for DebugImageInfoTableHeader {}
// SAFETY: Access to the mutable pointer is gated behind methods that requires &mut self.
unsafe impl Sync for DebugImageInfoTableHeader {}

/// Structure for a normal debug image info entry, per section 18.4.3 of UEFI Spec 2.11.
#[repr(C)]
struct EfiDebugImageInfoNormal {
    /// The type of debug image info entry. Will be `EFI_DEBUG_IMAGE_INFO_TYPE_NORMAL` for this structure.
    image_info_type: u32,
    /// Pointer to the loaded image protocol instance for the image.
    loaded_image_protocol_instance: *mut efi::protocols::loaded_image::Protocol,
    /// The handle of the image.
    image_handle: efi::Handle,
}

/// A union representing different types of debug image info entries.
///
/// each variant must start with a `u32` field representing the image info type, which is used to
/// determine the actual type of the entry to access.
///
/// At present, only one type is defined: EfiDebugImageInfoNormal.
#[repr(C)]
union EfiDebugImageInfo {
    /// Pointer to the image info type field of all variants.
    image_info_type: *const u32,
    /// A normal debug image info entry if the type is `EFI_DEBUG_IMAGE_INFO_TYPE_NORMAL`.
    normal: *const EfiDebugImageInfoNormal,
}

impl EfiDebugImageInfo {
    /// Creates a new EfiDebugImageInfo instance of type normal.
    ///
    /// This allocates memory for the internal EfiDebugImageInfoNormal structure.
    fn new(
        image_info_type: ImageInfoType,
        protocol: NonNull<efi::protocols::loaded_image::Protocol>,
        handle: efi::Handle,
    ) -> Self {
        Self {
            normal: alloc::boxed::Box::into_raw(alloc::boxed::Box::new(EfiDebugImageInfoNormal {
                image_info_type: image_info_type.into(),
                loaded_image_protocol_instance: protocol.as_ptr(),
                image_handle: handle,
            })),
        }
    }

    /// Returns the handle associated with this debug image info entry, if the entry type has one.
    fn handle(&self) -> Option<efi::Handle> {
        // SAFETY: The invariants of this struct ensure that this pointer is in-fact a valid pointer to EfiDebugImageInfoNormal.
        let normal = unsafe { &*self.normal };
        Some(normal.image_handle)
    }
}

impl Drop for EfiDebugImageInfo {
    fn drop(&mut self) {
        // SAFETY: The invariants of this struct ensure that this pointer was allocated via this instance of EfiDebugImageInfo.
        let normal = unsafe { alloc::boxed::Box::from_raw(self.normal as *mut EfiDebugImageInfoNormal) };
        drop(normal);
    }
}

/// Structure for the EFI_SYSTEM_TABLE_POINTER, per section 18.4.2 of UEFI Spec 2.11.
#[repr(C)]
pub struct EfiSystemTablePointer {
    /// The signature of the system table pointer structure. Must be `EFI_SYSTEM_TABLE_SIGNATURE`.
    pub signature: u64,
    /// The physical address of the EFI system table.
    pub efi_system_table_base: efi::PhysicalAddress,
    /// The CRC32 checksum of the EFI system table pointer structure.
    pub crc32: u32,
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_init_simple() {
        let table = DebugImageInfoData::new();
        assert_eq!(table.len(), 0);
        assert_eq!(table.capacity(), 0);
        assert!(table.table().is_empty());

        let locked_table = DebugImageInfoData::new_locked();
        let table_ref = locked_table.read();
        assert_eq!(table_ref.len(), 0);
        assert_eq!(table_ref.capacity(), 0);
        assert!(table_ref.table().is_empty());
    }

    #[test]
    fn test_add_entry() {
        let mut table = DebugImageInfoData::new();

        assert_eq!(table.header.table_size, 0);
        assert_eq!(table.len(), 0);
        assert_eq!(table.capacity(), 0);

        table.add_entry(ImageInfoType::Normal, NonNull::dangling(), 0x1234 as efi::Handle);

        assert_eq!(table.header.table_size, 1);
        assert_eq!(table.len(), 1);
        assert_eq!(table.capacity(), 16); // DEFAULT_CAPACITY
    }

    #[test]
    fn test_add_entry_require_grow() {
        let mut table = DebugImageInfoData::new();

        for i in 0..17 {
            table.add_entry(ImageInfoType::Normal, NonNull::dangling(), (0x1000 + i) as efi::Handle);
        }

        assert_eq!(table.header.table_size, 17);
        assert_eq!(table.len(), 17);
        assert_eq!(table.capacity(), 32); // Grew from 16 to 32
    }

    #[test]
    fn test_search_entry() {
        let mut table = DebugImageInfoData::new();

        for i in 0..3 {
            table.add_entry(ImageInfoType::Normal, NonNull::dangling(), (0x2000 + i) as efi::Handle);
        }

        // Table has been modified
        assert_eq!(
            table.header.update_status & DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED,
            DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED
        );
        assert_eq!(table.find(0x2000 as efi::Handle), Some(0));
        assert_eq!(table.find(0x2001 as efi::Handle), Some(1));
        assert_eq!(table.find(0x2002 as efi::Handle), Some(2));
        assert_eq!(table.find(0x3000 as efi::Handle), None);
    }

    #[test]
    fn test_remove_entry_still_find_others() {
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
    }

    #[test]
    fn test_swap_remove_out_of_bounds() {
        let mut table = DebugImageInfoData::new();

        for i in 0..3 {
            table.add_entry(ImageInfoType::Normal, NonNull::dangling(), (0x5000 + i) as efi::Handle);
        }

        assert_eq!(table.len(), 3);

        // Attempt to remove an out-of-bounds index
        assert!(table.swap_remove(3).is_none());
    }

    #[test]
    fn test_flag_setting() {
        let mut table = DebugImageInfoData::new();

        assert_eq!(table.header.update_status(), 0);

        table.set_update_in_progress();
        assert_eq!(
            table.header.update_status() & DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS,
            DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS
        );

        table.clear_update_in_progress();
        assert_eq!(
            table.header.update_status() & DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS,
            0
        );

        table.set_modified();
        assert_eq!(
            table.header.update_status() & DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED,
            DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED
        );

        table.set_update_in_progress();
        assert_eq!(
            table.header.update_status() & DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS,
            DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS
        );
        assert_eq!(
            table.header.update_status() & DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED,
            DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED
        );

        table.clear_update_in_progress();
        assert_eq!(
            table.header.update_status() & DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS,
            0
        );
        assert_eq!(
            table.header.update_status() & DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED,
            DebugImageInfoTableHeader::EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED
        );
    }
}
