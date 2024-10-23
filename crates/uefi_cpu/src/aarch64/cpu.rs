extern crate alloc;

use core::cell::RefCell;

use alloc::boxed::Box;
use alloc::rc::Rc;
use crate::CpuInitializer;

use uefi_interrupt::{InterruptManager, InterruptManagerAarch64};
use uefi_interrupt::Aarch64InterruptInitializer;
use crate::aarch64::hw_interrupt::{EfiHardwareInterruptProtocol, EfiHardwareInterruptV2Protocol};

pub struct AArch64CpuInitializer {
    interrupt_manager: Option<InterruptManagerAarch64>,
}
impl Default for AArch64CpuInitializer {
    fn default() -> Self {
        AArch64CpuInitializer {
            interrupt_manager: None,
        }
    }
}
impl CpuInitializer for AArch64CpuInitializer {
    fn initialize(&mut self) {
        // Initialize the GIC, so that we are ready to get exception handlers up and running
        let mut interrupt_manager = InterruptManagerAarch64::new();

        // Initialize the exception handlers
        interrupt_manager.initialize();

        self.interrupt_manager = Some(interrupt_manager);
    }

    // need to intake a boot service from r_efi::efi::BootServices
    fn post_init(&mut self, boot_services: *mut r_efi::efi::BootServices) {
        let mut gic_wrapper = Rc::new(RefCell::new(self.interrupt_manager.take().unwrap().gic_wrapper.unwrap()));
        let mut handlers = Rc::new(RefCell::new(vec![None; gic_wrapper.borrow().max_int as usize]));
        let mut aarch64_int = Aarch64InterruptInitializer::new(gic_wrapper.clone(), handlers.clone());

        // Produce Interrupt Protocol with the initialized GIC
        let interrupt_protocol =
            Box::into_raw(Box::new(EfiHardwareInterruptProtocol::new(aarch64_int)));

        let mut handle: r_efi::efi::Handle = core::ptr::null_mut();
        let status = unsafe {
            ((*boot_services).install_protocol_interface)(
                core::ptr::addr_of_mut!(handle),
                &mut crate::aarch64::hw_interrupt::EFI_HARDWARE_INTERRUPT_PROTOCOL_GUID,
                r_efi::system::NATIVE_INTERFACE,
                interrupt_protocol as *mut _,
            )
        };

        match status {
            r_efi::efi::Status::SUCCESS => {
                // Do nothing
            },
            _ => {
                panic!("Failed to install protocol interface {:?}", status);
            }
        }

        let mut aarch64_int = Aarch64InterruptInitializer::new(gic_wrapper.clone(), handlers.clone());

        // Produce Interrupt Protocol with the initialized GIC
        let interrupt_protocol_v2 =
            Box::into_raw(Box::new(EfiHardwareInterruptV2Protocol::new(aarch64_int)));

        let mut handle2: r_efi::efi::Handle = core::ptr::null_mut();
        let status = unsafe {
            ((*boot_services).install_protocol_interface)(
                core::ptr::addr_of_mut!(handle2),
                &mut crate::aarch64::hw_interrupt::EFI_HARDWARE_INTERRUPT2_PROTOCOL_GUID,
                r_efi::system::NATIVE_INTERFACE,
                interrupt_protocol_v2 as *mut _,
            )
        };

        match status {
            r_efi::efi::Status::SUCCESS => {
                // Do nothing
            },
            _ => {
                panic!("Failed to install protocol interface: {:?}", status);
            }
        }

        // Register the interrupt handlers for IRQs after CPU arch protocol is installed
        // gic_wrapper.borrow_mut().gic_v3.register_irq_handlers(handlers.clone());
    }
}

# [cfg (test)]
fn simple_test () {
    let mut aarch64_int = AArch64CpuInitializer::default();
}
