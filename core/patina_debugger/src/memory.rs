//! Implements memory operations for the Patina debugger.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use core::ptr;

use patina_paging::{MemoryAttributes, PageTable, page_allocator::PageAllocator};

use crate::arch::DebuggerArch;

const PAGE_SIZE: u64 = 0x1000;
const PAGE_MASK: u64 = !(PAGE_SIZE - 1);

/// Reads memory from the specified address into the buffer.
///
/// This will ensure that the address is valid in the page tables before reading.
/// If the base address is not valid, this will return an error. If the range is partially
/// valid then the valid portion will be read and the size of the valid region will
/// be returned.
///
pub fn read_memory<Arch: DebuggerArch>(address: u64, buffer: &mut [u8], unsafe_read: bool) -> Result<usize, ()> {
    let page_table = Arch::get_page_table()?;

    // Check that all of the pages are mapped before accessing the memory.
    let len = if !unsafe_read { check_range_accessibility(&page_table, address, buffer.len())? } else { buffer.len() };

    if len == 0 {
        return Err(());
    }

    let ptr = address as *const u8;
    unsafe {
        ptr::copy(ptr, buffer.as_mut_ptr(), len);
    }

    Ok(len)
}

/// Writes the buffer to the specified address.
///
/// This will ensure that the address is valid in the page tables before writing.
/// If the mapping is valid, but is not writable, this routine will temporarily
/// edit the page tables to allow the write to occur. If the mapping is not valid,
/// this will return an error.
///
pub fn write_memory<Arch: DebuggerArch>(address: u64, buffer: &[u8]) -> Result<(), ()> {
    let end_address = address + buffer.len() as u64;
    let mut page_table = Arch::get_page_table()?;

    // Check that all of the pages are mapped before accessing the memory.
    let valid_bytes = check_range_accessibility(&page_table, address, buffer.len())?;
    if valid_bytes != buffer.len() {
        return Err(());
    }

    // all pages are mapped. Edit one at a time, adding write permissions as needed.
    let mut current = address;
    while current < end_address {
        let page = current & PAGE_MASK;
        let end = (page + PAGE_SIZE).min(end_address);
        let len = (end - current) as usize;
        let offset = (current - address) as isize;

        // Check that this page is writable before writing. If it is not, then temporarily
        // modify the page table to allow writing.
        let attributes = page_table
            .query_memory_region(page, PAGE_SIZE)
            .expect("Unexpected failure on address that was already checked.");

        if attributes.contains(MemoryAttributes::ReadOnly) {
            page_table
                .remap_memory_region(page, PAGE_SIZE, attributes & !MemoryAttributes::ReadOnly)
                .map_err(|_| ())?;
        }

        let ptr = address as *mut u8;
        unsafe {
            ptr::copy_nonoverlapping(buffer.as_ptr().offset(offset), ptr, len);
        }

        if attributes.contains(MemoryAttributes::ReadOnly) {
            // Restore the original page attributes. Panic if this fails as the
            // debugger should not allow the system to continue if it's state cannot be restored.
            page_table
                .remap_memory_region(page, PAGE_SIZE, attributes)
                .map_err(|_| ())
                .expect("Failed to restore page table attributes!");
        }

        current += len as u64;
    }

    Ok(())
}

/// Checks that the range of memory is valid in the page tables. This ensures that
/// reads to this region will not fault. On success returns the number of bytes valid
/// to read from.
fn check_range_accessibility<P: PageTable>(page_table: &P, start_address: u64, length: usize) -> Result<usize, ()> {
    // This is done page-by-page because it is unknown if the memory region has
    // consistent attributes across the entire range.
    // The length takes us to the start of the next memory range, so we go until the end of the range, e.g
    // start_address + length - 1. This avoids overflow in the self map case
    let mut page = start_address & PAGE_MASK;
    while page <= start_address + (length - 1) as u64 {
        let res = page_table.query_memory_region(page, PAGE_SIZE).map_err(|_| ());
        let valid = match res {
            Ok(attributes) => !attributes.contains(MemoryAttributes::ReadProtect),
            Err(_) => false,
        };

        if !valid {
            // Only return a valid number if this isn't the first page, otherwise
            // return an error.
            if page > start_address {
                return Ok((page - start_address) as usize);
            } else {
                return Err(());
            }
        }

        // if this is the last page, return the full length
        match page.checked_add(PAGE_SIZE) {
            Some(next) => page = next,
            None => break,
        }
    }

    Ok(length)
}

/// Implements a page allocator for the debugger that will panic if allocations
/// are attempted.
pub struct DebugPageAllocator {}

