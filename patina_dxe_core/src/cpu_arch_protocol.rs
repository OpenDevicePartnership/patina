#![allow(unused)]
/// Architecture independent public C EFI CPU Architectural Protocol definition.
use crate::{dxe_services, protocols::PROTOCOL_DB};
use alloc::boxed::Box;
use core::ffi::c_void;
use patina_internal_cpu::{
    cpu::Cpu,
    interrupts::{self, ExceptionType, HandlerType, InterruptManager},
};
use patina_sdk::{
    boot_services::{BootServices, StandardBootServices},
    component::service::Service,
    error::{EfiError, Result},
    uefi_protocol::ProtocolInterface,
};
use r_efi::efi;

use mu_pi::protocols::cpu_arch::{CpuFlushType, CpuInitType, InterruptHandler, Protocol, PROTOCOL_GUID};

#[repr(C)]
pub struct EfiCpuArchProtocolImpl {
    protocol: Protocol,

    // Crate accessible fields
    pub(crate) cpu: Service<dyn Cpu>,
    pub(crate) interrupt_manager: Service<dyn InterruptManager>,
}

unsafe impl ProtocolInterface for EfiCpuArchProtocolImpl {
    const PROTOCOL_GUID: efi::Guid = PROTOCOL_GUID;
}

// Helper function to convert a raw mutable pointer to a mutable reference.
fn get_impl_ref<'a>(this: *const Protocol) -> &'a EfiCpuArchProtocolImpl {
    if this.is_null() {
        panic!("Null pointer passed to get_impl_ref()");
    }

    unsafe { &*(this as *const EfiCpuArchProtocolImpl) }
}

fn get_impl_ref_mut<'a>(this: *mut Protocol) -> &'a mut EfiCpuArchProtocolImpl {
    if this.is_null() {
        panic!("Null pointer passed to get_impl_ref_mut()");
    }

    unsafe { &mut *(this as *mut EfiCpuArchProtocolImpl) }
}

// EfiCpuArchProtocolImpl function pointers implementations.

extern "efiapi" fn flush_data_cache(
    this: *const Protocol,
    start: efi::PhysicalAddress,
    length: u64,
    flush_type: CpuFlushType,
) -> efi::Status {
    let cpu = &get_impl_ref(this).cpu;

    let result = cpu.flush_data_cache(start, length, flush_type);

    result.map(|_| efi::Status::SUCCESS).unwrap_or_else(|err| err.into())
}

extern "efiapi" fn enable_interrupt(this: *const Protocol) -> efi::Status {
    interrupts::enable_interrupts();

    efi::Status::SUCCESS
}

extern "efiapi" fn disable_interrupt(this: *const Protocol) -> efi::Status {
    interrupts::disable_interrupts();

    efi::Status::SUCCESS
}

extern "efiapi" fn get_interrupt_state(this: *const Protocol, state: *mut bool) -> efi::Status {
    interrupts::get_interrupt_state()
        .map(|interrupt_state| {
            unsafe {
                *state = interrupt_state;
            }
            efi::Status::SUCCESS
        })
        .unwrap_or_else(|err| err.into())
}

extern "efiapi" fn init(this: *const Protocol, init_type: CpuInitType) -> efi::Status {
    let cpu = &get_impl_ref(this).cpu;

    let result = cpu.init(init_type);

    result.map(|_| efi::Status::SUCCESS).unwrap_or_else(|err| err.into())
}

