//! Syscall Interface Setup
//!
//! This module handles the initialization and configuration of syscall/sysret MSRs
//! for privilege level transitions. It manages per-CPU storage for MSR values and
//! the syscall cache structure used during ring transitions.
//!
//! ## MSR Configuration
//!
//! - **MSR_IA32_STAR**: Contains segment selectors for syscall/sysret
//!   - Bits 47:32 = SYSRET CS and SS (LONG_CS_R3_PH << 16)
//!   - Bits 31:16 = SYSCALL CS and SS (LONG_CS_R0)
//!
//! - **MSR_IA32_LSTAR**: Contains the 64-bit RIP for syscall entry (SyscallCenter)
//!
//! - **MSR_IA32_EFER**: Extended Feature Enable Register
//!   - Bit 0 (SCE) must be set to enable syscall/sysret
//!
//! - **MSR_IA32_KERNEL_GS_BASE**: Used with swapgs to switch between user and kernel
//!   GS base addresses, allowing access to per-CPU data in the syscall handler.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
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
}

/// Internal state for the syscall interface.
///
/// ## Const Generic Parameters
///
/// * `MAX_CPUS` - The maximum number of CPUs that can be supported.
struct SyscallInterfaceState<const MAX_CPUS: usize> {
    /// Number of CPUs configured.
    num_cpus: usize,
    /// CPL3 stack array base address.
    cpl3_stack_base: u64,
    /// Per-CPU stack size.
    stack_size: usize,
}

impl<const MAX_CPUS: usize> SyscallInterfaceState<MAX_CPUS> {
    const fn new() -> Self {
        Self { num_cpus: 0, cpl3_stack_base: 0, stack_size: 0 }
    }
}

/// Syscall interface manager.
///
/// Manages the syscall/sysret MSR configuration for all CPUs and provides
/// the infrastructure for Ring 0 ↔ Ring 3 transitions.
///
/// ## Const Generic Parameters
///
/// * `MAX_CPUS` - The maximum number of CPUs that can be supported.
///   This should match the `MAX_CPUS` const generic of `MmSupervisorCore`.
pub struct SyscallInterface<const MAX_CPUS: usize> {
    /// Whether the interface has been initialized.
    initialized: AtomicBool,
    /// Internal state protected by mutex.
    state: Mutex<SyscallInterfaceState<MAX_CPUS>>,
}

impl<const MAX_CPUS: usize> SyscallInterface<MAX_CPUS> {
    /// Creates a new syscall interface.
    pub const fn new() -> Self {
        Self { initialized: AtomicBool::new(false), state: Mutex::new(SyscallInterfaceState::new()) }
    }

    /// Initializes the syscall interface.
    ///
    /// This should be called once during BSP initialization. `num_cpus` must be less than or
    /// equal to `MAX_CPUS`.
    pub fn init(&self, num_cpus: usize, cpl3_stack_base: u64, stack_size: usize) -> Result<(), SyscallSetupError> {
        // Check if already initialized
        if self.initialized.swap(true, Ordering::SeqCst) {
            return Err(SyscallSetupError::AlreadyInitialized);
        }

        if num_cpus == 0 || num_cpus > MAX_CPUS {
            self.initialized.store(false, Ordering::SeqCst);
            return Err(SyscallSetupError::InvalidCpuIndex);
        }

        let mut state = self.state.lock();
        state.num_cpus = num_cpus;
        state.cpl3_stack_base = cpl3_stack_base;
        state.stack_size = stack_size;

        log::info!(
            "SyscallInterface<{}> initialized: {} CPUs, cpl3_stack=0x{:016x}, stack_size=0x{:x}",
            MAX_CPUS,
            num_cpus,
            cpl3_stack_base,
            stack_size
        );

        Ok(())
    }

    /// Checks if the syscall interface is initialized.
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Gets the CPL3 stack pointer for a specific CPU.
    ///
    /// The stack pointer is calculated as:
    /// `cpl3_stack_base + stack_size * (cpu_index + 1) - sizeof(usize)`
    ///
    /// This gives the top of the stack for the CPU (stacks grow downward).
    /// TODO: Might just want to allocate a page on the fly before demotion and free the pointer
    /// upon returning instead of messing with the pre-allocated stack array.
    pub fn get_cpl3_stack(&self, cpu_index: usize) -> Result<u64, SyscallSetupError> {
        if !self.is_initialized() {
            log::error!("SyscallInterface not initialized");
            return Err(SyscallSetupError::NotInitialized);
        }

        let state = self.state.lock();
        if cpu_index >= state.num_cpus {
            log::error!("Invalid CPU index {}: exceeds configured CPU count {}", cpu_index, state.num_cpus);
            return Err(SyscallSetupError::InvalidCpuIndex);
        }

        // Calculate stack top: base + size * (index + 1) - sizeof(usize)
        let stack_top = state
            .cpl3_stack_base
            .wrapping_add((state.stack_size as u64) * ((cpu_index as u64) + 1))
            .wrapping_sub(core::mem::size_of::<usize>() as u64);

        Ok(stack_top)
    }
}

impl<const MAX_CPUS: usize> Default for SyscallInterface<MAX_CPUS> {
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
