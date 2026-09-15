//! Reserved-memory long-mode parking environment for application processors.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    arch::global_asm,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
};

use patina::{SIZE_16KB, UEFI_PAGE_SIZE, error::EfiError};
use patina_paging::{
    MemoryAttributes, PageTable, PagingType, PtError, page_allocator::PageAllocator, x64::X64PageTable,
};

use crate::{
    gdt::{CODE_SELECTOR, DescriptorTablePointer},
    interrupts::x64::idt::{Idt, IdtEntry},
};

pub(super) const PAGE_COUNT: usize = 7;
// A 16Kb alignment ensures  that the 3 mapped pages will be in the same leaf PT.
pub(super) const ALIGNMENT: usize = SIZE_16KB;
pub(super) const CODE_PAGE_INDEX: usize = 0;
pub(super) const DATA_PAGE_INDEX: usize = 1;
const IDT_PAGE_INDEX: usize = 2;
const PT_START_INDEX: usize = 3;
const PT_PAGE_COUNT: usize = PAGE_COUNT - PT_START_INDEX;

/// Data used by the reserved parking environment.
#[repr(C)]
struct ParkData {
    cr3: u64,
    gdtr: DescriptorTablePointer,
    idtr: DescriptorTablePointer,
    stack_top: u64,
    parked_count: AtomicU32,
    gdt: [u64; 8],
}

const _: () = assert!(core::mem::size_of::<Idt>() == UEFI_PAGE_SIZE);
const _: () = assert!(core::mem::size_of::<ParkData>() < UEFI_PAGE_SIZE / 2);

global_asm!(
    include_str!("park.asm"),
    data_cr3_off = const core::mem::offset_of!(ParkData, cr3),
    data_gdtr_off = const core::mem::offset_of!(ParkData, gdtr),
    data_idtr_off = const core::mem::offset_of!(ParkData, idtr),
    data_stack_top_off = const core::mem::offset_of!(ParkData, stack_top),
    data_parked_count_off = const core::mem::offset_of!(ParkData, parked_count),
);

unsafe extern "C" {
    fn ap_park_stub_start();
    fn ap_park_exception();
    fn ap_park_stub_end();

    /// Parks the current processor in the reserved environment and never returns.
    pub(super) fn ap_park() -> !;
}

/// Reserved entry address consumed by the handoff assembly when no AP context exists.
#[unsafe(no_mangle)]
pub(super) static AP_PARK_ENTRY: AtomicU64 = AtomicU64::new(0);

/// Reserved configuration address consumed by the handoff assembly.
#[unsafe(no_mangle)]
pub(super) static AP_PARK_CONFIG: AtomicU64 = AtomicU64::new(0);

fn page_address(base: usize, index: usize) -> usize {
    base + index * UEFI_PAGE_SIZE
}

fn page_mut(pages: &mut [u8], index: usize) -> Result<&mut [u8], EfiError> {
    let start = index.checked_mul(UEFI_PAGE_SIZE).ok_or(EfiError::InvalidParameter)?;
    pages.get_mut(start..start + UEFI_PAGE_SIZE).ok_or(EfiError::InvalidParameter)
}

fn stub_buffer() -> &'static [u8] {
    let start = ap_park_stub_start as *const () as usize;
    let end = ap_park_stub_end as *const () as usize;
    let size = end - start;
    // SAFETY: The linker-provided start/end symbols delimit the park assembly blob.
    unsafe { core::slice::from_raw_parts(start as *const u8, size) }
}

fn stub_exception_offset() -> usize {
    let start = ap_park_stub_start as *const () as usize;
    let exception = ap_park_exception as *const () as usize;
    exception - start
}

