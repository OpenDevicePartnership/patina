//! Real-mode AP bootstrap used by INIT-SIPI-SIPI path.
//!
//! Once in long mode, hands off to the `ap_setup` module for
//! convergence with live-handoff cores.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::arch::global_asm;

use patina::{
    SIZE_1MB, UEFI_PAGE_SHIFT, UEFI_PAGE_SIZE, arch::x64::read_msr, bit, error::EfiError, uefi_pages_to_size,
};

pub(super) const PAGE_COUNT: usize = 1;
pub(super) const MAX_ADDRESS: usize = SIZE_1MB - 1;

const CODE32_SELECTOR: u16 = 0x08;
const DATA32_SELECTOR: u16 = 0x10;
const CODE64_SELECTOR: u16 = 0x18;
const BOOTSTRAP_DATA_OFFSET: usize = 0x800;
const IA32_EFER: u32 = 0xC000_0080;
const EFER_LONG_MODE_ACTIVE: u64 = bit!(10);
const CR4_PAE: u64 = bit!(5);
const CR4_LA57: u64 = bit!(12);

const GDT_NULL: u64 = 0;
const GDT_CODE32: u64 = 0x00CF_9B00_0000_FFFF;
const GDT_DATA32: u64 = 0x00CF_9300_0000_FFFF;
const GDT_CODE64: u64 = 0x00AF_9B00_0000_FFFF;

#[derive(Clone, Copy)]
#[repr(C, packed)]
struct Gdtr32 {
    limit: u16,
    base: u32,
}

#[derive(Clone, Copy)]
#[repr(C, packed)]
struct FarPointer32 {
    offset: u32,
    selector: u16,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct BootstrapData {
    gdtr: Gdtr32,
    long_mode: FarPointer32,
    cr3: u32,
    cr0: u32,
    cr4: u32,
    efer: u64,
    entry: u64,
    gdt: [u64; 4],
}

const DATA_GDTR_OFFSET: usize = core::mem::offset_of!(BootstrapData, gdtr);
const DATA_LONG_MODE_OFFSET: usize = core::mem::offset_of!(BootstrapData, long_mode);
const DATA_CR3_OFFSET: usize = core::mem::offset_of!(BootstrapData, cr3);
const DATA_CR0_OFFSET: usize = core::mem::offset_of!(BootstrapData, cr0);
const DATA_CR4_OFFSET: usize = core::mem::offset_of!(BootstrapData, cr4);
const DATA_EFER_OFFSET: usize = core::mem::offset_of!(BootstrapData, efer);
const DATA_ENTRY_OFFSET: usize = core::mem::offset_of!(BootstrapData, entry);
const DATA_GDT_OFFSET: usize = core::mem::offset_of!(BootstrapData, gdt);
const DATA_SIZE: usize = core::mem::size_of::<BootstrapData>();

global_asm!(
    include_str!("ap_bootstrap.asm"),
    ia32_efer = const IA32_EFER,
    code32_selector = const CODE32_SELECTOR,
    data32_selector = const DATA32_SELECTOR,
    bootstrap_data_off = const BOOTSTRAP_DATA_OFFSET,
    data_cr3_off = const DATA_CR3_OFFSET,
    data_cr0_off = const DATA_CR0_OFFSET,
    data_cr4_off = const DATA_CR4_OFFSET,
    data_efer_off = const DATA_EFER_OFFSET,
    data_long_mode_off = const DATA_LONG_MODE_OFFSET,
    data_entry_off = const DATA_ENTRY_OFFSET,
    data_size = const DATA_SIZE,
);

unsafe extern "C" {
    fn ap_bootstrap_start();
    fn ap_bootstrap_rm_page_base();
    fn ap_bootstrap_rm_gdtr_offset();
    fn ap_bootstrap_rm_pm_entry();
    fn ap_bootstrap_protected_mode();
    fn ap_bootstrap_long_mode();
    fn ap_bootstrap_data();
    fn ap_bootstrap_end();
}

fn template_offset(symbol: unsafe extern "C" fn()) -> usize {
    symbol as *const () as usize - ap_bootstrap_start as *const () as usize
}

fn template() -> &'static [u8] {
    let start = ap_bootstrap_start as *const () as usize;
    let size = ap_bootstrap_end as *const () as usize - start;
    // SAFETY: The assembly start/end symbols delimit the complete bootstrap template.
    unsafe { core::slice::from_raw_parts(start as *const u8, size) }
}

