//! DXE Core CPU Architectural Protocol
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![allow(unused)]
/// Architecture independent public C EFI CPU Architectural Protocol definition.
use crate::{dxe_services, protocols::PROTOCOL_DB};
use alloc::boxed::Box;
use core::ffi::c_void;
use patina::standard::efi;
use patina::{
    arch,
    component::{
        Storage, component,
        service::{IntoService, Service},
    },
    error::{EfiError, Result},
    protocol::ProtocolInterface,
    uefi::boot_services::{BootServices, StandardBootServices},
};
use patina_internal_cpu::interrupts::{self, ExceptionType, HandlerType, InterruptManager, Interrupts};

use core::sync::atomic::{AtomicU64, Ordering};

use patina::pi::protocol::cpu_arch::{CpuArchProtocol, CpuFlushType, CpuInitType, InterruptHandler, PROTOCOL_GUID};

/// Cached CPU timer period. A value of zero indicates the period has not yet been queried from the
/// architecture layer.
static TIMER_PERIOD: AtomicU64 = AtomicU64::new(0);

#[derive(IntoService)]
#[service(dyn InterruptManager)]
pub(crate) struct DxeInterruptManager(pub(crate) Interrupts);

impl InterruptManager for DxeInterruptManager {
    fn register_exception_handler(&self, exception_type: ExceptionType, handler: HandlerType) -> Result<()> {
        self.0.register_exception_handler(exception_type, handler)
    }

    fn unregister_exception_handler(&self, exception_type: ExceptionType) -> Result<()> {
        self.0.unregister_exception_handler(exception_type)
    }
}

#[repr(C)]
struct EfiCpuArchProtocolImpl {
    protocol: CpuArchProtocol,

    // Crate accessible fields
    pub(crate) interrupt_manager: Service<dyn InterruptManager>,
}

// SAFETY: EfiCpuArchProtocolImpl provides a valid protocol structure with stable GUID.
unsafe impl ProtocolInterface for EfiCpuArchProtocolImpl {
    const PROTOCOL_GUID: patina::BinaryGuid = PROTOCOL_GUID;
}

// Helper to convert a raw protocol pointer to a reference. Returns `None` when the caller passes a
// null pointer so the caller can determine the appropriate action to take.
fn get_impl_ref<'a>(this: *const CpuArchProtocol) -> Option<&'a EfiCpuArchProtocolImpl> {
    if this.is_null() {
        return None;
    }

    // SAFETY: `this` is non-null and points to an EfiCpuArchProtocolImpl instance installed by
    //         Patina via `Box::leak`, so it is properly aligned and valid for the protocol's
    //         lifetime.
    Some(unsafe { &*(this as *const EfiCpuArchProtocolImpl) })
}

fn get_impl_ref_mut<'a>(this: *mut CpuArchProtocol) -> Option<&'a mut EfiCpuArchProtocolImpl> {
    if this.is_null() {
        return None;
    }

    // SAFETY: `this` is non-null and points to an EfiCpuArchProtocolImpl instance installed by
    //         Patina via `Box::leak`, so it is properly aligned and valid for the protocol's
    //         lifetime.
    Some(unsafe { &mut *(this as *mut EfiCpuArchProtocolImpl) })
}

// EfiCpuArchProtocolImpl function pointers implementations.
#[cfg_attr(coverage, coverage(off))]
extern "efiapi" fn flush_data_cache(
    this: *const CpuArchProtocol,
    start: efi::PhysicalAddress,
    length: u64,
    flush_type: CpuFlushType,
) -> efi::Status {
    if this.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    patina::arch::flush_data_cache(start, length, flush_type)
        .map_or_else(core::convert::Into::into, |()| efi::Status::SUCCESS)
}

extern "efiapi" fn enable_interrupt(this: *const CpuArchProtocol) -> efi::Status {
    arch::enable_interrupts();

    efi::Status::SUCCESS
}

extern "efiapi" fn disable_interrupt(this: *const CpuArchProtocol) -> efi::Status {
    arch::disable_interrupts();

    efi::Status::SUCCESS
}

