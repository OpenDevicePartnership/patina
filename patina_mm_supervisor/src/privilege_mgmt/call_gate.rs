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
use zerocopy::{FromBytes, Immutable, IntoBytes};

use crate::error::MmSupervisorResult;
use crate::intrinsics::with_write_protection_disabled;

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
    let tss_addr = gdt_base + u64::from(TSS_DESC_OFFSET);

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

    let program = || {
        // SAFETY: `gdt_base` is the base of the GDT the processor is running on. The platform
        // places the call gate, the TSS descriptor and the TSS (right after the descriptors)
        // within the first `GDT_PROGRAMMED_SIZE` bytes, so this region is allocated and writable
        // even though it reaches past the GDTR limit. This closure runs with write protection
        // cleared, and no other Rust reference to the GDT image exists while this one is alive.
        let gdt = unsafe { core::slice::from_raw_parts_mut(gdt_base as *mut u8, GDT_PROGRAMMED_SIZE) };
        program_privilege_transition_entries(gdt, gdt_base, return_pointer, cpl0_stack_ptr)
    };
    // SAFETY: the GDT is mapped read-only, and `program` only stores to the GDT entries the
    // transition needs. This runs in Ring 0.
    let result = unsafe { with_write_protection_disabled(program) };

    result.expect("GDT contains the call gate, TSS descriptor and TSS");

    log::trace!("Call gate set to 0x{return_pointer:016x}, CPL0 stack pointer set to 0x{cpl0_stack_ptr:016x}");
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_call_gate_error_names_the_undersized_gdt() {
        // The GDT image comes from outside the supervisor, so the one way programming can fail is
        // an image that does not reach the entries being written.
        assert_eq!(
            format!("{}", CallGateError::GdtTooSmall),
            "the GDT image does not contain the privilege transition entries to program"
        );
    }

    /// A zeroed GDT image large enough for the entries the supervisor programs.
    fn empty_gdt() -> Vec<u8> {
        vec![0u8; GDT_PROGRAMMED_SIZE]
    }

    /// Reads the entry of type `T` at `offset` out of a GDT image.
    fn entry_at<T: FromBytes>(gdt: &[u8], offset: usize) -> T {
        let bytes = gdt.get(offset..offset + core::mem::size_of::<T>()).expect("entry lies within the GDT image");
        T::read_from_bytes(bytes).expect("entry is exactly its byte size")
    }

    /// Writes `entry` into a GDT image at `offset`.
    fn write_entry_at<T: IntoBytes + Immutable>(gdt: &mut [u8], offset: usize, entry: &T) {
        let bytes = gdt.get_mut(offset..offset + core::mem::size_of::<T>()).expect("entry lies within the GDT image");
        entry.write_to(bytes).expect("entry is exactly its byte size");
    }

    fn call_gate_of(gdt: &[u8]) -> CallGateDescriptor {
        entry_at(gdt, CALL_GATE_OFFSET as usize)
    }

    fn tss_descriptor_of(gdt: &[u8]) -> TssDescriptor {
        entry_at(gdt, TSS_SEL_OFFSET as usize)
    }

    fn tss_of(gdt: &[u8]) -> TaskStateSegment {
        entry_at(gdt, TSS_DESC_OFFSET as usize)
    }

    #[test]
    fn test_descriptor_layout_matches_hardware() {
        // The CPU reads these structures directly, so their sizes are part of the contract.
        assert_eq!(core::mem::size_of::<CallGateDescriptor>(), 16);
        assert_eq!(core::mem::size_of::<TssDescriptor>(), 16);
        assert_eq!(core::mem::size_of::<TaskStateSegment>(), 0x68);
        assert_eq!(core::mem::size_of::<GdtRegister>(), 10);
        // The programmed window covers the TSS that follows the descriptors.
        assert_eq!(GDT_PROGRAMMED_SIZE, 0x80 + 0x68);
    }

    #[test]
    fn test_call_gate_offset_round_trips() {
        let mut desc = CallGateDescriptor::default();

        desc.set_offset(0x1234_5678_9ABC_DEF0);
        assert_eq!(desc.offset(), 0x1234_5678_9ABC_DEF0);
        // The 64-bit target is split across three fields of the descriptor.
        let (low, mid, high) = (desc.offset_low, desc.offset_mid, desc.offset_high);
        assert_eq!((low, mid, high), (0xDEF0, 0x9ABC, 0x1234_5678));

        desc.set_offset(0);
        assert_eq!(desc.offset(), 0);
    }

    #[test]
    fn test_tss_descriptor_base_round_trips() {
        let mut desc = TssDescriptor::default();

        desc.set_base(0xFEDC_BA98_7654_3210);
        assert_eq!(desc.base(), 0xFEDC_BA98_7654_3210);
        // The 64-bit base is split across four fields of the descriptor.
        let (low, mid_low, mid_high, high) = (desc.base_low, desc.base_mid_low, desc.base_mid_high, desc.base_high);
        assert_eq!((low, mid_low, mid_high, high), (0x3210, 0x54, 0x76, 0xFEDC_BA98));

        desc.set_base(0);
        assert_eq!(desc.base(), 0);
    }

    #[test]
    fn test_return_gate_is_callable_from_ring_3() {
        let mut desc = CallGateDescriptor::default();

        desc.set_return_gate(0xFFFF_8000_0000_1000);

        assert_eq!(desc.offset(), 0xFFFF_8000_0000_1000);
        // The gate must target the Ring 0 code segment...
        let selector = desc.selector;
        assert_eq!(selector, LONG_CS_R0);
        // ...be present, be a 64-bit call gate (type 0xC) and have DPL 3 so Ring 3 can call it.
        assert_eq!(desc.type_attr, CALL_GATE_TYPE_ATTR);
        assert_eq!(desc.type_attr & 0x80, 0x80, "present bit");
        assert_eq!((desc.type_attr >> 5) & 0x3, 3, "DPL");
        assert_eq!(desc.type_attr & 0xF, 0xC, "64-bit call gate type");
    }

    #[test]
    fn test_program_privilege_transition_entries() {
        const GDT_BASE: u64 = 0x0000_7FFF_1234_0000;
        const RETURN_POINTER: u64 = 0xFFFF_8000_0BAD_F00D;
        const CPL0_STACK: u64 = 0x0000_4000_0000_8000;

        let mut gdt = empty_gdt();
        assert_eq!(program_privilege_transition_entries(&mut gdt, GDT_BASE, RETURN_POINTER, CPL0_STACK), Ok(()));

        let call_gate = call_gate_of(&gdt);
        let selector = call_gate.selector;
        assert_eq!(call_gate.offset(), RETURN_POINTER);
        assert_eq!(selector, LONG_CS_R0);
        assert_eq!(call_gate.type_attr, CALL_GATE_TYPE_ATTR);

        // The TSS descriptor must point at the TSS that follows it in the GDT.
        assert_eq!(tss_descriptor_of(&gdt).base(), GDT_BASE + u64::from(TSS_DESC_OFFSET));

        // RSP0 is what the CPU loads when Ring 3 transitions back into Ring 0.
        assert_eq!(tss_of(&gdt).rsp0(), CPL0_STACK);
    }

    #[test]
    fn test_program_privilege_transition_entries_preserves_untouched_fields() {
        const GDT_BASE: u64 = 0x1000;

        // Start from a GDT whose entries already carry values the supervisor does not program.
        let mut gdt = empty_gdt();

        let call_gate = CallGateDescriptor { ist: 0x3, ..Default::default() };
        write_entry_at(&mut gdt, CALL_GATE_OFFSET as usize, &call_gate);

        let tss_desc = TssDescriptor { limit_low: 0x67, type_attr: 0x89, ..Default::default() };
        write_entry_at(&mut gdt, TSS_SEL_OFFSET as usize, &tss_desc);

        // Fill the TSS with a pattern, so any byte the programming changes is visible.
        let tss_start = TSS_DESC_OFFSET as usize;
        let tss_end = tss_start + core::mem::size_of::<TaskStateSegment>();
        for (index, byte) in gdt[tss_start..tss_end].iter_mut().enumerate() {
            *byte = 0x80 | index as u8;
        }
        let tss_before = gdt[tss_start..tss_end].to_vec();

        assert_eq!(program_privilege_transition_entries(&mut gdt, GDT_BASE, 0x2000, 0x3000), Ok(()));

        // Only the fields the supervisor owns change; everything else survives.
        assert_eq!(call_gate_of(&gdt).ist, 0x3);
        let (limit_low, type_attr) = {
            let desc = tss_descriptor_of(&gdt);
            (desc.limit_low, desc.type_attr)
        };
        assert_eq!(limit_low, 0x67);
        // The type/attribute byte of the TSS descriptor is set by the GDT builder, not here.
        assert_eq!(type_attr, 0x89);

        // RSP0 sits at bytes 4..12 of the TSS. Every other byte keeps its old value.
        assert_eq!(tss_of(&gdt).rsp0(), 0x3000);
        let tss_after = &gdt[tss_start..tss_end];
        assert_eq!(tss_after[..4], tss_before[..4]);
        assert_eq!(tss_after[12..], tss_before[12..]);
    }

    #[test]
    fn test_program_privilege_transition_entries_rejects_short_gdt() {
        // A GDT image that stops before the TSS (or before either descriptor) is reported rather
        // than written past.
        for size in [0, CALL_GATE_OFFSET as usize, TSS_SEL_OFFSET as usize, GDT_PROGRAMMED_SIZE - 1] {
            let mut gdt = vec![0u8; size];
            assert_eq!(
                program_privilege_transition_entries(&mut gdt, 0x1000, 0x2000, 0x3000),
                Err(CallGateError::GdtTooSmall.into()),
                "unexpected result for a {size} byte GDT"
            );
        }
    }

    #[test]
    fn test_update_entry_rejects_an_offset_that_overflows() {
        // The offset arithmetic is guarded so a bogus descriptor offset can never wrap around and
        // end up addressing memory outside the GDT image.
        let mut gdt = empty_gdt();

        assert_eq!(
            update_entry::<CallGateDescriptor, _>(&mut gdt, usize::MAX, |desc| desc.set_offset(0x1000)),
            Err(CallGateError::GdtTooSmall.into())
        );
        assert_eq!(
            update_entry::<TaskStateSegment, _>(&mut gdt, usize::MAX - 4, |tss| tss.set_rsp0(0)),
            Err(CallGateError::GdtTooSmall.into())
        );

        // A rejected offset must leave the GDT untouched.
        assert_eq!(gdt, empty_gdt());
    }

    #[test]
    fn test_program_privilege_transition_entries_is_idempotent() {
        let mut gdt = empty_gdt();

        assert_eq!(program_privilege_transition_entries(&mut gdt, 0x1000, 0x2000, 0x3000), Ok(()));
        let first = gdt.clone();

        assert_eq!(program_privilege_transition_entries(&mut gdt, 0x1000, 0x2000, 0x3000), Ok(()));
        assert_eq!(gdt, first);

        // Re-programming for a different demotion replaces the previous values.
        assert_eq!(program_privilege_transition_entries(&mut gdt, 0x1000, 0x4000, 0x5000), Ok(()));
        assert_eq!(call_gate_of(&gdt).offset(), 0x4000);
        assert_eq!(tss_of(&gdt).rsp0(), 0x5000);
    }
}