pub(super) fn prepare(pages: &mut [u8]) -> Result<(), EfiError> {
    let base = pages.as_ptr() as usize;
    if pages.len() != PAGE_COUNT * UEFI_PAGE_SIZE || !base.is_multiple_of(ALIGNMENT) {
        log::error!(
            "AP park allocation must be {PAGE_COUNT} pages aligned to {ALIGNMENT:#x}; got {:#x} bytes at {base:#x}",
            pages.len()
        );
        return Err(EfiError::InvalidParameter);
    }

    pages.fill(0);

    let code = page_address(base, CODE_PAGE_INDEX);
    let data = page_address(base, DATA_PAGE_INDEX);
    let idt = page_address(base, IDT_PAGE_INDEX);
    let page_tables = page_address(base, PT_START_INDEX);

    // Setup the park page tables to map the code/data pages.
    setup_park_page_tables(page_tables, PT_PAGE_COUNT, code, data, idt).map_err(|_| EfiError::InvalidParameter)?;

    // Copy the park stub into the code page.
    let stub_buffer = stub_buffer();
    let destination = page_mut(pages, CODE_PAGE_INDEX)?.get_mut(..stub_buffer.len()).ok_or(EfiError::BadBufferSize)?;
    destination.copy_from_slice(stub_buffer);

    // Setup the park IDT.
    let exception_entry = code + stub_exception_offset();
    let idt_entry = IdtEntry::interrupt_gate(exception_entry as u64, CODE_SELECTOR, 0);
    let idt_page = page_mut(pages, IDT_PAGE_INDEX)?;
    for entry in idt_page.chunks_exact_mut(core::mem::size_of::<IdtEntry>()) {
        // SAFETY: The page base and each entry-sized offset are aligned for `IdtEntry`.
        unsafe { entry.as_mut_ptr().cast::<IdtEntry>().write(idt_entry) };
    }

    // Set the data block for the park page.
    let park_data = ParkData {
        cr3: page_tables as u64,
        gdtr: DescriptorTablePointer {
            limit: (core::mem::size_of::<[u64; 8]>() - 1) as u16,
            base: (data + core::mem::offset_of!(ParkData, gdt)) as u64,
        },
        idtr: DescriptorTablePointer { limit: (core::mem::size_of::<Idt>() - 1) as u16, base: idt as u64 },
        stack_top: (data + UEFI_PAGE_SIZE) as u64,
        parked_count: AtomicU32::new(0),
        gdt: crate::gdt::minimal_long_mode_entries(),
    };

    // SAFETY: The data page is page-aligned, large enough for `ParkData`, and
    // remains writable until DXE applies its final runtime attributes.
    unsafe { page_mut(pages, DATA_PAGE_INDEX)?.as_mut_ptr().cast::<ParkData>().write(park_data) };

    Ok(())
}

fn setup_park_page_tables(
    page_tables: usize,
    table_count: usize,
    code_page: usize,
    data_page: usize,
    idt_page: usize,
) -> Result<(), PtError> {
    struct ParkPageAllocator {
        next: usize,
        remaining_pages: usize,
    }

    impl PageAllocator for ParkPageAllocator {
        fn allocate_page(&mut self, align: u64, size: u64, is_root: bool) -> Result<u64, PtError> {
            if align != UEFI_PAGE_SIZE as u64 || size != UEFI_PAGE_SIZE as u64 || is_root {
                return Err(PtError::InvalidParameter);
            }

            if self.remaining_pages == 0 {
                return Err(PtError::OutOfResources);
            }

            let page = self.next;
            self.next += UEFI_PAGE_SIZE;
            self.remaining_pages -= 1;
            Ok(page as u64)
        }
    }

    // Create an allocator patina-paging to use for the remaining non-root tables.
    let allocator = ParkPageAllocator { next: page_tables + UEFI_PAGE_SIZE, remaining_pages: table_count - 1 };

    // SAFETY: The root and allocator pages are valid and all 0.
    let mut page_table =
        unsafe { X64PageTable::from_existing(page_tables as u64, allocator, PagingType::Paging4Level) }?;

    page_table.map_memory_region(code_page as u64, UEFI_PAGE_SIZE as u64, MemoryAttributes::ReadOnly)?;

    page_table.map_memory_region(data_page as u64, UEFI_PAGE_SIZE as u64, MemoryAttributes::ExecuteProtect)?;

    page_table.map_memory_region(
        idt_page as u64,
        UEFI_PAGE_SIZE as u64,
        MemoryAttributes::ReadOnly | MemoryAttributes::ExecuteProtect,
    )?;

    Ok(())
}

