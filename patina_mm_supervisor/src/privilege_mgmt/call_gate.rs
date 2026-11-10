//! Call gate and TSS management.
//!
//! Programs the call gate and Task State Segment (TSS) entries that a demoted Ring 3
//! routine uses to return to Ring 0. This return path is separate from syscall/sysret.
//!
//! The supervisor does not build the GDT it programs here. Each core's SMI entry code, which is
//! placed at SMBASE + 0x8000 before the supervisor starts, loads a GDT whose layout is defined
//! in `SmiException.nasm` (`MmSupervisorPkg/Core/Relocate/SmiException.nasm` in
//! [mu_feature_mm_supv](https://github.com/microsoft/mu_feature_mm_supv)). The supervisor keeps
//! that GDT loaded and finds it through GDTR, so the offsets below must match that file.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina_internal_cpu::gdt::TaskStateSegment;
use patina_paging::x64::{disable_write_protection, enable_write_protection};
use zerocopy::{FromBytes, Immutable, IntoBytes};

use crate::error::MmSupervisorResult;

// The transfer assembly is built only for the UEFI target so host builds (tests, doctests) link.
#[cfg(target_os = "uefi")]
core::arch::global_asm!(include_str!("call_gate_transfer.asm"));

/// Long mode Ring 0 code segment selector in the SMI entry GDT.
pub const LONG_CS_R0: u16 = 0x38;

/// Call gate descriptor offset in the SMI entry GDT.
pub const CALL_GATE_OFFSET: u16 = 0x60;

/// Offset of the TSS descriptor in the GDT (also its selector).
pub const TSS_SEL_OFFSET: u16 = 0x70;

/// Offset of the TSS itself, which follows the GDT entries in the same buffer.
pub const TSS_DESC_OFFSET: u16 = 0x80;

/// Type and attribute byte of the return call gate: present (P = 1), callable from Ring 3
/// (DPL = 3) and of type 0xC (64-bit call gate).
const CALL_GATE_TYPE_ATTR: u8 = 0xEC;

/// Number of bytes of the GDT image that [`setup_call_gate`] programs, up to and including the
/// TSS that follows the descriptors.
///
/// The platform places the TSS right after the last GDT descriptor (see `SmiException.nasm`),
/// so this window intentionally extends past the GDTR limit, which covers only the descriptors.
const GDT_PROGRAMMED_SIZE: usize = TSS_DESC_OFFSET as usize + core::mem::size_of::<TaskStateSegment>();

/// Errors that can occur while programming the privilege transition entries of a GDT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallGateError {
    /// The GDT image does not contain the entries that need to be programmed.
    GdtTooSmall,
}

impl core::error::Error for CallGateError {}

impl core::fmt::Display for CallGateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::GdtTooSmall => {
                write!(f, "the GDT image does not contain the privilege transition entries to program")
            }
        }
    }
}

/// 64-bit call gate descriptor.
///
/// A far call through a call gate changes privilege level.
#[repr(C, packed)]
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    zerocopy_derive::FromBytes,
    zerocopy_derive::IntoBytes,
    zerocopy_derive::Immutable
)]
pub struct CallGateDescriptor {
    /// Offset bits 15:0
    pub offset_low: u16,
    /// Target code segment selector
    pub selector: u16,
    /// Reserved. 64-bit call gates have no IST field; this byte is kept as the GDT was built.
    pub ist: u8,
    /// Type (0xC = 64-bit call gate) and DPL
    pub type_attr: u8,
    /// Offset bits 31:16
    pub offset_mid: u16,
    /// Offset bits 63:32
    pub offset_high: u32,
    /// Reserved (must be 0)
    pub reserved: u32,
}

impl CallGateDescriptor {
    /// Sets the target offset in the descriptor.
    pub fn set_offset(&mut self, offset: u64) {
        self.offset_low = (offset & 0xFFFF) as u16;
        self.offset_mid = ((offset >> 16) & 0xFFFF) as u16;
        self.offset_high = ((offset >> 32) & 0xFFFFFFFF) as u32;
    }

