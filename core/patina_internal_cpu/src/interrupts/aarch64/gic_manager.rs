use arm_gic::gicv3::{GicV3, IntId, Trigger, registers::GICD};
use core::ptr::{addr_of, addr_of_mut, write_volatile};
use patina_sdk::error::EfiError;
use r_efi::efi;

use crate::interrupts::aarch64::sysreg::{read_sysreg, write_sysreg};

// Create basic enum for GIC version
#[derive(PartialEq)]
pub enum GicVersion {
    ArmGicV2 = 2,
    ArmGicV3 = 3,
}

// Determine the current exception level
pub fn get_current_el() -> u64 {
    unsafe { read_sysreg!(CurrentEL) }
}

// Determine the GIC version
fn get_control_system_reg_enable() -> u64 {
    let current_el = get_current_el();
    match current_el {
        0xC => unsafe { read_sysreg!(ICC_SRE_EL3) },
        0x08 => unsafe { read_sysreg!(ICC_SRE_EL2) },
        0x04 => unsafe { read_sysreg!(ICC_SRE_EL1) },
        _ => panic!("Invalid current EL {}", current_el),
    }
}

fn set_control_system_reg_enable(icc_sre: u64) -> u64 {
    let current_el = get_current_el();
    match current_el {
        0x0C => {
            unsafe { write_sysreg!(ICC_SRE_EL3, icc_sre) };
        }
        0x08 => {
            unsafe { write_sysreg!(ICC_SRE_EL2, icc_sre) };
        }
        0x04 => {
            unsafe { write_sysreg!(ICC_SRE_EL1, icc_sre) };
        }
        _ => panic!("Invalid current EL {}", current_el),
    }

    get_control_system_reg_enable()
}

pub fn get_system_gic_version() -> GicVersion {
    let pfr0_el1 = unsafe { read_sysreg!(ID_AA64PFR0_EL1) };

    if (pfr0_el1 & (0xf << 24)) == 0 {
        return GicVersion::ArmGicV2;
    }

    let mut icc_sre = get_control_system_reg_enable();

    if icc_sre & 0x1 == 1 {
        return GicVersion::ArmGicV3;
    }

    icc_sre |= 0x1;
    icc_sre = set_control_system_reg_enable(icc_sre);
    if icc_sre & 0x1 == 1 {
        return GicVersion::ArmGicV3;
    }

    GicVersion::ArmGicV2
}

/// Get the maximum interrupt number supported by the GIC.
///
/// # Safety
///
/// This function reads the GICD_TYPER register, which is set during core
/// initialization and is not expected to change during runtime.
///
pub unsafe fn get_max_interrupt_number(gicd: *mut GICD) -> u32 {
    let max_num = unsafe { addr_of!((*gicd).typer).read_volatile() & 0x1f };

    if max_num == 0x1f { 1020 } else { (max_num + 1) * 32 }
}

pub fn get_mpidr() -> u64 {
    unsafe { read_sysreg!(mpidr_el1) }
}

pub fn set_binary_point_reg(value: u64) {
    unsafe {
        write_sysreg!(ICC_BPR1_EL1, value);
    }
}

/// Initialize the GIC.
///
/// # Safety
///
/// This function writes to the GICD registers, which are expected to be
/// initialized during core initialization and not expected to change during
/// runtime.
///
pub unsafe fn gic_initialize(gicd_base: *mut u64, gicr_base: *mut u64) -> Result<GicV3, EfiError> {
    let gic_v = get_system_gic_version();
    if gic_v == GicVersion::ArmGicV2 {
        debug_assert!(false, "GICv2 is not supported");
        return Err(EfiError::Unsupported);
    }

    // Initialize the GIC, which will include locating the GICD and GICR.
    // Enable affinity routing and non-secure group 1 interrupts.
    // Enable gic cpu interface
    // Enable gic distributor
    let mut gic_v3 = unsafe { GicV3::new(gicd_base, gicr_base) };
    gic_v3.setup();

    // Disable all interrupts and set priority to 0x80.
    let max_int = unsafe { get_max_interrupt_number(gic_v3.gicd_ptr()) };
    for i in 0..max_int {
        if i < 16 {
            gic_v3.enable_interrupt(IntId::sgi(i), false);
            gic_v3.set_interrupt_priority(IntId::sgi(i), 0x80);
        } else if i < 32 {
            gic_v3.enable_interrupt(IntId::ppi(i - 16), false);
            gic_v3.set_interrupt_priority(IntId::ppi(i - 16), 0x80);
        } else {
            gic_v3.enable_interrupt(IntId::spi(i - 32), false);
            gic_v3.set_interrupt_priority(IntId::spi(i - 32), 0x80);
        }
    }

    // Route the SPIs to the primary CPU. SPIs start at the INTID 32
    // MuChange - SPIs per the GICv3 spec start at line 32, but the previous code
    // relied on irouter to be a value different than the spec that
    // skipped those first 32 lines.
    let cpu_target = get_mpidr() & (0xFF0000FFFF);
    for i in 0..(max_int - 32) {
        unsafe {
            let irouter_ptr = addr_of_mut!((*gic_v3.gicd_ptr()).irouter[i as usize]);
            write_volatile(irouter_ptr, cpu_target);
        }
    }

    // Set binary point reg to 0x7 (no preemption)
    set_binary_point_reg(0x7);

    // Set priority mask reg to 0xff to allow all priorities through
    GicV3::set_priority_mask(0xff);

    Ok(gic_v3)
}