impl PageAllocator for DebugPageAllocator {
    fn allocate_page(&mut self, _align: u64, _size: u64, _is_root: bool) -> patina_paging::PtResult<u64> {
        panic!("Should not allocate page tables from the debugger!");
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    use crate::*;
    use gdbstub::target::ext::breakpoints;
    use mockall::predicate::*;
    use mockall::*;
    use patina_paging::{MemoryAttributes, PtResult};

    mock! {
        pub MemPageTable {}

        impl PageTable for MemPageTable {
            fn map_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> PtResult<()>;
            fn unmap_memory_region(&mut self, address: u64, size: u64) -> PtResult<()>;
            fn remap_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> PtResult<()>;
            fn install_page_table(&mut self) -> PtResult<()>;
            fn query_memory_region(&self, address: u64, size: u64) -> PtResult<MemoryAttributes>;
            fn dump_page_tables(&self, address: u64, size: u64) -> PtResult<()>;
        }
    }

    mock! {
        pub MemDebuggerArch {}

        impl DebuggerArch for MemDebuggerArch {
            const DEFAULT_EXCEPTION_TYPES: &'static [usize] = &[];
            const BREAKPOINT_INSTRUCTION: &'static [u8] = &[];
            const GDB_TARGET_XML: &'static str = "";
            const GDB_REGISTERS_XML: &'static str = "";
            type PageTable = MockMemPageTable;

            fn breakpoint();
            fn process_entry(exception_type: u64, context: &mut ExceptionContext) -> ExceptionInfo;
            fn process_exit(exception_info: &mut ExceptionInfo);
            fn set_single_step(exception_info: &mut ExceptionInfo);
            fn initialize();
            fn add_watchpoint(address: u64, length: u64, access_type: breakpoints::WatchKind) -> bool;
            fn remove_watchpoint(address: u64, length: u64, access_type: breakpoints::WatchKind) -> bool;
            fn get_page_table() -> Result<MockMemPageTable, ()>;
            fn reboot();
        }
    }

    #[test]
    fn test_access_check_valid_page() {
        let mut mock_page_table = MockMemPageTable::new();
        mock_page_table.expect_query_memory_region().once().returning(|_, _| Ok(MemoryAttributes::empty()));

        let result = check_range_accessibility(&mock_page_table, 0, 0x1000);
        assert!(result.expect("Failed to check range access.") == 0x1000);
    }

    #[test]
    fn test_access_check_invalid_page() {
        let mut mock_page_table = MockMemPageTable::new();
        mock_page_table
            .expect_query_memory_region()
            .times(2)
            .returning(|_, _| Err(patina_paging::PtError::InvalidMemoryRange));

        let result = check_range_accessibility(&mock_page_table, 0, 0x1000);
        result.expect_err("Should have return a failure.");
        let result = check_range_accessibility(&mock_page_table, 0x800, 0x1000);
        result.expect_err("Should have return a failure.");
    }

    #[test]
    fn test_access_check_valid_range() {
        let mut mock_page_table = MockMemPageTable::new();
        mock_page_table.expect_query_memory_region().times(4).returning(|_, _| Ok(MemoryAttributes::empty()));

        let result = check_range_accessibility(&mock_page_table, 0x800, 0x3000);
        assert!(result.expect("Failed to check range access.") == 0x3000);
    }

    #[test]
    fn test_access_check_partially_valid_range() {
        let mut mock_page_table = MockMemPageTable::new();
        mock_page_table.expect_query_memory_region().times(2).returning(|_, _| Ok(MemoryAttributes::empty()));
        mock_page_table
            .expect_query_memory_region()
            .times(1)
            .returning(|_, _| Err(patina_paging::PtError::InvalidMemoryRange));

        let result = check_range_accessibility(&mock_page_table, 0x800, 0x3000);
        assert!(result.expect("Failed to check range access.") == 0x1800);
    }

    // This is an artifact of having to mock something that relies on static
    // architectural state.
    static PAGE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_read_memory_valid() {
        let data = [0xCF_u8];
        let mut buffer = [0_u8];

        let _lock = PAGE_LOCK.lock().unwrap();
        let ctx = MockMemDebuggerArch::get_page_table_context();
        ctx.expect().returning(|| {
            let mut mock_page_table = MockMemPageTable::new();
            mock_page_table.expect_query_memory_region().returning(|_, _| Ok(MemoryAttributes::ReadOnly));
            Ok(mock_page_table)
        });

        let address = &data as *const _ as u64;
        let result = read_memory::<MockMemDebuggerArch>(address, &mut buffer, false);
        assert!(result.expect("Failed to read memory.") == buffer.len());
        assert_eq!(buffer, data);
    }

    #[test]
    fn test_read_memory_invalid() {
        let mut buffer = [0_u8; 1];

        let _lock = PAGE_LOCK.lock().unwrap();
        let ctx = MockMemDebuggerArch::get_page_table_context();
        ctx.expect().returning(|| {
            let mut mock_page_table = MockMemPageTable::new();
            mock_page_table
                .expect_query_memory_region()
                .returning(|_, _| Err(patina_paging::PtError::InvalidMemoryRange));
            Ok(mock_page_table)
        });

        let result = read_memory::<MockMemDebuggerArch>(0, &mut buffer, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_write_memory_valid() {
        let data = [0_u8];
        let buffer = [0xCF_u8; 1];

        let _lock = PAGE_LOCK.lock().unwrap();
        let ctx = MockMemDebuggerArch::get_page_table_context();
        ctx.expect().returning(|| {
            let mut mock_page_table = MockMemPageTable::new();
            mock_page_table.expect_query_memory_region().returning(|_, _| Ok(MemoryAttributes::empty()));
            Ok(mock_page_table)
        });

        let address = &data as *const _ as u64;
        let result = write_memory::<MockMemDebuggerArch>(address, &buffer);
        assert!(result.is_ok());
        assert_eq!(buffer, data);
    }
}