pub(super) fn install(pages: &'static [u8]) -> Result<(), EfiError> {
    let base = pages.as_ptr() as usize;
    if pages.len() != PAGE_COUNT * UEFI_PAGE_SIZE || !base.is_multiple_of(ALIGNMENT) {
        return Err(EfiError::InvalidParameter);
    }
    let code = page_address(base, CODE_PAGE_INDEX);
    let data = page_address(base, DATA_PAGE_INDEX);

    AP_PARK_CONFIG.store(data as u64, Ordering::Release);
    AP_PARK_ENTRY.store(code as u64, Ordering::Release);
    Ok(())
}

fn data() -> Option<&'static ParkData> {
    let data = AP_PARK_CONFIG.load(Ordering::Acquire);
    if data == 0 {
        return None;
    }

    // SAFETY: `install` publishes the address of the persistent, correctly
    // aligned park data before any AP can access its counters.
    Some(unsafe { &*(data as *const ParkData) })
}

pub(super) fn parked_count() -> u32 {
    data().map_or(0, |data| data.parked_count.load(Ordering::Acquire))
}

#[cfg_attr(coverage, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    const PAGE_PRESENT: u64 = 1;
    const PAGE_WRITABLE: u64 = 1 << 1;
    const PAGE_NO_EXECUTE: u64 = 1 << 63;
    const PAGE_TABLE_ENTRY_COUNT: usize = UEFI_PAGE_SIZE / core::mem::size_of::<u64>();

    fn table_index(address: usize, shift: u32) -> usize {
        (address >> shift) & (PAGE_TABLE_ENTRY_COUNT - 1)
    }

    fn with_aligned_pages(test: impl FnOnce(&mut [u8])) {
        let size = PAGE_COUNT * UEFI_PAGE_SIZE;
        let mut storage = std::vec![0u8; ALIGNMENT + size];
        let base = storage.as_ptr() as usize;
        let aligned = base.next_multiple_of(ALIGNMENT);
        let offset = aligned - base;
        test(&mut storage[offset..offset + size]);
    }

    fn read_entry(page: &[u8], index: usize) -> u64 {
        let start = index * core::mem::size_of::<u64>();
        u64::from_ne_bytes(page[start..start + core::mem::size_of::<u64>()].try_into().unwrap())
    }

    #[test]
    fn park_prepare_rejects_incorrect_size() {
        with_aligned_pages(|pages| {
            let short_len = pages.len() - UEFI_PAGE_SIZE;
            let short = &mut pages[..short_len];
            assert_eq!(prepare(short), Err(EfiError::InvalidParameter));
        });
    }

    #[test]
    fn park_prepare_maps_only_runtime_pages_with_expected_permissions() {
        with_aligned_pages(|pages| {
            prepare(pages).unwrap();
            let base = pages.as_ptr() as usize;
            let pt_index = PT_START_INDEX + PT_PAGE_COUNT - 1;
            let pt = &pages[pt_index * UEFI_PAGE_SIZE..(pt_index + 1) * UEFI_PAGE_SIZE];
            let address_mask = 0x000F_FFFF_FFFF_F000;

            for index in 0..PAGE_COUNT {
                let address = page_address(base, index);
                let entry = read_entry(pt, table_index(address, 12));
                if index <= IDT_PAGE_INDEX {
                    assert_eq!(entry & address_mask, address as u64);
                    assert_ne!(entry & PAGE_PRESENT, 0);
                    assert_eq!(entry & PAGE_WRITABLE != 0, index == DATA_PAGE_INDEX);
                    assert_eq!(entry & PAGE_NO_EXECUTE == 0, index == CODE_PAGE_INDEX);
                } else {
                    assert_eq!(entry, 0);
                }
            }
        });
    }
}