extern "efiapi" fn register_interrupt_handler(
    this: *const Protocol,
    interrupt_type: isize,
    interrupt_handler: InterruptHandler,
) -> efi::Status {
    let interrupt_manager = &get_impl_ref(this).interrupt_manager;

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

extern "efiapi" fn get_timer_value(
    this: *const Protocol,
    timer_index: u32,
    timer_value: *mut u64,
    timer_period: *mut u64,
) -> efi::Status {
    let cpu = &get_impl_ref(this).cpu;

    let result = cpu.get_timer_value(timer_index);

    match result {
        Ok((value, period)) => {
            unsafe {
                *timer_value = value;
                *timer_period = period;
            }
            efi::Status::SUCCESS
        }
        Err(err) => err.into(),
    }
}

extern "efiapi" fn set_memory_attributes(
    _this: *const Protocol,
    base_address: efi::PhysicalAddress,
    length: u64,
    attributes: u64,
) -> efi::Status {
    match dxe_services::core_set_memory_space_attributes(base_address, length, attributes) {
        Ok(_) => efi::Status::SUCCESS,
        Err(status) => status.into(),
    }
}

impl EfiCpuArchProtocolImpl {
    fn new(cpu: Service<dyn Cpu>, interrupt_manager: Service<dyn InterruptManager>) -> Self {
        Self {
            protocol: Protocol {
                flush_data_cache,
                enable_interrupt,
                disable_interrupt,
                get_interrupt_state,
                init,
                register_interrupt_handler,
                get_timer_value,
                set_memory_attributes,
                number_of_timers: 0,
                dma_buffer_alignment: 0,
            },

            // private data
            cpu,
            interrupt_manager,
        }
    }
}

/// This component installs the cpu arch protocol
pub(crate) fn install_cpu_arch_protocol(
    cpu: Service<dyn Cpu>,
    interrupt_manager: Service<dyn InterruptManager>,
    bs: StandardBootServices,
) -> Result<()> {
    let protocol = EfiCpuArchProtocolImpl::new(cpu, interrupt_manager);

    // Convert the protocol to a raw pointer and store it in to protocol DB
    let interface = Box::leak(Box::new(protocol));

    bs.install_protocol_interface(None, interface)
        .inspect_err(|_| log::error!("Failed to install EFI_CPU_ARCH_PROTOCOL"))?;
    log::info!("installed EFI_CPU_ARCH_PROTOCOL_GUID");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use mockall::{mock, predicate::*};
    use mu_pi::protocols::cpu_arch::{EfiExceptionType, EfiSystemContext};

    mock! {
        EfiCpuInit {}
        impl Cpu for EfiCpuInit {
            fn flush_data_cache(
                &self,
                start: efi::PhysicalAddress,
                length: u64,
                flush_type: CpuFlushType,
            ) -> Result<()>;
            fn init(&self, init_type: CpuInitType) -> Result<()>;
            fn get_timer_value(&self, timer_index: u32) -> Result<(u64, u64)>;
        }
    }

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
            f();
        })
        .unwrap();
    }

    #[test]
    fn test_flush_data_cache() {
        let mut cpu_init = MockEfiCpuInit::new();
        cpu_init.expect_flush_data_cache().with(eq(0), eq(0), always()).returning(|_, _, _| Ok(()));
        let cpu: Service<dyn Cpu> = Service::mock(Box::new(cpu_init));

        let im: Service<dyn InterruptManager> = Service::mock(Box::new(MockInterruptManager::new()));

        let protocol = EfiCpuArchProtocolImpl::new(cpu, im);

        let status = flush_data_cache(&protocol.protocol, 0, 0, CpuFlushType::EfiCpuFlushTypeWriteBackInvalidate);
        assert_eq!(status, efi::Status::SUCCESS);
    }

    #[test]
    fn test_enable_interrupt() {
        let cpu: Service<dyn Cpu> = Service::mock(Box::new(MockEfiCpuInit::new()));
        let im: Service<dyn InterruptManager> = Service::mock(Box::new(MockInterruptManager::new()));
        let protocol = EfiCpuArchProtocolImpl::new(cpu, im);

        let status = enable_interrupt(&protocol.protocol);
        assert_eq!(status, efi::Status::SUCCESS);
    }

    #[test]
    fn test_disable_interrupt() {
        let cpu: Service<dyn Cpu> = Service::mock(Box::new(MockEfiCpuInit::new()));
        let im: Service<dyn InterruptManager> = Service::mock(Box::new(MockInterruptManager::new()));
        let protocol = EfiCpuArchProtocolImpl::new(cpu, im);

        let status = disable_interrupt(&protocol.protocol);
        assert_eq!(status, efi::Status::SUCCESS);
    }

    #[test]
    fn test_get_interrupt_state() {
        let cpu: Service<dyn Cpu> = Service::mock(Box::new(MockEfiCpuInit::new()));
        let im: Service<dyn InterruptManager> = Service::mock(Box::new(MockInterruptManager::new()));
        let protocol = EfiCpuArchProtocolImpl::new(cpu, im);

        let mut state = false;
        let status = get_interrupt_state(&protocol.protocol, &mut state as *mut bool);
        assert_eq!(status, efi::Status::SUCCESS);
    }

    #[test]
    fn test_init() {
        let mut cpu_init = MockEfiCpuInit::new();
        cpu_init.expect_init().with(always()).returning(|_| Ok(()));
        let cpu: Service<dyn Cpu> = Service::mock(Box::new(cpu_init));

        let mut im: Service<dyn InterruptManager> = Service::mock(Box::new(MockInterruptManager::new()));

        let protocol = EfiCpuArchProtocolImpl::new(cpu, im);

        let status = init(&protocol.protocol, CpuInitType::EfiCpuInit);
        assert_eq!(status, efi::Status::SUCCESS);
    }

    extern "efiapi" fn mock_interrupt_handler(_type: EfiExceptionType, _context: EfiSystemContext) {}

    #[test]
    fn test_register_interrupt_handler() {
        let cpu: Service<dyn Cpu> = Service::mock(Box::new(MockEfiCpuInit::new()));

        let mut interrupt_manager = MockInterruptManager::new();
        interrupt_manager
            .expect_register_exception_handler()
            .with(eq(ExceptionType::from(0_usize)), always())
            .returning(|_, _| Ok(()));
        let im: Service<dyn InterruptManager> = Service::mock(Box::new(interrupt_manager));

        let protocol = EfiCpuArchProtocolImpl::new(cpu, im);

        let status = register_interrupt_handler(&protocol.protocol, 0, mock_interrupt_handler);
        assert_eq!(status, efi::Status::SUCCESS);
    }

    #[test]
    fn test_get_timer_value() {
        let mut cpu_init = MockEfiCpuInit::new();
        cpu_init.expect_get_timer_value().with(eq(0)).returning(|_| Ok((0, 0)));
        let cpu: Service<dyn Cpu> = Service::mock(Box::new(cpu_init));

        let im: Service<dyn InterruptManager> = Service::mock(Box::new(MockInterruptManager::new()));

        let protocol = EfiCpuArchProtocolImpl::new(cpu, im);

        let mut timer_value: u64 = 0;
        let mut timer_period: u64 = 0;
        let status = get_timer_value(&protocol.protocol, 0, &mut timer_value as *mut _, &mut timer_period as *mut _);
        assert_eq!(status, efi::Status::SUCCESS);
    }
}