fn patch_template<T: Copy>(page: &mut [u8], at: usize, value: T) -> Result<(), EfiError> {
    let end = at.checked_add(core::mem::size_of::<T>()).ok_or(EfiError::InvalidParameter)?;
    let destination = page.get_mut(at..end).ok_or(EfiError::InvalidParameter)?;
    // SAFETY: The destination has exactly enough writable bytes. Unaligned writes
    // are required for immediate values embedded in the real-mode instruction stream.
    unsafe { destination.as_mut_ptr().cast::<T>().write_unaligned(value) };
    Ok(())
}

pub(super) fn prepare(page: &mut [u8]) -> Result<(), EfiError> {
    let base = page.as_ptr() as usize;
    if page.len() != uefi_pages_to_size!(PAGE_COUNT) || !base.is_multiple_of(UEFI_PAGE_SIZE) || base > MAX_ADDRESS {
        log::error!("AP bootstrap requires one page below 1MB. Got {:#x} bytes at {base:#x}.", page.len());
        return Err(EfiError::InvalidParameter);
    }

    let template = template();
    let destination = page.get_mut(..template.len()).ok_or_else(|| {
        log::error!("AP bootstrap template does not fit in one page");
        EfiError::BadBufferSize
    })?;
    destination.copy_from_slice(template);

    let cr0: u64;
    let cr3: u64;
    let cr4: u64;
    // SAFETY: Reading control registers has no side effects.
    unsafe {
        core::arch::asm!(
            "mov {0}, cr0",
            "mov {1}, cr3",
            "mov {2}, cr4",
            out(reg) cr0,
            out(reg) cr3,
            out(reg) cr4,
            options(nomem, nostack, preserves_flags),
        );
    }
    let transition_cr3 = cr3 & !0xFFF;
    let transition_cr3 = u32::try_from(transition_cr3).map_err(|_| {
        log::error!("x64 MP Services requires a 32-bit-addressable root page table. CR3 is {cr3:#x}.");
        EfiError::Unsupported
    })?;

    // Setup the data section of the bootstrap stub.
    let data_offset = template_offset(ap_bootstrap_data);
    if data_offset != BOOTSTRAP_DATA_OFFSET {
        return Err(EfiError::BadBufferSize);
    }

    let gdt_address = base + data_offset + DATA_GDT_OFFSET;
    let data = BootstrapData {
        gdtr: Gdtr32 { limit: (core::mem::size_of::<[u64; 4]>() - 1) as u16, base: gdt_address as u32 },
        long_mode: FarPointer32 {
            offset: (base + template_offset(ap_bootstrap_long_mode)) as u32,
            selector: CODE64_SELECTOR,
        },
        cr3: transition_cr3,
        cr0: cr0 as u32,
        cr4: (cr4 & (CR4_PAE | CR4_LA57)) as u32,
        // LMA is read-only and becomes active after paging is enabled.
        // SAFETY: IA32_EFER is available on x86_64 processors.
        efer: unsafe { read_msr(IA32_EFER) } & !EFER_LONG_MODE_ACTIVE,
        entry: super::ap_setup::ap_entry_addr() as u64,
        gdt: [GDT_NULL, GDT_CODE32, GDT_DATA32, GDT_CODE64],
    };

    // Patch all of the address immediate values in the RM stub.
    patch_template(page, template_offset(ap_bootstrap_rm_page_base), base as u32)?;
    patch_template(
        page,
        template_offset(ap_bootstrap_rm_gdtr_offset),
        u16::try_from(data_offset + DATA_GDTR_OFFSET).map_err(|_| EfiError::BadBufferSize)?,
    )?;
    patch_template(
        page,
        template_offset(ap_bootstrap_rm_pm_entry),
        (base + template_offset(ap_bootstrap_protected_mode)) as u32,
    )?;
    patch_template(page, data_offset, data)?;

    log::info!("AP bootstrap page setup at 0x{base:#x}");
    Ok(())
}

pub(super) fn startup_vector(page: &[u8]) -> Result<u8, EfiError> {
    let base = page.as_ptr() as usize;
    if page.len() != uefi_pages_to_size!(PAGE_COUNT) || !base.is_multiple_of(UEFI_PAGE_SIZE) {
        return Err(EfiError::InvalidParameter);
    }
    u8::try_from(base >> UEFI_PAGE_SHIFT).map_err(|_| EfiError::InvalidParameter)
}

#[cfg_attr(coverage, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_template_fits_in_one_page() {
        assert!(template().len() <= UEFI_PAGE_SIZE);
        assert_eq!(&template()[..4], &[0xFA, 0xFC, 0x8C, 0xC8]);
    }

    #[test]
    fn test_ap_bootstrap_offsets_match_layout() {
        assert_eq!(template_offset(ap_bootstrap_data), BOOTSTRAP_DATA_OFFSET);
        assert_eq!(template().len(), BOOTSTRAP_DATA_OFFSET + DATA_SIZE);
    }
}
