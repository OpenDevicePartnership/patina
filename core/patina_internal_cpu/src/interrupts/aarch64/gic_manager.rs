use arm_gic::{
    IntId, Trigger,
    gicv3::{GicV3, InterruptGroup},
};
use patina_sdk::error::EfiError;
use r_efi::efi;
use safe_mmio::field;

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
pub unsafe fn gic_initialize<'a>(gicd_base: *mut u64, gicr_base: *mut u64) -> Result<GicV3<'a>, EfiError> {
    let gic_v = get_system_gic_version();
    if gic_v == GicVersion::ArmGicV2 {
        debug_assert!(false, "GICv2 is not supported");
        return Err(EfiError::Unsupported);
    }

    // Initialize the GIC, which will include locating the GICD and GICR.
    // Enable affinity routing and non-secure group 1 interrupts.
    // Enable gic cpu interface
    // Enable gic distributor
    let mut gic_v3 = unsafe { GicV3::new(gicd_base as _, gicr_base as _, 1, false) };
    gic_v3.setup(0);

    // Disable all interrupts and set priority to 0x80.
    gic_v3.enable_all_interrupts(false);
    for i in IntId::private() {
        gic_v3.set_interrupt_priority(i, Some(0), 0x80);
    }
    for spi in 0..gic_v3.typer().num_spis() {
        gic_v3.set_interrupt_priority(IntId::spi(spi), None, 0x80);
    }
    // Set binary point reg to 0x7 (no preemption)
    set_binary_point_reg(0x7);
    // Set priority mask reg to 0xff to allow all priorities through
    GicV3::set_priority_mask(0xff);
    Ok(gic_v3)
}

pub struct AArch64InterruptInitializer<'a> {
    pub gic_v3: GicV3<'a>,
}

impl AArch64InterruptInitializer<'_> {
    fn source_to_intid(interrupt_source: u64) -> Result<IntId, efi::Status> {
        let int_id: u32 = interrupt_source.try_into().map_err(|_| efi::Status::INVALID_PARAMETER)?;
        Ok(match int_id {
            x if x < IntId::SGI_COUNT => IntId::sgi(x),
            x if x < IntId::SGI_COUNT + IntId::PPI_COUNT => IntId::ppi(x - IntId::SGI_COUNT),
            x => IntId::spi(x - IntId::SGI_COUNT + IntId::PPI_COUNT),
        })
    }

    pub fn enable_interrupt_source(&mut self, interrupt_source: u64) -> efi::Status {
        let int_id = if let Ok(int_id) = Self::source_to_intid(interrupt_source) {
            int_id
        } else {
            return efi::Status::INVALID_PARAMETER;
        };

        self.gic_v3.enable_interrupt(int_id, Some(0), true);

        efi::Status::SUCCESS
    }

    pub fn disable_interrupt_source(&mut self, interrupt_source: u64) -> efi::Status {
        let int_id = if let Ok(int_id) = Self::source_to_intid(interrupt_source) {
            int_id
        } else {
            return efi::Status::INVALID_PARAMETER;
        };
        self.gic_v3.enable_interrupt(int_id, Some(0), false);
        efi::Status::SUCCESS
    }

    pub fn get_interrupt_source_state(&mut self, interrupt_source: u64) -> bool {
        let index = (interrupt_source / 32) as usize;
        let bit = 1 << (interrupt_source % 32);

        if interrupt_source < 32 {
            let mut sgi = self.gic_v3.sgi_ptr(0);
            field!(sgi, isenabler0).read() & bit != 0
        } else {
            let mut gicd = self.gic_v3.gicd_ptr();
            field!(gicd, isenabler).get(index).unwrap().read() & bit != 0
        }
    }

    pub fn end_of_interrupt(&self, interrupt_source: u64) -> efi::Status {
        let int_id = if let Ok(int_id) = Self::source_to_intid(interrupt_source) {
            int_id
        } else {
            return efi::Status::INVALID_PARAMETER;
        };
        GicV3::end_interrupt(int_id, InterruptGroup::Group1);
        efi::Status::SUCCESS
    }

    pub fn get_trigger_type(&mut self, interrupt_source: u64) -> Trigger {
        let index = (interrupt_source / 16) as usize;
        let bit = 1 << (interrupt_source % 16);

        let level = if interrupt_source < 32 {
            let mut sgi = self.gic_v3.sgi_ptr(0);
            field!(sgi, icfgr).get(index).unwrap().read() & bit != 0
        } else {
            let mut gicd = self.gic_v3.gicd_ptr();
            field!(gicd, icfgr).get(index).unwrap().read() & bit != 0
        };

        if level { Trigger::Level } else { Trigger::Edge }
    }

    pub fn set_trigger_type(&mut self, interrupt_source: u64, trigger_type: Trigger) -> Result<(), EfiError> {
        let int_id = Self::source_to_intid(interrupt_source)?;

        self.gic_v3.set_trigger(int_id, Some(0), trigger_type);

        Ok(())
    }

    pub fn new(gic_v3: GicV3<'static>) -> Self {
        AArch64InterruptInitializer { gic_v3 }
    }
}