extern "efiapi" fn get_interrupt_state(this: *const CpuArchProtocol, state: *mut bool) -> efi::Status {
    if state.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }
    // SAFETY: caller must ensure that state is a valid pointer. It is null-checked above.
    unsafe {
        state.write_unaligned(arch::interrupts_enabled());
    }
    efi::Status::SUCCESS
}

extern "efiapi" fn init(this: *const CpuArchProtocol, init_type: CpuInitType) -> efi::Status {
    if this.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    efi::Status::UNSUPPORTED
}

extern "efiapi" fn register_interrupt_handler(
    this: *const CpuArchProtocol,
    interrupt_type: isize,
    interrupt_handler: InterruptHandler,
) -> efi::Status {
    let Some(impl_ref) = get_impl_ref(this) else {
        return efi::Status::INVALID_PARAMETER;
    };
    let interrupt_manager = &impl_ref.interrupt_manager;

    let const_fn_ptr = interrupt_handler as *const ();
    let result = if const_fn_ptr.is_null() {
        interrupt_manager.unregister_exception_handler(interrupt_type as ExceptionType)
    } else {
        interrupt_manager
            .register_exception_handler(interrupt_type as ExceptionType, HandlerType::UefiRoutine(interrupt_handler))
    };

    match result {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => err.into(),
    }
}

/// Returns the current CPU timer counter value along with its period.
///
/// The timer period, in units of 100 nanoseconds per tick, is derived from the counter frequency
/// and cached after it is first computed. The architecture (SDK) layer is only consulted for the
/// frequency while the cache remains unfilled; once populated, the cached value is reused.
fn get_cpu_timer_value(timer_index: u32) -> Result<(u64, u64)> {
    if timer_index != 0 {
        return Err(EfiError::InvalidParameter);
    }

    let value = patina::arch::get_timer_value();

    let cached = TIMER_PERIOD.load(Ordering::Relaxed);
    if cached != 0 {
        // The period is already known; reuse the cached value without querying the arch layer.
        return Ok((value, cached));
    }

    // The timer period is the number of 100 ns units that elapse per counter tick, computed from
    // the counter frequency in Hz (100 ns units per tick = 10^7 / frequency). If the frequency
    // cannot be determined, the period is reported as zero.
    let period = match patina::arch::get_timer_frequency() {
        Some(frequency) => 10_000_000 / frequency.get(),
        None => 0,
    };
    TIMER_PERIOD.store(period, Ordering::Relaxed);
    Ok((value, period))
}

extern "efiapi" fn get_timer_value(
    this: *const CpuArchProtocol,
    timer_index: u32,
    timer_value: *mut u64,
    timer_period: *mut u64,
) -> efi::Status {
    if timer_value.is_null() || timer_period.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }
    if this.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let result = get_cpu_timer_value(timer_index);

    match result {
        Ok((value, period)) => {
            // SAFETY: caller must ensure that timer_value and timer_period are valid pointers. They are null-checked above.
            unsafe {
                timer_value.write_unaligned(value);
                timer_period.write_unaligned(period);
            }
            efi::Status::SUCCESS
        }
        Err(err) => err.into(),
    }
}

extern "efiapi" fn set_memory_attributes(
    _this: *const CpuArchProtocol,
    base_address: efi::PhysicalAddress,
    length: u64,
    attributes: u64,
) -> efi::Status {
    match dxe_services::core_set_memory_space_attributes(base_address, length, attributes) {
        Ok(()) => efi::Status::SUCCESS,
        Err(status) => status.into(),
    }
}

impl EfiCpuArchProtocolImpl {
    fn new(interrupt_manager: Service<dyn InterruptManager>) -> Self {
        Self {
            protocol: CpuArchProtocol {
                flush_data_cache,
                enable_interrupt,
                disable_interrupt,
                get_interrupt_state,
                init,
                register_interrupt_handler,
                get_timer_value,
                set_memory_attributes,
                number_of_timers: 0,
                dma_buffer_alignment: patina::arch::cache_writeback_granule(),
            },

            // private data
            interrupt_manager,
        }
    }
}

