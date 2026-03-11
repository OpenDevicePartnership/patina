//! Call Gate and TSS Management
//!
//! This module manages call gates and Task State Segment (TSS) descriptors
//! for privilege level transitions. Call gates provide an alternative mechanism
//! (besides syscall/sysret) for Ring 3 code to transition back to Ring 0.
//!
//! ## Call Gate Usage
//!
//! 1. When invoking a demoted routine, the supervisor sets up a call gate
//!    pointing to the return address.
//!
//! 2. The demoted routine in Ring 3 can return to Ring 0 by doing a far call
//!    to the call gate selector.
//!
//! 3. The CPU automatically transitions to Ring 0 and jumps to the address
//!    in the call gate descriptor.
//!
//! ## TSS Usage
//!
//! The TSS is used to specify the Ring 0 stack pointer (RSP0) that the CPU
//! will load when transitioning from Ring 3 to Ring 0 via an interrupt or
//! call gate.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina_paging::x64::{disable_write_protection, enable_write_protection};
use zerocopy::{FromBytes, IntoBytes};

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
/// ## Safety
/// This function is safe to call as it only reads the GDTR register.
#[cfg(target_arch = "x86_64")]
pub unsafe fn get_current_gdt_base() -> u64 {
    // Get current GDT base
    let mut gdtr = GdtRegister::default();
    // SAFETY: `sgdt` only stores the GDTR into the provided `gdtr` buffer and
    // has no other side effects.
    unsafe {
        core::arch::asm!(
            "sgdt [{}]",
            in(reg) &mut gdtr,
            options(nostack, preserves_flags)
        );
    }

    gdtr.base
}

/// Sets up the call gate for returning from a demoted routine.
/// This function is called from assembly code (InvokeDemotedRoutine).
///
/// ## Safety
///
/// This modifies the GDT.
#[unsafe(no_mangle)]
#[cfg(target_arch = "x86_64")]
pub unsafe extern "efiapi" fn setup_call_gate(return_pointer: u64, cpl0_stack_ptr: u64) {
    // Get current GDT base
    // SAFETY: Reads the current GDTR register; see `get_current_gdt_base`.
    let gdt_base = unsafe { get_current_gdt_base() };

    let call_gate_addr = gdt_base + CALL_GATE_OFFSET as u64;

    let tss_desc_addr = gdt_base + TSS_SEL_OFFSET as u64;
    let tss_addr = gdt_base + TSS_DESC_OFFSET as u64;

    // SAFETY: This is safe because we are temporarily disabling page protection
    // on the GDT to update the call gate descriptor, which is necessary for the
    // call gate setup. We will restore protections after updating.
    let cr0 = unsafe { disable_write_protection() };

    // Now program the call gate descriptor for the return address
    let call_gate = call_gate_addr as *mut CallGateDescriptor;

    // Update the call gate offset
    // SAFETY: `call_gate` points to the call gate descriptor within the GDT,
    // which is valid and readable while page protection is masked above.
    let call_gate_bytes =
        unsafe { core::ptr::read_volatile(call_gate.cast::<[u8; core::mem::size_of::<CallGateDescriptor>()]>()) };
    let mut desc = CallGateDescriptor::read_from_bytes(&call_gate_bytes)
        .expect("byte buffer is exactly the size of CallGateDescriptor");
    desc.set_offset(return_pointer);
    desc.selector = LONG_CS_R0;
    // Type = 0xC (64-bit call gate), P = 1, DPL = 3 (Ring 3 can call)
    desc.type_attr = 0xEC;
    // SAFETY: `call_gate` points to the call gate descriptor within the GDT,
    // which is valid and writable while page protection is masked above.
    unsafe {
        core::ptr::write_volatile(
            call_gate.cast::<[u8; core::mem::size_of::<CallGateDescriptor>()]>(),
            desc.as_bytes().try_into().expect("descriptor is exactly its byte size"),
        )
    };

    // Then program the TSS descriptor to point to the TSS (which contains the stack pointer for Ring 0)
    let tss_desc = tss_desc_addr as *mut TssDescriptor;
    let tss = tss_addr as *mut TaskStateSegment;

    // Read the raw TSS descriptor bytes, then reinterpret them as a
    // `TssDescriptor` via zerocopy rather than a typed volatile read.
    // SAFETY: `tss_desc` points to the TSS descriptor within the GDT, which is
    // valid and readable while page protection is masked above.
    let tss_desc_bytes =
        unsafe { core::ptr::read_volatile(tss_desc.cast::<[u8; core::mem::size_of::<TssDescriptor>()]>()) };
    let mut desc =
        TssDescriptor::read_from_bytes(&tss_desc_bytes).expect("byte buffer is exactly the size of TssDescriptor");
    desc.set_base(tss_addr);
    // SAFETY: `tss_desc` points to the TSS descriptor within the GDT, which is
    // valid and writable while page protection is masked above.
    unsafe {
        core::ptr::write_volatile(
            tss_desc.cast::<[u8; core::mem::size_of::<TssDescriptor>()]>(),
            desc.as_bytes().try_into().expect("descriptor is exactly its byte size"),
        )
    };

    // Update RSP0 in the TSS
    // SAFETY: `tss` points to the Task State Segment within the GDT region,
    // which is valid and readable while page protection is masked above.
    let tss_bytes = unsafe { core::ptr::read_volatile(tss.cast::<[u8; core::mem::size_of::<TaskStateSegment>()]>()) };
    let mut tss_data =
        TaskStateSegment::read_from_bytes(&tss_bytes).expect("byte buffer is exactly the size of TaskStateSegment");
    tss_data.privilege_stack_table[0] = cpl0_stack_ptr;
    // SAFETY: `tss` points to the Task State Segment within the GDT region,
    // which is valid and writable while page protection is masked above.
    unsafe {
        core::ptr::write_volatile(
            tss.cast::<[u8; core::mem::size_of::<TaskStateSegment>()]>(),
            tss_data.as_bytes().try_into().expect("TSS is exactly its byte size"),
        )
    };

    // Restore GDT read-only protection.
    // SAFETY: `cr0` is the exact value returned by the paired `disable_write_protection` above, so
    // this restores CR0.WP to its prior state, closing the write-enabled window. Runs in Ring 0.
    unsafe {
        enable_write_protection(cr0);
    }

    log::trace!("Call gate set to 0x{:016x}, CPL0 stack pointer set to 0x{:016x}", return_pointer, cpl0_stack_ptr);
}
