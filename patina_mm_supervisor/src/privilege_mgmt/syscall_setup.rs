//! Syscall interface setup.
//!
//! Records the CPL3 stack array base, the per-CPU stack size and the CPU count, and returns
//! each CPU's CPL3 stack top for Ring 3 demotion.
//!
//! ## MSR Configuration
//!
//! `invoke_demoted_routine` programs these MSRs before each demotion and restores them on
//! return:
//!
//! - `MSR_IA32_STAR`: segment selectors for syscall/sysret
//!   - Bits 63:48 = sysret CS and SS base (`LONG_CS_R3_PH`)
//!   - Bits 47:32 = syscall CS and SS base (`LONG_CS_R0`)
//!
//! - `MSR_IA32_LSTAR`: 64-bit RIP of the syscall entry (`syscall_center`)
//!
//! - `MSR_IA32_FMASK`: RFLAGS bits the CPU clears on `syscall` entry. `syscall` does no
//!   other flag sanitization, so this is the only thing that stops Ring 3 from choosing the
//!   flags Ring 0 runs under, in particular DF, which reverses compiler-emitted `rep movs`,
//!   and AC, which disables SMAP.
//!
//! - `MSR_IA32_EFER`: bit 0 (SCE) enables syscall/sysret
//!
//! - `MSR_IA32_KERNEL_GS_BASE`: after swapgs, GS points at the slots holding the supervisor
//!   stack pointer and the saved Ring 3 stack pointer for the syscall entry stub.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// Errors specific to syscall setup operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallSetupError {
    /// The syscall interface has not been initialized.
    NotInitialized,
    /// Already initialized.
    AlreadyInitialized,
    /// Invalid CPU index (exceeds configured CPU count).
    InvalidCpuIndex,
    /// The CPU's stack top does not fit in a 64-bit address.
    StackAddressOverflow,
}

impl core::error::Error for SyscallSetupError {}

impl fmt::Display for SyscallSetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized => write!(f, "the syscall interface has not been initialized"),
            Self::AlreadyInitialized => write!(f, "the syscall interface is already initialized"),
            Self::InvalidCpuIndex => write!(f, "the CPU index exceeds the configured CPU count"),
            Self::StackAddressOverflow => write!(f, "the CPL3 stack top does not fit in a 64-bit address"),
        }
    }
}

/// Internal state for the syscall interface.
struct SyscallInterfaceState {
    /// Number of CPUs configured.
    number_of_cores: usize,
    /// CPL3 stack array base address.
    cpl3_stack_base: u64,
    /// Per-CPU stack size.
    stack_size: usize,
}

impl SyscallInterfaceState {
    /// Creates zeroed state; [`SyscallInterface::init`] fills it in.
    const fn new() -> Self {
        Self { number_of_cores: 0, cpl3_stack_base: 0, stack_size: 0 }
    }
}

/// Syscall interface state shared by all CPUs.
///
/// Holds the CPL3 stack layout used for transitions between Ring 0 and Ring 3.
pub struct SyscallInterface {
    /// Whether the interface has been initialized.
    initialized: AtomicBool,
    /// Internal state.
    state: Mutex<SyscallInterfaceState>,
}

impl SyscallInterface {
    /// Creates a new syscall interface.
    pub const fn new() -> Self {
        Self { initialized: AtomicBool::new(false), state: Mutex::new(SyscallInterfaceState::new()) }
    }

    /// Initializes the syscall interface. Call once during BSP initialization, with the core
    /// count that `parse_mp_information_hob` has already validated.
    ///
    /// ## Errors
    ///
    /// Returns `AlreadyInitialized` on a second call. The first configuration is kept.
    pub fn init(
        &self,
        number_of_cores: usize,
        cpl3_stack_base: u64,
        stack_size: usize,
    ) -> Result<(), SyscallSetupError> {
        if self.initialized.swap(true, Ordering::SeqCst) {
            log::error!("Syscall interface init rejected: already initialized");
            return Err(SyscallSetupError::AlreadyInitialized);
        }

        let mut state = self.state.lock();
        state.number_of_cores = number_of_cores;
        state.cpl3_stack_base = cpl3_stack_base;
        state.stack_size = stack_size;

        log::info!(
            "SyscallInterface initialized: {number_of_cores} cores, cpl3_stack=0x{cpl3_stack_base:016x}, stack_size=0x{stack_size:x}"
        );

        Ok(())
    }

