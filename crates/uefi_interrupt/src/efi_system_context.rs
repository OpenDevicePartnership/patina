//! CPU System Context for x64 and aarch64
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

///
/// Universal EFI_SYSTEM_CONTEXT_X64 definition.
///
#[repr(C)]
#[derive(Debug)]
pub struct EfiSystemContextX64 {
    pub exception_data: u64,
    pub fx_save_state: EfiFxSaveStateX64,
    pub dr0: u64,
    pub dr1: u64,
    pub dr2: u64,
    pub dr3: u64,
    pub dr6: u64,
    pub dr7: u64,
    pub cr0: u64,
    pub cr1: u64,
    pub cr2: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub cr8: u64,
    pub rflags: u64,
    pub ldtr: u64,
    pub tr: u64,
    pub gdtr: [u64; 2],
    pub idtr: [u64; 2],
    pub rip: u64,
    pub gs: u64,
    pub fs: u64,
    pub es: u64,
    pub ds: u64,
    pub cs: u64,
    pub ss: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

///
/// EFI_FX_SAVE_STATE_X64 definition
///
#[repr(C)]
#[derive(Debug)]
pub struct EfiFxSaveStateX64 {
    pub fcw: u16,
    pub fsw: u16,
    pub ftw: u16,
    pub opcode: u16,
    pub rip: u64,
    pub data_offset: u64,
    pub reserved1: [u8; 8],
    pub st0_mm0: [u8; 10],
    pub reserved2: [u8; 6],
    pub st1_mm1: [u8; 10],
    pub reserved3: [u8; 6],
    pub st2_mm2: [u8; 10],
    pub reserved4: [u8; 6],
    pub st3_mm3: [u8; 10],
    pub reserved5: [u8; 6],
    pub st4_mm4: [u8; 10],
    pub reserved6: [u8; 6],
    pub st5_mm5: [u8; 10],
    pub reserved7: [u8; 6],
    pub st6_mm6: [u8; 10],
    pub reserved8: [u8; 6],
    pub st7_mm7: [u8; 10],
    pub reserved9: [u8; 6],
    pub xmm0: [u8; 16],
    pub xmm1: [u8; 16],
    pub xmm2: [u8; 16],
    pub xmm3: [u8; 16],
    pub xmm4: [u8; 16],
    pub xmm5: [u8; 16],
    pub xmm6: [u8; 16],
    pub xmm7: [u8; 16],
    //
    // NOTE: UEFI 2.0 spec definition as follows.
    //
    pub reserved11: [u8; 14 * 16],
}

///
/// Universal EFI_SYSTEM_CONTEXT_AARCH64 definition.
///
#[repr(C)]
#[derive(Debug)]
pub struct EfiSystemContextAArch64 {
    pub x0: u64,
    pub x1: u64,
    pub x2: u64,
    pub x3: u64,
    pub x4: u64,
    pub x5: u64,
    pub x6: u64,
    pub x7: u64,
    pub x8: u64,
    pub x9: u64,
    pub x10: u64,
    pub x11: u64,
    pub x12: u64,
    pub x13: u64,
    pub x14: u64,
    pub x15: u64,
    pub x16: u64,
    pub x17: u64,
    pub x18: u64,
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,
    pub fp: u64, // x29 - Frame pointer
    pub lr: u64, // x30 - Link Register
    pub sp: u64, // x31 - Stack pointer

    // FP/SIMD Registers
    pub v0: [u64; 2],
    pub v1: [u64; 2],
    pub v2: [u64; 2],
    pub v3: [u64; 2],
    pub v4: [u64; 2],
    pub v5: [u64; 2],
    pub v6: [u64; 2],
    pub v7: [u64; 2],
    pub v8: [u64; 2],
    pub v9: [u64; 2],
    pub v10: [u64; 2],
    pub v11: [u64; 2],
    pub v12: [u64; 2],
    pub v13: [u64; 2],
    pub v14: [u64; 2],
    pub v15: [u64; 2],
    pub v16: [u64; 2],
    pub v17: [u64; 2],
    pub v18: [u64; 2],
    pub v19: [u64; 2],
    pub v20: [u64; 2],
    pub v21: [u64; 2],
    pub v22: [u64; 2],
    pub v23: [u64; 2],
    pub v24: [u64; 2],
    pub v25: [u64; 2],
    pub v26: [u64; 2],
    pub v27: [u64; 2],
    pub v28: [u64; 2],
    pub v29: [u64; 2],
    pub v30: [u64; 2],
    pub v31: [u64; 2],

    pub elr: u64,  // Exception Link Register
    pub spsr: u64, // Saved Processor Status Register
    pub fpsr: u64, // Floating Point Status Register
    pub esr: u64,  // Exception syndrome register
    pub far: u64,  // Fault Address Register
}

#[repr(C)]
pub union EfiSystemContext {
    pub system_context_x64: *mut EfiSystemContextX64,
    pub system_context_aarch64: *mut EfiSystemContextAArch64,
}

impl EfiSystemContext {
    #[cfg(target_arch = "x86_64")]
    pub fn get_arch_context(&self) -> &mut EfiSystemContextX64 {
        unsafe { &mut *(self.system_context_x64) }
    }

    #[cfg(target_arch = "aarch64")]
    pub fn get_arch_context(&self) -> &mut EfiSystemContextAArch64 {
        unsafe { &mut *(self.system_context_aarch64) }
    }
}
