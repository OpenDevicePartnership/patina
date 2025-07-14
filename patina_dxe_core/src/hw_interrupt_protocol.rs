use crate::GicBases;
use crate::tpl_lock::TplMutex;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::ffi::c_void;
use patina_internal_cpu::interrupts::gic_manager::{
    AArch64InterruptInitializer, get_max_interrupt_number, gic_initialize,
};
use patina_internal_cpu::interrupts::{ExceptionContext, InterruptHandler, InterruptManager};
use r_efi::efi;

use arm_gic::gicv3::{GicV3, Trigger};
use patina_sdk::boot_services::{BootServices, StandardBootServices};
use patina_sdk::component::{params::Config, service::Service};
use patina_sdk::error::Result;
use patina_sdk::guid::{HARDWARE_INTERRUPT_PROTOCOL, HARDWARE_INTERRUPT_PROTOCOL_V2};
use patina_sdk::uefi_protocol::ProtocolInterface;

pub type HwInterruptHandler = extern "efiapi" fn(u64, &mut ExceptionContext);

#[repr(C)]
#[non_exhaustive]
pub enum HardwareInterrupt2TriggerType {
    // HardwareInterrupt2TriggerTypeLevelLow = 0, // Not used
    HardwareInterrupt2TriggerTypeLevelHigh = 1,
    // HardwareInterrupt2TriggerTypeEdgeFalling = 2, // Not used
    HardwareInterrupt2TriggerTypeEdgeRising = 3,
}

type HardwareInterruptRegister =
    unsafe extern "efiapi" fn(*mut EfiHardwareInterruptProtocol, u64, HwInterruptHandler) -> efi::Status;
type HardwareInterruptEnable = unsafe extern "efiapi" fn(*mut EfiHardwareInterruptProtocol, u64) -> efi::Status;
type HardwareInterruptDisable = unsafe extern "efiapi" fn(*mut EfiHardwareInterruptProtocol, u64) -> efi::Status;
type HardwareInterruptGetState =
    unsafe extern "efiapi" fn(*mut EfiHardwareInterruptProtocol, u64, *mut bool) -> efi::Status;
type HardwareInterruptEnd = unsafe extern "efiapi" fn(*mut EfiHardwareInterruptProtocol, u64) -> efi::Status;

/// C struct for the Hardware Interrupt protocol.
#[repr(C)]
pub struct EfiHardwareInterruptProtocol<'a> {
    register_interrupt_source: HardwareInterruptRegister,
    enable_interrupt_source: HardwareInterruptEnable,
    disable_interrupt_source: HardwareInterruptDisable,
    get_interrupt_source_state: HardwareInterruptGetState,
    end_of_interrupt: HardwareInterruptEnd,

    // Internal rust access only! Does not exist in C definition.
    hw_interrupt_handler: &'a HwInterruptProtocolHandler,
}

impl<'a> EfiHardwareInterruptProtocol<'a> {
    fn new(hw_interrupt_handler: &'a HwInterruptProtocolHandler) -> Self {
        Self {
            register_interrupt_source: Self::register_interrupt_source,
            enable_interrupt_source: Self::enable_interrupt_source,
            disable_interrupt_source: Self::disable_interrupt_source,
            get_interrupt_source_state: Self::get_interrupt_source_state,
            end_of_interrupt: Self::end_of_interrupt,
            hw_interrupt_handler,
        }
    }

    /// EFIAPI for V1 protocol.
    unsafe extern "efiapi" fn register_interrupt_source(
        this: *mut EfiHardwareInterruptProtocol,
        interrupt_source: u64,
        handler: HwInterruptHandler,
    ) -> efi::Status {
        if this.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        unsafe { &mut *this }.hw_interrupt_handler.register_interrupt_source(interrupt_source as usize, handler)
    }

    unsafe extern "efiapi" fn enable_interrupt_source(
        this: *mut EfiHardwareInterruptProtocol,
        interrupt_source: u64,
    ) -> efi::Status {
        if this.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        unsafe { &mut *this }.hw_interrupt_handler.aarch64_int.lock().enable_interrupt_source(interrupt_source)
    }

    unsafe extern "efiapi" fn disable_interrupt_source(
        this: *mut EfiHardwareInterruptProtocol,
        interrupt_source: u64,
    ) -> efi::Status {
        if this.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        unsafe { &mut *this }.hw_interrupt_handler.aarch64_int.lock().disable_interrupt_source(interrupt_source)
    }