    /// Returns whether the syscall interface is initialized.
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Returns the top of the CPL3 stack for `cpu_index` (stacks grow down):
    /// `cpl3_stack_base + stack_size * (cpu_index + 1) - sizeof(usize)`.
    ///
    /// ## Errors
    ///
    /// Returns `NotInitialized` before [`init`](Self::init), `InvalidCpuIndex` for an index past
    /// the core count, and `StackAddressOverflow` if the stack top does not fit in a `u64`.
    ///
    /// TODO: Consider allocating a page before demotion and freeing it on return instead of
    /// using the pre-allocated stack array.
    pub fn get_cpl3_stack(&self, cpu_index: usize) -> Result<u64, SyscallSetupError> {
        if !self.is_initialized() {
            log::error!("SyscallInterface not initialized");
            return Err(SyscallSetupError::NotInitialized);
        }

        let state = self.state.lock();
        if cpu_index >= state.number_of_cores {
            log::error!("Invalid CPU index {}: exceeds configured CPU count {}", cpu_index, state.number_of_cores);
            return Err(SyscallSetupError::InvalidCpuIndex);
        }

        // The base and size come from the PassDown HOB, so the sum is checked instead of trusted.
        (cpu_index as u64)
            .checked_add(1)
            .and_then(|slots| (state.stack_size as u64).checked_mul(slots))
            .and_then(|offset| state.cpl3_stack_base.checked_add(offset))
            .and_then(|end| end.checked_sub(core::mem::size_of::<usize>() as u64))
            .ok_or_else(|| {
                log::error!(
                    "CPL3 stack for CPU {cpu_index} overflows: base=0x{:016x}, stack_size=0x{:x}",
                    state.cpl3_stack_base,
                    state.stack_size
                );
                SyscallSetupError::StackAddressOverflow
            })
    }
}

impl Default for SyscallInterface {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpl3_stack_calculation() {
        let interface: SyscallInterface<8> = SyscallInterface::new();

        // Initialize with known values: num_cpus=4, cpl3_stack_base=0x10000, stack_size=0x4000
        interface.init(4, 0x10000, 0x4000).unwrap();

        // CPU 0: base + 0x4000 * 1 - 8 = 0x10000 + 0x4000 - 8 = 0x13FF8
        assert_eq!(interface.get_cpl3_stack(0).unwrap(), 0x13FF8);

        // CPU 1: base + 0x4000 * 2 - 8 = 0x10000 + 0x8000 - 8 = 0x17FF8
        assert_eq!(interface.get_cpl3_stack(1).unwrap(), 0x17FF8);
    }

    #[test]
    fn test_init_twice_fails() {
        let interface: SyscallInterface<8> = SyscallInterface::new();
        assert!(interface.init(4, 0x10000, 0x4000).is_ok());
        assert_eq!(interface.init(4, 0x10000, 0x4000), Err(SyscallSetupError::AlreadyInitialized));
    }

    #[test]
    fn test_invalid_cpu_index() {
        let interface: SyscallInterface<8> = SyscallInterface::new();
        interface.init(4, 0x10000, 0x4000).unwrap();

        assert_eq!(interface.get_cpl3_stack(4), Err(SyscallSetupError::InvalidCpuIndex));
        assert_eq!(interface.get_cpl3_stack(100), Err(SyscallSetupError::InvalidCpuIndex));
    }

    #[test]
    fn test_max_cpus_exceeded() {
        let interface: SyscallInterface<4> = SyscallInterface::new();
        // Try to init with more CPUs than the const generic allows
        assert_eq!(interface.init(8, 0x10000, 0x4000), Err(SyscallSetupError::InvalidCpuIndex));
    }
}