    /// Returns the target offset stored in the descriptor.
    ///
    /// Only used by the unit tests, which read back what [`Self::set_offset`] encoded.
    #[cfg(test)]
    pub fn offset(&self) -> u64 {
        u64::from(self.offset_high) << 32 | u64::from(self.offset_mid) << 16 | u64::from(self.offset_low)
    }

    /// Configures the descriptor as the present, Ring 3 callable call gate used to return to
    /// Ring 0 at `return_pointer`.
    pub fn set_return_gate(&mut self, return_pointer: u64) {
        self.set_offset(return_pointer);
        self.selector = LONG_CS_R0;
        self.type_attr = CALL_GATE_TYPE_ATTR;
    }
}

/// 64-bit TSS descriptor (16 bytes in 64-bit mode).
#[repr(C, packed)]
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    zerocopy_derive::FromBytes,
    zerocopy_derive::IntoBytes,
    zerocopy_derive::Immutable
)]
pub struct TssDescriptor {
    /// Limit bits 15:0
    pub limit_low: u16,
    /// Base bits 15:0
    pub base_low: u16,
    /// Base bits 23:16
    pub base_mid_low: u8,
    /// Type and attributes
    pub type_attr: u8,
    /// Limit bits 19:16 and flags
    pub limit_flags: u8,
    /// Base bits 31:24
    pub base_mid_high: u8,
    /// Base bits 63:32
    pub base_high: u32,
    /// Reserved
    pub reserved: u32,
}

impl TssDescriptor {
    /// Sets the base address in the descriptor.
    pub fn set_base(&mut self, base: u64) {
        self.base_low = (base & 0xFFFF) as u16;
        self.base_mid_low = ((base >> 16) & 0xFF) as u8;
        self.base_mid_high = ((base >> 24) & 0xFF) as u8;
        self.base_high = ((base >> 32) & 0xFFFFFFFF) as u32;
    }

    /// Returns the base address stored in the descriptor.
    ///
    /// Only used by the unit tests, which read back what [`Self::set_base`] encoded.
    #[cfg(test)]
    pub fn base(&self) -> u64 {
        u64::from(self.base_high) << 32
            | u64::from(self.base_mid_high) << 24
            | u64::from(self.base_mid_low) << 16
            | u64::from(self.base_low)
    }
}

/// In-memory image of the GDTR, as stored by sgdt.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GdtRegister {
    /// Size of the GDT minus 1
    pub limit: u16,
    /// Linear address of the GDT
    pub base: u64,
}

/// Returns the base address of the current GDT, read from GDTR.
// Reads the running processor's GDTR, so it cannot be tested deterministically on the host.
pub fn get_current_gdt_base() -> u64 {
    let mut gdtr = GdtRegister::default();
    // SAFETY: sgdt only stores GDTR into `gdtr` and has no other side effects.
    unsafe {
        core::arch::asm!(
            "sgdt [{}]",
            in(reg) &raw mut gdtr,
            options(nostack, preserves_flags)
        );
    }

    gdtr.base
}

/// Reads the entry of type `T` at `offset` in `gdt`, hands it to `update`, and writes it back.
///
/// Reading first keeps the fields the supervisor does not program (for example the reserved
/// byte of the call gate) as the GDT was built.
fn update_entry<T, F>(gdt: &mut [u8], offset: usize, update: F) -> MmSupervisorResult<()>
where
    T: FromBytes + IntoBytes + Immutable,
    F: FnOnce(&mut T),
{
    let gdt_len = gdt.len();
    let end = offset.checked_add(core::mem::size_of::<T>()).ok_or_else(|| {
        log::error!("GDT entry at offset {offset} overflows the address space");
        CallGateError::GdtTooSmall
    })?;
    let bytes = gdt.get_mut(offset..end).ok_or_else(|| {
        log::error!("GDT entry at offset {offset}..{end} runs past the end of the {gdt_len} byte GDT");
        CallGateError::GdtTooSmall
    })?;

    let mut entry = T::read_from_bytes(bytes).map_err(|_| {
        log::error!("GDT entry at offset {offset} could not be read");
        CallGateError::GdtTooSmall
    })?;
    update(&mut entry);
    entry.write_to(bytes).map_err(|_| {
        log::error!("GDT entry at offset {offset} could not be written back");
        CallGateError::GdtTooSmall
    })?;

    Ok(())
}