    unsafe extern "efiapi" fn get_interrupt_source_state(
        this: *mut EfiHardwareInterruptProtocol,
        interrupt_source: u64,
        state: *mut bool,
    ) -> efi::Status {
        if this.is_null() || state.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        let enable =
            unsafe { &mut *this }.hw_interrupt_handler.aarch64_int.lock().get_interrupt_source_state(interrupt_source);
        unsafe {
            *state = enable;
        }
        efi::Status::SUCCESS
    }

    unsafe extern "efiapi" fn end_of_interrupt(
        this: *mut EfiHardwareInterruptProtocol,
        interrupt_source: u64,
    ) -> efi::Status {
        if this.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        unsafe { &mut *this }.hw_interrupt_handler.aarch64_int.lock().end_of_interrupt(interrupt_source)
    }
}

unsafe impl ProtocolInterface for EfiHardwareInterruptProtocol<'_> {
    const PROTOCOL_GUID: efi::Guid = HARDWARE_INTERRUPT_PROTOCOL;
}

type HardwareInterruptRegisterV2 =
    unsafe extern "efiapi" fn(*mut EfiHardwareInterruptV2Protocol, u64, HwInterruptHandler) -> efi::Status;
type HardwareInterruptEnableV2 = unsafe extern "efiapi" fn(*mut EfiHardwareInterruptV2Protocol, u64) -> efi::Status;
type HardwareInterruptDisableV2 = unsafe extern "efiapi" fn(*mut EfiHardwareInterruptV2Protocol, u64) -> efi::Status;
type HardwareInterruptGetStateV2 =
    unsafe extern "efiapi" fn(*mut EfiHardwareInterruptV2Protocol, u64, *mut bool) -> efi::Status;
type HardwareInterruptEndV2 = unsafe extern "efiapi" fn(*mut EfiHardwareInterruptV2Protocol, u64) -> efi::Status;

type HardwareInterruptGetTriggerTypeV2 = unsafe extern "efiapi" fn(
    *mut EfiHardwareInterruptV2Protocol,
    u64,
    *mut HardwareInterrupt2TriggerType,
) -> efi::Status;
type HardwareInterruptSetTriggerTypeV2 =
    unsafe extern "efiapi" fn(*mut EfiHardwareInterruptV2Protocol, u64, HardwareInterrupt2TriggerType) -> efi::Status;

/// C struct for the Hardware Interrupt protocol v2.
#[repr(C)]
pub struct EfiHardwareInterruptV2Protocol<'a> {
    register_interrupt_source: HardwareInterruptRegisterV2,
    enable_interrupt_source: HardwareInterruptEnableV2,
    disable_interrupt_source: HardwareInterruptDisableV2,
    get_interrupt_source_state: HardwareInterruptGetStateV2,
    end_of_interrupt: HardwareInterruptEndV2,

    get_trigger_type: HardwareInterruptGetTriggerTypeV2,
    set_trigger_type: HardwareInterruptSetTriggerTypeV2,

    // One off for the HwInterruptProtocolHandler
    hw_interrupt_handler: &'a HwInterruptProtocolHandler,
}

impl<'a> EfiHardwareInterruptV2Protocol<'a> {
    fn new(hw_interrupt_handler: &'a HwInterruptProtocolHandler) -> Self {
        Self {
            register_interrupt_source: Self::register_interrupt_source,
            enable_interrupt_source: Self::enable_interrupt_source,
            disable_interrupt_source: Self::disable_interrupt_source,
            get_interrupt_source_state: Self::get_interrupt_source_state,
            end_of_interrupt: Self::end_of_interrupt,
            get_trigger_type: Self::get_trigger_type,
            set_trigger_type: Self::set_trigger_type,
            hw_interrupt_handler,
        }
    }

    /// EFIAPI for V2 protocol.
    unsafe extern "efiapi" fn register_interrupt_source(
        this: *mut EfiHardwareInterruptV2Protocol,
        interrupt_source: u64,
        handler: HwInterruptHandler,
    ) -> efi::Status {
        if this.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        unsafe { &mut *this }.hw_interrupt_handler.register_interrupt_source(interrupt_source as usize, handler)
    }

