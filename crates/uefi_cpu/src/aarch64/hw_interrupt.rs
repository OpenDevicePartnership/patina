extern crate alloc;

use core::ffi::c_void;
use r_efi::efi;
use uefi_interrupt::{Aarch64InterruptInitializer, HardwareInterruptHandler};

#[repr(C)]
pub enum HardwareInterrupt2TriggerType {
    HardwareInterrupt2TriggerTypeLevelLow = 0,
    HardwareInterrupt2TriggerTypeLevelHigh = 1,
    HardwareInterrupt2TriggerTypeEdgeFalling = 2,
    HardwareInterrupt2TriggerTypeEdgeRising = 3,
}

// { 0x2890B3EA, 0x053D, 0x1643, { 0xAD, 0x0C, 0xD6, 0x48, 0x08, 0xDA, 0x3F, 0xF1 } }
pub const EFI_HARDWARE_INTERRUPT_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0x2890B3EA, 0x053D, 0x1643, 0xAD, 0x0C, &[0xD6, 0x48, 0x08, 0xDA, 0x3F, 0xF1]);

type HardwareInterruptRegister =
    extern "efiapi" fn(*mut EfiHardwareInterruptProtocol, u64, HardwareInterruptHandler) -> efi::Status;
type HardwareInterruptEnable = extern "efiapi" fn(*mut EfiHardwareInterruptProtocol, u64) -> efi::Status;
type HardwareInterruptDisable = extern "efiapi" fn(*mut EfiHardwareInterruptProtocol, u64) -> efi::Status;
type HardwareInterruptGetState = extern "efiapi" fn(*mut EfiHardwareInterruptProtocol, u64, *mut bool) -> efi::Status;
type HardwareInterruptEnd = extern "efiapi" fn(*mut EfiHardwareInterruptProtocol, u64) -> efi::Status;

/// C struct for the Advanced Logger protocol.
#[repr(C)]
pub struct EfiHardwareInterruptProtocol {
    register_interrupt_source: HardwareInterruptRegister,
    enable_interrupt_source: HardwareInterruptEnable,
    disable_interrupt_source: HardwareInterruptDisable,
    get_interrupt_source_state: HardwareInterruptGetState,
    end_of_interrupt: HardwareInterruptEnd,

    // Internal rust access only! Does not exist in C definition.
    aarch64_interrupt: Aarch64InterruptInitializer,
}

impl EfiHardwareInterruptProtocol {
    pub fn new(aarch64_interrupt: Aarch64InterruptInitializer) -> Self {
        Self {
            register_interrupt_source: register_interrupt_source_v1,
            enable_interrupt_source: enable_interrupt_source_v1,
            disable_interrupt_source: disable_interrupt_source_v1,
            get_interrupt_source_state: get_interrupt_source_state_v1,
            end_of_interrupt: end_of_interrupt_v1,
            aarch64_interrupt,
        }
    }
}

/// EFIAPI for V1 protocol.
pub extern "efiapi" fn register_interrupt_source_v1(
    this: *mut EfiHardwareInterruptProtocol,
    interrupt_source: u64,
    handler: HardwareInterruptHandler,
) -> efi::Status {
    let protocol = this as *const c_void;
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    unsafe { &mut *this }.aarch64_interrupt.register_interrupt_source(interrupt_source, handler)
}

pub extern "efiapi" fn enable_interrupt_source_v1(
    this: *mut EfiHardwareInterruptProtocol,
    interrupt_source: u64,
) -> efi::Status {
    let protocol = this as *const c_void;
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    unsafe { &mut *this }.aarch64_interrupt.enable_interrupt_source(interrupt_source)
}

pub extern "efiapi" fn disable_interrupt_source_v1(
    this: *mut EfiHardwareInterruptProtocol,
    interrupt_source: u64,
) -> efi::Status {
    let protocol = this as *const c_void;
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    unsafe { &mut *this }.aarch64_interrupt.disable_interrupt_source(interrupt_source)
}

pub extern "efiapi" fn get_interrupt_source_state_v1(
    this: *mut EfiHardwareInterruptProtocol,
    interrupt_source: u64,
    state: *mut bool,
) -> efi::Status {
    let protocol = this as *const c_void;
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    unsafe { &mut *this }.aarch64_interrupt.get_interrupt_source_state(interrupt_source, state)
}

pub extern "efiapi" fn end_of_interrupt_v1(
    this: *mut EfiHardwareInterruptProtocol,
    interrupt_source: u64,
) -> efi::Status {
    let protocol = this as *const c_void;
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    unsafe { &mut *this }.aarch64_interrupt.end_of_interrupt(interrupt_source)
}

//   { 0x32898322, 0x2d1a, 0x474a, { 0xba, 0xaa, 0xf3, 0xf7, 0xcf, 0x56, 0x94, 0x70 } }
pub const EFI_HARDWARE_INTERRUPT2_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0x32898322, 0x2d1a, 0x474a, 0xba, 0xaa, &[0xf3, 0xf7, 0xcf, 0x56, 0x94, 0x70]);

type HardwareInterruptRegisterV2 =
    extern "efiapi" fn(*mut EfiHardwareInterruptV2Protocol, u64, HardwareInterruptHandler) -> efi::Status;
type HardwareInterruptEnableV2 = extern "efiapi" fn(*mut EfiHardwareInterruptV2Protocol, u64) -> efi::Status;
type HardwareInterruptDisableV2 = extern "efiapi" fn(*mut EfiHardwareInterruptV2Protocol, u64) -> efi::Status;
type HardwareInterruptGetStateV2 =
    extern "efiapi" fn(*mut EfiHardwareInterruptV2Protocol, u64, *mut bool) -> efi::Status;