/// This component installs the cpu arch protocol
#[derive(Default)]
pub(crate) struct CpuArchProtocolInstaller;

#[component]
impl CpuArchProtocolInstaller {
    fn entry_point(self, interrupt_manager: Service<dyn InterruptManager>, bs: StandardBootServices) -> Result<()> {
        let protocol = EfiCpuArchProtocolImpl::new(interrupt_manager);

        // Convert the protocol to a raw pointer and store it in to protocol DB
        let interface = Box::leak(Box::new(protocol));

        bs.install_protocol_interface(None, interface)
            .inspect_err(|_| log::error!("Failed to install EFI_CPU_ARCH_PROTOCOL"))?;
        log::info!("installed EFI_CPU_ARCH_PROTOCOL_GUID");

        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use crate::test_support;

    use super::*;

    use mockall::{mock, predicate::*};
    use patina::pi::protocol::cpu_arch::{EfiExceptionType, EfiSystemContext};

    mock! {
        InterruptManager {}
        impl InterruptManager for InterruptManager {
            fn register_exception_handler(
                &self,
                interrupt_type: ExceptionType,
                handler: HandlerType,
            ) -> Result<()>;
            fn unregister_exception_handler(&self, interrupt_type: ExceptionType) -> Result<()>;
        }
    }

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        crate::test_support::with_global_lock(|| {
            test_support::init_test_logger();
            f();
        })
        .unwrap();
    }