    unsafe extern "efiapi" fn enable_interrupt_source(
        this: *mut EfiHardwareInterruptV2Protocol,
        interrupt_source: u64,
    ) -> efi::Status {
        if this.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        unsafe { &mut *this }.hw_interrupt_handler.aarch64_int.lock().enable_interrupt_source(interrupt_source)
    }

    unsafe extern "efiapi" fn disable_interrupt_source(
        this: *mut EfiHardwareInterruptV2Protocol,
        interrupt_source: u64,
    ) -> efi::Status {
        if this.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        unsafe { &mut *this }.hw_interrupt_handler.aarch64_int.lock().disable_interrupt_source(interrupt_source)
    }

    unsafe extern "efiapi" fn get_interrupt_source_state(
        this: *mut EfiHardwareInterruptV2Protocol,
        interrupt_source: u64,
        state: *mut bool,
    ) -> efi::Status {
        if this.is_null() || state.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        let enable =
            unsafe { &mut *this }.hw_interrupt_handler.aarch64_int.lock().get_interrupt_source_state(interrupt_source);
        unsafe {
            *state = enable;
        }
        efi::Status::SUCCESS
    }

    unsafe extern "efiapi" fn end_of_interrupt(
        this: *mut EfiHardwareInterruptV2Protocol,
        interrupt_source: u64,
    ) -> efi::Status {
        if this.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        unsafe { &mut *this }.hw_interrupt_handler.aarch64_int.lock().end_of_interrupt(interrupt_source)
    }

    unsafe extern "efiapi" fn get_trigger_type(
        this: *mut EfiHardwareInterruptV2Protocol,
        interrupt_source: u64,
        trigger_type: *mut HardwareInterrupt2TriggerType,
    ) -> efi::Status {
        if this.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        let level = unsafe { &mut *this }.hw_interrupt_handler.aarch64_int.lock().get_trigger_type(interrupt_source);

        // I know this looks odd, but this is how ArmGicV3 in EDK2 does it...
        let t_type = level.into();

        unsafe {
            *trigger_type = t_type;
        }

        efi::Status::SUCCESS
    }

    unsafe extern "efiapi" fn set_trigger_type(
        this: *mut EfiHardwareInterruptV2Protocol,
        interrupt_source: u64,
        trigger_type: HardwareInterrupt2TriggerType,
    ) -> efi::Status {
        if this.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        let level = trigger_type.into();

        let result =
            unsafe { &mut *this }.hw_interrupt_handler.aarch64_int.lock().set_trigger_type(interrupt_source, level);

        match result {
            Ok(()) => efi::Status::SUCCESS,
            Err(err) => err.into(),
        }
    }
}

unsafe impl ProtocolInterface for EfiHardwareInterruptV2Protocol<'_> {
    const PROTOCOL_GUID: efi::Guid = HARDWARE_INTERRUPT_PROTOCOL_V2;
}

impl From<Trigger> for HardwareInterrupt2TriggerType {
    fn from(a: Trigger) -> HardwareInterrupt2TriggerType {
        // convert A to B
        match a {
            Trigger::Level => HardwareInterrupt2TriggerType::HardwareInterrupt2TriggerTypeLevelHigh,
            Trigger::Edge => HardwareInterrupt2TriggerType::HardwareInterrupt2TriggerTypeEdgeRising,
        }
    }
}

impl From<HardwareInterrupt2TriggerType> for Trigger {
    fn from(a: HardwareInterrupt2TriggerType) -> Trigger {
        // convert A to B
        match a {
            HardwareInterrupt2TriggerType::HardwareInterrupt2TriggerTypeLevelHigh => Trigger::Level,
            HardwareInterrupt2TriggerType::HardwareInterrupt2TriggerTypeEdgeRising => Trigger::Edge,
        }
    }
}

struct HwInterruptProtocolHandler {
    handlers: TplMutex<Vec<Option<HwInterruptHandler>>>,
    aarch64_int: TplMutex<AArch64InterruptInitializer>,
}

