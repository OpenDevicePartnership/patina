#![allow(unused)]
/// Architecture independent public C EFI CPU Architectural Protocol definition.
use crate::protocols::PROTOCOL_DB;
use alloc::boxed::Box;
use core::ffi::c_void;
use r_efi::efi;
use uefi_cpu::{
    cpu::EfiCpuInit,
    interrupts::{ExceptionType, HandlerType, InterruptManager},
};
use uefi_sdk::error::EfiError;

use mu_pi::protocols::cpu_arch::{CpuFlushType, CpuInitType, InterruptHandler, Protocol, PROTOCOL_GUID};

#[repr(C)]
pub struct EfiCpuArchProtocolImpl<'a> {
    protocol: Protocol,

    // Crate accessible fields
    pub(crate) cpu_init: &'a mut dyn EfiCpuInit,
    pub(crate) interrupt_manager: &'a mut dyn InterruptManager,
}

// Helper function to convert a raw mutable pointer to a mutable reference.
fn get_impl_ref<'a>(this: *const Protocol) -> &'a EfiCpuArchProtocolImpl<'a> {
    if this.is_null() {
        panic!("Null pointer passed to get_impl_ref()");
    }

    unsafe { &*(this as *const EfiCpuArchProtocolImpl<'a>) }
}

fn get_impl_ref_mut<'a>(this: *mut Protocol) -> &'a mut EfiCpuArchProtocolImpl<'a> {
    if this.is_null() {
        panic!("Null pointer passed to get_impl_ref_mut()");
    }

    unsafe { &mut *(this as *mut EfiCpuArchProtocolImpl<'a>) }
}

// EfiCpuArchProtocolImpl function pointers implementations.

extern "efiapi" fn flush_data_cache(
    this: *const Protocol,
    start: efi::PhysicalAddress,
    length: u64,
    flush_type: CpuFlushType,
) -> efi::Status {
    let cpu_init = &get_impl_ref(this).cpu_init;

    let result = cpu_init.flush_data_cache(start, length, flush_type);

    result.map(|_| efi::Status::SUCCESS).unwrap_or_else(|err| err.into())
}

extern "efiapi" fn enable_interrupt(this: *const Protocol) -> efi::Status {
    let cpu_init = &get_impl_ref(this).cpu_init;
    let result = cpu_init.enable_interrupt();

    result.map(|_| efi::Status::SUCCESS).unwrap_or_else(|err| err.into())
}

extern "efiapi" fn disable_interrupt(this: *const Protocol) -> efi::Status {
    let cpu_init = &get_impl_ref(this).cpu_init;
    let result = cpu_init.disable_interrupt();

    result.map(|_| efi::Status::SUCCESS).unwrap_or_else(|err| err.into())
}

extern "efiapi" fn get_interrupt_state(this: *const Protocol, state: *mut bool) -> efi::Status {
    let cpu_init = &get_impl_ref(this).cpu_init;
    let result = cpu_init.get_interrupt_state();

    result
        .map(|interrupt_state| {
            unsafe {
                *state = interrupt_state;
            }
            efi::Status::SUCCESS
        })
        .unwrap_or_else(|err| err.into())
}

extern "efiapi" fn init(this: *const Protocol, init_type: CpuInitType) -> efi::Status {
    let cpu_init = &get_impl_ref(this).cpu_init;

    let result = cpu_init.init(init_type);

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
    let cpu_init = &get_impl_ref(this).cpu_init;

    let result = cpu_init.get_timer_value(timer_index);

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
    _base_address: efi::PhysicalAddress,
    _length: u64,
    _attributes: u64,
) -> efi::Status {
    let result: Result<bool, efi::Status> = Ok(true);

    // TODO: call in to gcd here.

    result.map(|_| efi::Status::SUCCESS).unwrap_or_else(|err| err)
}

impl<'a> EfiCpuArchProtocolImpl<'a> {
    fn new(cpu_init: &'a mut dyn EfiCpuInit, interrupt_manager: &'a mut dyn InterruptManager) -> Self {
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
            cpu_init,
            interrupt_manager,
        }
    }
}

/// This function is called by the DXE Core to install the protocol.
pub(crate) fn install_cpu_arch_protocol<'a>(
    cpu_init: &'a mut dyn EfiCpuInit,
    interrupt_manager: &'a mut dyn InterruptManager,
) {
    let protocol = EfiCpuArchProtocolImpl::new(cpu_init, interrupt_manager);

    // Convert the protocol to a raw pointer and store it in to protocol DB
    // TODO: This is a "deleberate" memory leak. We need to free this memory
    // when the protocol is uninstalled.
    let interface = Box::into_raw(Box::new(protocol));
    let interface = interface as *mut c_void;

    let _ = PROTOCOL_DB.install_protocol_interface(None, PROTOCOL_GUID, interface);
    log::info!("installed EFI_CPU_ARCH_PROTOCOL_GUID");
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockall::{predicate::*, *};
    use mu_pi::protocols::cpu_arch::EfiSystemContext;
    use r_efi::efi;
    use uefi_cpu::interrupts::InterruptManager;
    use uefi_cpu::paging::EfiCpuPaging;

    // CPU Init Trait Mock
    mock! {
        pub(crate) MockEfiCpuInit {}

        impl EfiCpuInit for MockEfiCpuInit {
            fn initialize(&mut self) -> Result<(), EfiError>;

            fn flush_data_cache(&self, start: efi::PhysicalAddress, length: u64, flush_type: CpuFlushType) -> Result<(), EfiError>;
            fn enable_interrupt(&self) -> Result<(), EfiError>;
            fn disable_interrupt(&self) -> Result<(), EfiError>;
            fn get_interrupt_state(&self) -> Result<bool, EfiError>;
            fn init(&self, init_type: CpuInitType) -> Result<(), EfiError>;
            fn get_timer_value(&self, timer_index: u32) -> Result<(u64, u64), EfiError>;
        }
    }

    mock! {
        pub(crate) MockEfiCpuPaging {}

        impl EfiCpuPaging for MockEfiCpuPaging {
            fn set_memory_attributes(&mut self, base_address: efi::PhysicalAddress, length: u64, attributes: u64) -> Result<(), EfiError>;

            fn map_memory_region(&mut self, base_address: efi::PhysicalAddress, length: u64, attributes: u64) -> Result<(), EfiError>;
            fn unmap_memory_region(&mut self, base_address: efi::PhysicalAddress, length: u64) -> Result<(), EfiError>;
            fn remap_memory_region(&mut self, base_address: efi::PhysicalAddress, length: u64, attributes: u64) -> Result<(), EfiError>;
            fn install_page_table(&self) -> Result<(), EfiError>;
            fn query_memory_region(&self, base_address: efi::PhysicalAddress, length: u64) -> Result<u64, EfiError>;
        }
    }

    // Interrupt Manager Trait Mock
    mock! {
        pub(crate) MockInterruptManager {}

        impl InterruptManager for MockInterruptManager {
            fn initialize(&mut self) -> Result<(), EfiError>;
            fn register_exception_handler(&self, exception_type: ExceptionType, handler: HandlerType) -> Result<(), EfiError>;
            fn unregister_exception_handler(&self, exception_type: ExceptionType) -> Result<(), EfiError>;
        }
    }

    #[test]
    fn test_flush_data_cache() {
        let mut cpu_init = MockMockEfiCpuInit::new();
        cpu_init.expect_flush_data_cache().times(1).returning(|_, _, _| Ok(()));

        let mut interrupt_manager = MockMockInterruptManager::new();
        let mut protocol_impl = EfiCpuArchProtocolImpl::new(&mut cpu_init, &mut interrupt_manager);
        let protocol = &protocol_impl.protocol as *const _;

        let status = unsafe {
            (protocol_impl.protocol.flush_data_cache)(protocol, 0, 0, CpuFlushType::EfiCpuFlushTypeWriteBackInvalidate)
        };
        assert_eq!(status, efi::Status::SUCCESS);
    }

    #[test]
    fn test_enable_interrupt() {
        let mut cpu_init = MockMockEfiCpuInit::new();
        cpu_init.expect_enable_interrupt().times(1).returning(|| Ok(()));

        let mut interrupt_manager = MockMockInterruptManager::new();
        let mut protocol_impl = EfiCpuArchProtocolImpl::new(&mut cpu_init, &mut interrupt_manager);
        let protocol = &protocol_impl.protocol as *const _;

        let status = unsafe { (protocol_impl.protocol.enable_interrupt)(protocol) };
        assert_eq!(status, efi::Status::SUCCESS);
    }

    #[test]
    fn test_disable_interrupt() {
        let mut cpu_init = MockMockEfiCpuInit::new();
        cpu_init.expect_disable_interrupt().times(1).returning(|| Ok(()));

        let mut interrupt_manager = MockMockInterruptManager::new();
        let mut protocol_impl = EfiCpuArchProtocolImpl::new(&mut cpu_init, &mut interrupt_manager);
        let protocol = &protocol_impl.protocol as *const _;

        let status = unsafe { (protocol_impl.protocol.disable_interrupt)(protocol) };
        assert_eq!(status, efi::Status::SUCCESS);
    }
    #[test]
    fn test_get_interrupt_state() {
        let mut cpu_init = MockMockEfiCpuInit::new();
        cpu_init.expect_get_interrupt_state().times(1).returning(|| Ok(true));

        let mut interrupt_manager = MockMockInterruptManager::new();
        let mut protocol_impl = EfiCpuArchProtocolImpl::new(&mut cpu_init, &mut interrupt_manager);
        let protocol = &protocol_impl.protocol as *const _;

        let mut result = false;
        let status = unsafe { (protocol_impl.protocol.get_interrupt_state)(protocol, &mut result as *mut bool) };
        assert_eq!(status, efi::Status::SUCCESS);
        assert!(result);
    }
    #[test]
    fn test_init() {
        let mut cpu_init = MockMockEfiCpuInit::new();
        cpu_init.expect_init().times(1).returning(|_| Ok(()));

        let mut interrupt_manager = MockMockInterruptManager::new();
        let mut protocol_impl = EfiCpuArchProtocolImpl::new(&mut cpu_init, &mut interrupt_manager);
        let protocol = &protocol_impl.protocol as *const _;

        let status = unsafe { (protocol_impl.protocol.init)(protocol, CpuInitType::EfiCpuInit) };
        assert_eq!(status, efi::Status::SUCCESS);
    }
    #[test]
    fn test_register_interrupt_handler() {
        let mut cpu_init = MockMockEfiCpuInit::new();

        let mut interrupt_manager = MockMockInterruptManager::new();
        interrupt_manager.expect_register_exception_handler().times(1).returning(|_, _| Ok(()));
        interrupt_manager.expect_unregister_exception_handler().times(1).returning(|_| Ok(()));

        let mut protocol_impl = EfiCpuArchProtocolImpl::new(&mut cpu_init, &mut interrupt_manager);
        let protocol = &protocol_impl.protocol as *const _;

        extern "efiapi" fn my_interrupt_handler(_interrupt_type: isize, _system_context: EfiSystemContext) {}
        let interrupt_handler: InterruptHandler = my_interrupt_handler;
        let status = unsafe { (protocol_impl.protocol.register_interrupt_handler)(protocol, 0, interrupt_handler) };
        assert_eq!(status, efi::Status::SUCCESS);

        #[allow(clippy::transmute_null_to_fn)]
        let null_fn: InterruptHandler = unsafe { core::mem::transmute(core::ptr::null::<()>()) };
        let status = unsafe { (protocol_impl.protocol.register_interrupt_handler)(protocol, 0, null_fn) };
        assert_eq!(status, efi::Status::SUCCESS);
    }
    #[test]
    fn test_get_timer_value() {
        let mut cpu_init = MockMockEfiCpuInit::new();
        cpu_init.expect_get_timer_value().times(1).returning(|_| Ok((0, 0)));

        let mut interrupt_manager = MockMockInterruptManager::new();
        let mut protocol_impl = EfiCpuArchProtocolImpl::new(&mut cpu_init, &mut interrupt_manager);
        let protocol = &protocol_impl.protocol as *const _;

        let timer_index: u32 = 0;
        let mut timer_value: u64 = 0;
        let mut timer_period: u64 = 0;
        let status = unsafe {
            (protocol_impl.protocol.get_timer_value)(
                protocol,
                timer_index,
                &mut timer_value as *mut _,
                &mut timer_period as *mut _,
            )
        };
        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(timer_value, 0);
        assert_eq!(timer_period, 0);
    }

    // TODO: Following tests will be enabled once the GCD integration is done.
    // #[test]
    // fn test_set_memory_attributes() {
    //     let mut cpu_init = MockMockEfiCpuPaging::new();
    //     cpu_init.expect_set_memory_attributes().times(1).returning(|_, _, _| Ok(()));

    //     let mut interrupt_manager = MockMockInterruptManager::new();
    //     let mut protocol_impl = EfiCpuArchProtocolImpl::new(&mut cpu_init, &mut interrupt_manager);
    //     let protocol = &protocol_impl.protocol as *const _;

    //     let base_address: u64 = 0;
    //     let length: u64 = 0;
    //     let attributes: u64 = 0;
    //     let status = unsafe { (protocol_impl.protocol.set_memory_attributes)(protocol, base_address, length, attributes) };
    //     assert_eq!(status, efi::Status::SUCCESS);
    // }
}
