//! Call Gate and TSS Management
//!
//! This module manages call gates and Task State Segment (TSS) descriptors
//! for privilege level transitions. Call gates provide an alternative mechanism
//! (besides syscall/sysret) for Ring 3 code to transition back to Ring 0.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina_paging::x64::{disable_write_protection, enable_write_protection};
use zerocopy::{FromBytes, Immutable, IntoBytes};

use crate::error::MmSupervisorResult;

// Firmware-only call-gate transfer assembly; included only for the UEFI target so host builds
// (tests, doctests) can link.
#[cfg(target_os = "uefi")]
core::arch::global_asm!(include_str!("call_gate_transfer.asm"));

/// Long mode Ring 0 code segment selector.
pub const LONG_CS_R0: u16 = 0x38;

/// Call gate descriptor offset in GDT.
pub const CALL_GATE_OFFSET: u16 = 0x60;

/// TSS selector offset in GDT.
pub const TSS_SEL_OFFSET: u16 = 0x70;

/// TSS descriptor offset in GDT.
pub const TSS_DESC_OFFSET: u16 = 0x80;

/// Type and attribute byte of the return call gate: present (P = 1), callable from Ring 3
/// (DPL = 3) and of type 0xC (64-bit call gate).
const CALL_GATE_TYPE_ATTR: u8 = 0xEC;

/// Number of bytes of the GDT image that [`setup_call_gate`] programs: everything up to and
/// including the Task State Segment that follows the descriptors.
///
/// The platform lays the TSS out immediately after the GDT (see `SmiException.nasm`, where the
/// TSS is emitted right after the last descriptor), so this window deliberately extends past the
/// limit stored in the GDTR, which covers only the descriptors themselves.
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

/// 64-bit Call Gate Descriptor.
///
/// A call gate allows privilege level transitions through a far call instruction.
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
    /// Reserved (must be 0) and IST (bits 2:0)
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

/// 64-bit TSS Descriptor (16 bytes in 64-bit mode).
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

/// GDTR (GDT Register) structure.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GdtRegister {
    /// Size of the GDT minus 1
    pub limit: u16,
    /// Linear address of the GDT
    pub base: u64,
}

/// 64-bit Task State Segment.
///
/// In 64-bit mode the TSS holds information that is not directly related to the
/// task-switch mechanism, but is used for stack switching when an interrupt or
/// exception occurs. Layout matches the Intel SDM (total size 0x68 bytes).
#[repr(C, packed(4))]
#[derive(Debug, Clone, Copy, zerocopy_derive::FromBytes, zerocopy_derive::IntoBytes, zerocopy_derive::Immutable)]
pub struct TaskStateSegment {
    reserved_1: u32,
    /// 64-bit canonical RSP values for privilege levels 0-2. Loaded on
    /// privilege escalation from a lower to a higher privilege level.
    pub privilege_stack_table: [u64; 3],
    reserved_2: u64,
    /// 64-bit canonical IST pointers. Loaded when an IDT entry has a non-zero
    /// IST index.
    pub interrupt_stack_table: [u64; 7],
    reserved_3: u64,
    reserved_4: u16,
    /// 16-bit offset to the I/O permission bitmap from the TSS base.
    pub io_map_base: u16,
}

/// Gets the current GDT base address by reading the GDTR register.
///
/// ## Safety
/// This function is safe to call as it only reads the GDTR register.
// Reads the GDTR of the running processor, which cannot be exercised deterministically in a
// host-based unit test.
pub unsafe fn get_current_gdt_base() -> u64 {
    // Get current GDT base
    let mut gdtr = GdtRegister::default();
    // SAFETY: `sgdt` only stores the GDTR into the provided `gdtr` buffer and
    // has no other side effects.
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
/// The entry is read first so that fields the supervisor does not program (for example the call
/// gate IST field) keep whatever the GDT was built with.
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
/// `gdt` is the byte image of the GDT and `gdt_base` is the linear address that image is mapped
/// at, which is needed because the TSS descriptor stores the linear address of the TSS. This
/// programs:
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

    // Program the call gate descriptor for the return address.
    update_entry::<CallGateDescriptor, _>(gdt, CALL_GATE_OFFSET as usize, |desc| desc.set_return_gate(return_pointer))?;

    // Point the TSS descriptor at the TSS, which holds the Ring 0 stack pointer.
    update_entry::<TssDescriptor, _>(gdt, TSS_SEL_OFFSET as usize, |desc| desc.set_base(tss_addr))?;

    // Update RSP0 in the TSS.
    update_entry::<TaskStateSegment, _>(gdt, TSS_DESC_OFFSET as usize, |tss| {
        tss.privilege_stack_table[0] = cpl0_stack_ptr;
    })?;

    Ok(())
}

/// Sets up the call gate for returning from a demoted routine.
/// This function is called from assembly code (`InvokeDemotedRoutine`).
///
/// ## Safety
///
/// This modifies the GDT.
///
/// Reads the GDTR and CR0 of the running processor, which cannot be exercised in a host-based
/// unit test; the programming it performs is covered through
/// `program_privilege_transition_entries`.
#[unsafe(no_mangle)]
pub unsafe extern "efiapi" fn setup_call_gate(return_pointer: u64, cpl0_stack_ptr: u64) {
    // Get current GDT base
    // SAFETY: Reads the current GDTR register; see `get_current_gdt_base`.
    let gdt_base = unsafe { get_current_gdt_base() };

    // SAFETY: This is safe because we are temporarily disabling page protection
    // on the GDT to update the call gate descriptor, which is necessary for the
    // call gate setup. We will restore protections after updating.
    let cr0 = unsafe { disable_write_protection() };

    // SAFETY: `gdt_base` is the base of the GDT the processor is currently running on. The
    // platform emits the call gate, the TSS descriptor and the TSS itself within the first
    // `GDT_PROGRAMMED_SIZE` bytes of that image, with the TSS placed immediately after the
    // descriptors, so this region is allocated and writable even though it reaches past the GDTR
    // limit. Write protection is masked above, and no other Rust reference to the GDT image
    // exists while this one is alive.
    let gdt = unsafe { core::slice::from_raw_parts_mut(gdt_base as *mut u8, GDT_PROGRAMMED_SIZE) };

    let result = program_privilege_transition_entries(gdt, gdt_base, return_pointer, cpl0_stack_ptr);

    // Restore GDT read-only protection before reacting to any failure, so the write-enabled
    // window is always closed.
    // SAFETY: `cr0` is the exact value returned by the paired `disable_write_protection` above, so
    // this restores CR0.WP to its prior state, closing the write-enabled window. Runs in Ring 0.
    unsafe {
        enable_write_protection(cr0);
    }

    result.expect("GDT contains the call gate, TSS descriptor and TSS");

    log::trace!("Call gate set to 0x{return_pointer:016x}, CPL0 stack pointer set to 0x{cpl0_stack_ptr:016x}");
}