impl InterruptHandler for HwInterruptProtocolHandler {
    fn handle_interrupt(&'static self, exception_type: usize, context: &mut ExceptionContext) {
        let int_id = GicV3::get_and_acknowledge_interrupt();
        if int_id.is_none() {
            // The special interrupt do not need to be acknowledged
            return;
        }

        let int_id = int_id.unwrap();
        let raw_value: u32 = int_id.into();

        if raw_value >= self.handlers.lock().len() as u32 {
            match raw_value {
                1021 | 1022 | 1023 => {
                    // The special interrupt do not need to be acknowledged
                }
                _ => {
                    log::error!("Invalid interrupt source: 0x{:x}", raw_value);
                }
            }
            return;
        }

        if let Some(handler) = self.handlers.lock()[raw_value as usize] {
            handler(raw_value as u64, context);
        } else {
            GicV3::end_interrupt(int_id);
            log::error!("Unhandled Exception! 0x{:x}", exception_type);
            log::error!("Exception Context: {:#x?}", context);
            panic! {"Unhandled Exception! 0x{:x}", exception_type};
        }
    }
}

impl HwInterruptProtocolHandler {
    pub fn new(handlers: Vec<Option<HwInterruptHandler>>, aarch64_int: AArch64InterruptInitializer) -> Self {
        Self {
            handlers: TplMutex::new(efi::TPL_HIGH_LEVEL, handlers, "Hardware Interrupt Lock"),
            aarch64_int: TplMutex::new(efi::TPL_HIGH_LEVEL, aarch64_int, "AArch64 GIC Lock"),
        }
    }

    /// Internal implementation of interrupt related functions.
    pub fn register_interrupt_source(&self, interrupt_source: usize, handler: HwInterruptHandler) -> efi::Status {
        if interrupt_source >= self.handlers.lock().len() {
            return efi::Status::INVALID_PARAMETER;
        }

        let m_handler = handler as *const c_void;

        // If the handler is a null pointer, return invalid parameter
        if m_handler.is_null() & self.handlers.lock()[interrupt_source].is_none() {
            return efi::Status::INVALID_PARAMETER;
        }

        if !m_handler.is_null() & self.handlers.lock()[interrupt_source].is_some() {
            return efi::Status::ALREADY_STARTED;
        }

        // If the interrupt handler is unregistered then disable the interrupt
        if m_handler.is_null() {
            self.handlers.lock()[interrupt_source as usize] = None;
            return self.aarch64_int.lock().disable_interrupt_source(interrupt_source as u64);
        } else {
            self.handlers.lock()[interrupt_source as usize] = Some(handler);
            return self.aarch64_int.lock().enable_interrupt_source(interrupt_source as u64);
        }
    }
}

/// This component installs the two hardware interrupt protocols.
pub(crate) fn install_hw_interrupt_protocol(
    interrupt_manager: Service<dyn InterruptManager>,
    gic_bases: Config<GicBases>,
    boot_services: StandardBootServices,
) -> Result<()> {
    let mut gic_v3 = unsafe {
        gic_initialize(gic_bases.0 as _, gic_bases.1 as _).inspect_err(|_| log::error!("Failed to initialize GICv3"))?
    };
    log::info!("GICv3 initialized");

    let max_int = unsafe { get_max_interrupt_number(gic_v3.gicd_ptr()) as usize };
    let handlers = vec![None; max_int];
    let aarch64_int = AArch64InterruptInitializer::new(gic_v3);

    // Prepare context for the v1 interrupt handler
    let hw_int_protocol_handler = Box::leak(Box::new(HwInterruptProtocolHandler::new(handlers, aarch64_int)));
    // Produce Interrupt Protocol with the initialized GIC
    let interrupt_protocol = Box::leak(Box::new(EfiHardwareInterruptProtocol::new(hw_int_protocol_handler)));

    boot_services
        .install_protocol_interface(None, interrupt_protocol)
        .inspect_err(|_| log::error!("Failed to install HARDWARE_INTERRUPT_PROTOCOL"))?;

    // Produce Interrupt Protocol with the initialized GIC
    let interrupt_protocol_v2 = Box::leak(Box::new(EfiHardwareInterruptV2Protocol::new(hw_int_protocol_handler)));

    boot_services
        .install_protocol_interface(None, interrupt_protocol_v2)
        .inspect_err(|_| log::error!("Failed to install HARDWARE_INTERRUPT_PROTOCOL_V2"))?;
    log::info!("installed HARDWARE_INTERRUPT_PROTOCOL_V2");

    // Register the interrupt handlers for IRQs after CPU arch protocol is installed
    interrupt_manager
        .register_exception_handler(1, patina_internal_cpu::interrupts::HandlerType::Handler(hw_int_protocol_handler))
        .inspect_err(|_| log::error!("Failed to register exception handler for hardware interrupts"))?;

    Ok(())
}