pub struct AArch64InterruptInitializer {
    pub gic_v3: GicV3,
}

impl AArch64InterruptInitializer {
    pub fn enable_interrupt_source(&mut self, interrupt_source: u64) -> efi::Status {
        let int_id = if interrupt_source < 16 {
            IntId::sgi(interrupt_source.try_into().unwrap())
        } else if interrupt_source < 32 {
            IntId::ppi((interrupt_source - 16).try_into().unwrap())
        } else {
            IntId::spi((interrupt_source - 32).try_into().unwrap())
        };
        self.gic_v3.enable_interrupt(int_id, true);
        efi::Status::SUCCESS
    }

    pub fn disable_interrupt_source(&mut self, interrupt_source: u64) -> efi::Status {
        let int_id = if interrupt_source < 16 {
            IntId::sgi(interrupt_source.try_into().unwrap())
        } else if interrupt_source < 32 {
            IntId::ppi((interrupt_source - 16).try_into().unwrap())
        } else {
            IntId::spi((interrupt_source - 32).try_into().unwrap())
        };
        self.gic_v3.enable_interrupt(int_id, false);
        efi::Status::SUCCESS
    }

    pub fn get_interrupt_source_state(&mut self, interrupt_source: u64) -> bool {
        let index = (interrupt_source / 32) as usize;
        let bit = 1 << (interrupt_source % 32);

        // SAFETY: We know that `gic_v3.gic_v3.gicd` is a valid and unique pointer to the registers of a
        // GIC distributor interface, and `gic_v3.gic_v3.sgi` to the SGI and PPI registers of a GIC
        // redistributor interface.
        unsafe {
            if interrupt_source < 32 {
                addr_of_mut!((*self.gic_v3.sgi_ptr()).isenabler0).read_volatile() & bit != 0
            } else {
                addr_of_mut!((*self.gic_v3.gicd_ptr()).isenabler[index]).read_volatile() & bit != 0
            }
        }
    }

    pub fn end_of_interrupt(&self, interrupt_source: u64) -> efi::Status {
        let int_id = if interrupt_source < 16 {
            IntId::sgi(interrupt_source.try_into().unwrap())
        } else if interrupt_source < 32 {
            IntId::ppi((interrupt_source - 16).try_into().unwrap())
        } else {
            IntId::spi((interrupt_source - 32).try_into().unwrap())
        };
        GicV3::end_interrupt(int_id);
        efi::Status::SUCCESS
    }

    pub fn get_trigger_type(&mut self, interrupt_source: u64) -> Trigger {
        let index = (interrupt_source / 16) as usize;
        let bit = 1 << (interrupt_source % 16);

        // SAFETY: We know that `gic_v3.gic_v3.gicd` is a valid and unique pointer to the registers of a
        // GIC distributor interface, and `gic_v3.gic_v3.sgi` to the SGI and PPI registers of a GIC
        // redistributor interface.
        let level = unsafe {
            if interrupt_source < 32 {
                addr_of_mut!((*self.gic_v3.sgi_ptr()).icfgr[index]).read_volatile() & bit == 0
            } else {
                addr_of_mut!((*self.gic_v3.gicd_ptr()).icfgr[index]).read_volatile() & bit == 0
            }
        };

        if level { Trigger::Level } else { Trigger::Edge }
    }

    pub fn set_trigger_type(&mut self, interrupt_source: u64, trigger_type: Trigger) -> Result<(), EfiError> {
        let int_id = if interrupt_source < 16 {
            IntId::sgi(interrupt_source.try_into().unwrap())
        } else if interrupt_source < 32 {
            IntId::ppi((interrupt_source - 16).try_into().unwrap())
        } else {
            IntId::spi((interrupt_source - 32).try_into().unwrap())
        };

        self.gic_v3.set_trigger(int_id, trigger_type);

        Ok(())
    }

    pub fn new(gic_v3: GicV3) -> Self {
        AArch64InterruptInitializer { gic_v3 }
    }
}