type HardwareInterruptEndV2 = extern "efiapi" fn(*mut EfiHardwareInterruptV2Protocol, u64) -> efi::Status;

type HardwareInterruptGetTriggerTypeV2 =
    extern "efiapi" fn(*mut EfiHardwareInterruptV2Protocol, u64, *mut HardwareInterrupt2TriggerType) -> efi::Status;
type HardwareInterruptSetTriggerTypeV2 =
    extern "efiapi" fn(*mut EfiHardwareInterruptV2Protocol, u64, HardwareInterrupt2TriggerType) -> efi::Status;

/// C struct for the Advanced Logger protocol.
#[repr(C)]
pub struct EfiHardwareInterruptV2Protocol {
    register_interrupt_source: HardwareInterruptRegisterV2,
    enable_interrupt_source: HardwareInterruptEnableV2,
    disable_interrupt_source: HardwareInterruptDisableV2,
    get_interrupt_source_state: HardwareInterruptGetStateV2,
    end_of_interrupt: HardwareInterruptEndV2,

    get_trigger_type: HardwareInterruptGetTriggerTypeV2,
    set_trigger_type: HardwareInterruptSetTriggerTypeV2,

    // One off for the Aarch64InterruptInitializer
    aarch64_interrupt: Aarch64InterruptInitializer,
}

impl EfiHardwareInterruptV2Protocol {
    pub fn new(aarch64_interrupt: Aarch64InterruptInitializer) -> Self {
        Self {
            register_interrupt_source: register_interrupt_source_v2,
            enable_interrupt_source: enable_interrupt_source_v2,
            disable_interrupt_source: disable_interrupt_source_v2,
            get_interrupt_source_state: get_interrupt_source_state_v2,
            end_of_interrupt: end_of_interrupt_v2,
            get_trigger_type: get_trigger_type_v2,
            set_trigger_type: set_trigger_type_v2,
            aarch64_interrupt,
        }
    }
}

/// EFIAPI for V2 protocol.
pub extern "efiapi" fn register_interrupt_source_v2(
    this: *mut EfiHardwareInterruptV2Protocol,
    interrupt_source: u64,
    handler: HardwareInterruptHandler,
) -> efi::Status {
    let protocol = this as *const c_void;
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    unsafe { &mut *this }.aarch64_interrupt.register_interrupt_source(interrupt_source, handler)
}

pub extern "efiapi" fn enable_interrupt_source_v2(
    this: *mut EfiHardwareInterruptV2Protocol,
    interrupt_source: u64,
) -> efi::Status {
    let protocol = this as *const c_void;
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    unsafe { &mut *this }.aarch64_interrupt.enable_interrupt_source(interrupt_source)
}

pub extern "efiapi" fn disable_interrupt_source_v2(
    this: *mut EfiHardwareInterruptV2Protocol,
    interrupt_source: u64,
) -> efi::Status {
    let protocol = this as *const c_void;
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    unsafe { &mut *this }.aarch64_interrupt.disable_interrupt_source(interrupt_source)
}

pub extern "efiapi" fn get_interrupt_source_state_v2(
    this: *mut EfiHardwareInterruptV2Protocol,
    interrupt_source: u64,
    state: *mut bool,
) -> efi::Status {
    let protocol = this as *const c_void;
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    unsafe { &mut *this }.aarch64_interrupt.get_interrupt_source_state(interrupt_source, state)
}

pub extern "efiapi" fn end_of_interrupt_v2(
    this: *mut EfiHardwareInterruptV2Protocol,
    interrupt_source: u64,
) -> efi::Status {
    let protocol = this as *const c_void;
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    unsafe { &mut *this }.aarch64_interrupt.end_of_interrupt(interrupt_source)
}

pub extern "efiapi" fn get_trigger_type_v2(
    this: *mut EfiHardwareInterruptV2Protocol,
    interrupt_source: u64,
    trigger_type: *mut HardwareInterrupt2TriggerType,
) -> efi::Status {
    let protocol = this as *const c_void;
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let level = unsafe { &mut *this }.aarch64_interrupt.get_trigger_type(interrupt_source);

    // I know this looks odd, but this is how ArmGicV3 in EDK2 does it...
    let t_type = if level {
        HardwareInterrupt2TriggerType::HardwareInterrupt2TriggerTypeEdgeRising
    } else {
        HardwareInterrupt2TriggerType::HardwareInterrupt2TriggerTypeLevelHigh
    };

    unsafe {
        *trigger_type = t_type;
    }

    efi::Status::SUCCESS
}

pub extern "efiapi" fn set_trigger_type_v2(
    this: *mut EfiHardwareInterruptV2Protocol,
    interrupt_source: u64,
    trigger_type: HardwareInterrupt2TriggerType,
) -> efi::Status {
    let protocol = this as *const c_void;
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let level = match trigger_type {
        HardwareInterrupt2TriggerType::HardwareInterrupt2TriggerTypeLevelLow => true,
        HardwareInterrupt2TriggerType::HardwareInterrupt2TriggerTypeLevelHigh => true,
        HardwareInterrupt2TriggerType::HardwareInterrupt2TriggerTypeEdgeFalling => false,
        HardwareInterrupt2TriggerType::HardwareInterrupt2TriggerTypeEdgeRising => false,
    };

    unsafe { &mut *this }.aarch64_interrupt.set_trigger_type(interrupt_source, level);

    efi::Status::SUCCESS
}