    #[test]
    fn test_enable_interrupt() {
        with_locked_state(|| {
            let im: Service<dyn InterruptManager> = Service::mock(Box::new(MockInterruptManager::new()));
            let protocol = EfiCpuArchProtocolImpl::new(im);

            let status = enable_interrupt(&raw const protocol.protocol);
            assert_eq!(status, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_disable_interrupt() {
        with_locked_state(|| {
            let im: Service<dyn InterruptManager> = Service::mock(Box::new(MockInterruptManager::new()));
            let protocol = EfiCpuArchProtocolImpl::new(im);

            let status = disable_interrupt(&raw const protocol.protocol);
            assert_eq!(status, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_get_interrupt_state() {
        with_locked_state(|| {
            let im: Service<dyn InterruptManager> = Service::mock(Box::new(MockInterruptManager::new()));
            let protocol = EfiCpuArchProtocolImpl::new(im);

            let mut state = false;
            let status = get_interrupt_state(&raw const protocol.protocol, &raw mut state);
            assert_eq!(status, efi::Status::SUCCESS);
        });
    }

    extern "efiapi" fn mock_interrupt_handler(_type: EfiExceptionType, _context: EfiSystemContext) {}

    #[test]
    fn test_register_interrupt_handler() {
        with_locked_state(|| {
            let mut interrupt_manager = MockInterruptManager::new();
            interrupt_manager
                .expect_register_exception_handler()
                .with(eq(ExceptionType::from(0_usize)), always())
                .returning(|_, _| Ok(()));
            let im: Service<dyn InterruptManager> = Service::mock(Box::new(interrupt_manager));

            let protocol = EfiCpuArchProtocolImpl::new(im);

            let status = register_interrupt_handler(&raw const protocol.protocol, 0, mock_interrupt_handler);
            assert_eq!(status, efi::Status::SUCCESS);

            // Verify the case when `this` is null.
            let status = register_interrupt_handler(core::ptr::null(), 0, mock_interrupt_handler);
            assert_eq!(status, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_get_timer_value() {
        with_locked_state(|| {
            let im: Service<dyn InterruptManager> = Service::mock(Box::new(MockInterruptManager::new()));

            let protocol = EfiCpuArchProtocolImpl::new(im);

            let mut timer_value: u64 = 0;
            let mut timer_period: u64 = 0;
            let status = get_timer_value(&raw const protocol.protocol, 0, &raw mut timer_value, &raw mut timer_period);
            assert_eq!(status, efi::Status::SUCCESS);

            // Verify the case when `this` is null.
            let status = get_timer_value(core::ptr::null(), 0, &raw mut timer_value, &raw mut timer_period);
            assert_eq!(status, efi::Status::INVALID_PARAMETER);

            // Null out-parameters should also be rejected.
            let status = get_timer_value(&raw const protocol.protocol, 0, core::ptr::null_mut(), &raw mut timer_period);
            assert_eq!(status, efi::Status::INVALID_PARAMETER);
            let status = get_timer_value(&raw const protocol.protocol, 0, &raw mut timer_value, core::ptr::null_mut());
            assert_eq!(status, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_get_cpu_timer_value() {
        with_locked_state(|| {
            // Start from a clean cache so the frequency-derived path is exercised first.
            TIMER_PERIOD.store(0, Ordering::Relaxed);

            // A non-zero timer index is rejected.
            assert!(matches!(get_cpu_timer_value(1), Err(EfiError::InvalidParameter)));

            // With no cached period and the host stub arch reporting no frequency, the period is 0
            // and the counter value is the stub's 0.
            assert_eq!(get_cpu_timer_value(0).unwrap(), (0, 0));

            // Once a non-zero period is cached, it is returned as-is without recomputing from the
            // frequency.
            TIMER_PERIOD.store(1234, Ordering::Relaxed);
            assert_eq!(get_cpu_timer_value(0).unwrap(), (0, 1234));

            // Reset the shared cache so the value does not leak into other tests.
            TIMER_PERIOD.store(0, Ordering::Relaxed);
        });
    }

    // Tests for DxeInterruptManager delegation
    #[test]
    fn test_dxe_interrupt_manager_register_then_unregister_delegates() {
        with_locked_state(|| {
            let dxe_interrupt_manager = DxeInterruptManager(Interrupts::default());

            // Register first
            let result = dxe_interrupt_manager.register_exception_handler(
                ExceptionType::from(0_usize),
                HandlerType::UefiRoutine(mock_interrupt_handler),
            );
            assert!(result.is_ok());

            // Then unregister
            let result = dxe_interrupt_manager.unregister_exception_handler(ExceptionType::from(0_usize));
            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_dxe_interrupt_manager_unregister_then_register_delegates() {
        with_locked_state(|| {
            let dxe_interrupt_manager = DxeInterruptManager(Interrupts::default());
            let result = dxe_interrupt_manager.unregister_exception_handler(ExceptionType::from(0_usize));
            // Expecting an error because there is no handler registered yet, but the method should still be callable.
            assert!(result.is_err());

            let result = dxe_interrupt_manager.register_exception_handler(
                ExceptionType::from(0_usize),
                HandlerType::UefiRoutine(mock_interrupt_handler),
            );
            assert!(result.is_ok());

            // Now the unregister should succeed
            let result = dxe_interrupt_manager.unregister_exception_handler(ExceptionType::from(0_usize));
            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_get_impl_ref_null_returns_none() {
        assert!(get_impl_ref(core::ptr::null()).is_none());
    }

    #[test]
    fn test_get_impl_ref_returns_some_for_valid_pointer() {
        with_locked_state(|| {
            let im: Service<dyn InterruptManager> = Service::mock(Box::new(MockInterruptManager::new()));
            let protocol = EfiCpuArchProtocolImpl::new(im);

            let this = &raw const protocol.protocol;
            let impl_ref = get_impl_ref(this).expect("non-null pointer should yield Some");
            assert_eq!(&raw const impl_ref.protocol, this);
        });
    }

    #[test]
    fn test_get_impl_ref_mut_null_returns_none() {
        assert!(get_impl_ref_mut(core::ptr::null_mut()).is_none());
    }

    #[test]
    fn test_get_impl_ref_mut_returns_some_for_valid_pointer() {
        with_locked_state(|| {
            let im: Service<dyn InterruptManager> = Service::mock(Box::new(MockInterruptManager::new()));
            let mut protocol = EfiCpuArchProtocolImpl::new(im);

            let this = &raw mut protocol.protocol;
            let impl_ref = get_impl_ref_mut(this).expect("non-null pointer should yield Some");
            assert_eq!(&raw const impl_ref.protocol, this.cast_const());
        });
    }
}
