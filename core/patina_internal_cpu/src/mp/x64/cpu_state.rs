//! CPU state adopted by APs before entering the Rust dispatch loop.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

// This code can not be meaningfully tested from usermode.
#![cfg_attr(coverage, coverage(off))]

use core::cell::UnsafeCell;

use patina_mtrr::{Mtrr, create_mtrr_lib, structs::MtrrSettings};

/// Global BSP MTRR snapshot shared by every AP.
struct MtrrState {
    mtrrs: UnsafeCell<Option<MtrrSettings>>,
}

// SAFETY: The BSP writes the snapshot before initial AP startup or while every
// dispatchable AP is idle under the dispatch lock. APs only read it after work
// publication or during startup.
unsafe impl Sync for MtrrState {}

static MTRR_STATE: MtrrState = MtrrState { mtrrs: UnsafeCell::new(None) };

impl MtrrState {
    fn capture(&self) -> Result<bool, ()> {
        let mtrr = create_mtrr_lib(0);
        if !mtrr.is_supported() {
            return Ok(false);
        }
        let settings = mtrr
            .get_all_mtrrs()
            .inspect_err(|e| log::error!("Failed to read BSP MTRRs for AP synchronization: {e:?}"))
            .map_err(|_| ())?;

        // SAFETY: The caller guarantees no AP can read the slot while it is replaced.
        let slot = unsafe { &mut *self.mtrrs.get() };
        *slot = Some(settings);
        Ok(true)
    }

    fn apply(&self) -> bool {
        let mut mtrr = create_mtrr_lib(0);
        if !mtrr.is_supported() {
            return true;
        }

        // SAFETY: The BSP publishes the snapshot before dispatching this work
        // or starting an AP, and does not replace it until every AP is idle.
        let Some(settings) = (unsafe { &*self.mtrrs.get() }).as_ref() else {
            log::error!("AP MTRR synchronization ran without a prepared snapshot");
            return false;
        };
        mtrr.set_all_mtrrs(settings);
        true
    }
}

/// Captures the BSP's MTRRs before AP startup or while every AP is idle.
pub(super) fn capture() -> Result<bool, ()> {
    MTRR_STATE.capture()
}

/// Applies the current global MTRR snapshot on the calling AP.
pub(super) fn apply() -> bool {
    MTRR_STATE.apply()
}

pub(super) const IA32_EFER: u32 = 0xC000_0080;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CpuControl {
    pub(super) cr0: u64,
    pub(super) cr3: u64,
    pub(super) cr4: u64,
    pub(super) efer: u64,
}

impl CpuControl {
    pub(super) fn capture() -> Self {
        let cr0: u64;
        let cr3: u64;
        let cr4: u64;
        // SAFETY: Reading control registers has no side effects.
        #[cfg(not(test))]
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
        #[cfg(test)]
        {
            cr0 = 0;
            cr3 = 0;
            cr4 = 0;
        }
        // SAFETY: IA32_EFER is available on x86_64 processors and has no read side effects.
        let efer = unsafe { patina::arch::x64::read_msr(IA32_EFER) };
        Self { cr0, cr3, cr4, efer }
    }
}

pub(super) fn cpuid(leaf: u32, subleaf: u32) -> core::arch::x86_64::CpuidResult {
    #[cfg(not(test))]
    return core::arch::x86_64::__cpuid_count(leaf, subleaf);
    #[cfg(test)]
    {
        let _ = (leaf, subleaf);
        core::arch::x86_64::CpuidResult { eax: 0, ebx: 0, ecx: 0, edx: 0 }
    }
}

pub(super) fn flush_tlb() {
    // SAFETY: Writing the active CR3 value back preserves the page tables while
    // invalidating non-global translations on the current processor.
    #[cfg(not(test))]
    unsafe {
        core::arch::asm!(
            "mov {cr3}, cr3",
            "mov cr3, {cr3}",
            cr3 = out(reg) _,
            options(nostack, preserves_flags),
        );
    }
}