/// Programs the entries a Ring 3 routine needs to get back into Ring 0.
///
/// `gdt` is the byte image of the GDT and `gdt_base` is its linear address, which the TSS
/// descriptor needs because it stores the linear address of the TSS. This programs:
///
/// - the call gate at [`CALL_GATE_OFFSET`], targeting `return_pointer` in the Ring 0 code segment,
/// - the TSS descriptor at [`TSS_SEL_OFFSET`], pointing at the TSS at [`TSS_DESC_OFFSET`], and
/// - RSP0 in that TSS, so the CPU switches to `cpl0_stack_ptr` on the transition.
fn program_privilege_transition_entries(
    gdt: &mut [u8],
    gdt_base: u64,
    return_pointer: u64,
    cpl0_stack_ptr: u64,
) -> MmSupervisorResult<()> {
    let tss_addr = gdt_base.wrapping_add(u64::from(TSS_DESC_OFFSET));

    update_entry::<CallGateDescriptor, _>(gdt, CALL_GATE_OFFSET as usize, |desc| desc.set_return_gate(return_pointer))?;

    update_entry::<TssDescriptor, _>(gdt, TSS_SEL_OFFSET as usize, |desc| desc.set_base(tss_addr))?;

    update_entry::<TaskStateSegment, _>(gdt, TSS_DESC_OFFSET as usize, |tss| tss.set_rsp0(cpl0_stack_ptr))?;

    Ok(())
}

/// Sets up the call gate used to return from a demoted routine.
///
/// Called from the `invoke_demoted_routine` assembly. It reads the running processor's GDTR and
/// CR0, so it is not unit tested on the host; the GDT programming is tested through
/// `program_privilege_transition_entries`.
///
/// ## Safety
///
/// Writes the live GDT and TSS with CR0.WP cleared. Must run in Ring 0.
///
/// ## Panics
///
/// Panics if the GDT image does not contain the call gate, TSS descriptor and TSS.
#[unsafe(no_mangle)]
pub unsafe extern "efiapi" fn setup_call_gate(return_pointer: u64, cpl0_stack_ptr: u64) {
    let gdt_base = get_current_gdt_base();

    // SAFETY: the GDT is mapped read-only. Write protection is cleared only for the update below
    // and restored right after it.
    let cr0 = unsafe { disable_write_protection() };

    // SAFETY: `gdt_base` is the base of the GDT the processor is running on. The platform places
    // the call gate, the TSS descriptor and the TSS (right after the descriptors) within the
    // first `GDT_PROGRAMMED_SIZE` bytes, so this region is allocated and writable even though it
    // reaches past the GDTR limit. Write protection is cleared above, and no other Rust
    // reference to the GDT image exists while this one is alive.
    let gdt = unsafe { core::slice::from_raw_parts_mut(gdt_base as *mut u8, GDT_PROGRAMMED_SIZE) };

    let result = program_privilege_transition_entries(gdt, gdt_base, return_pointer, cpl0_stack_ptr);

    // Restore write protection before acting on a failure so the write-enabled window always
    // closes.
    // SAFETY: `cr0` is the value returned by the paired `disable_write_protection`, so this
    // restores CR0.WP to its prior state. Runs in Ring 0.
    unsafe {
        enable_write_protection(cr0);
    }

    result.expect("GDT contains the call gate, TSS descriptor and TSS");

    log::trace!("Call gate set to 0x{return_pointer:016x}, CPL0 stack pointer set to 0x{cpl0_stack_ptr:016x}");
}
