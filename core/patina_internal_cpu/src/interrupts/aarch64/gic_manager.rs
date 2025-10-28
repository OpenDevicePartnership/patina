use arm_gic::{
    IntId, Trigger,
    gicv3::{GicV3, InterruptGroup},
};
use patina::error::EfiError;
use safe_mmio::field;

use patina::{read_sysreg, write_sysreg};

// Create basic enum for GIC version
#[derive(PartialEq)]
pub enum GicVersion {
    ArmGicV2 = 2,
    ArmGicV3 = 3,
}

// Determine the current exception level
pub fn get_current_el() -> u64 {
    read_sysreg!(CurrentEL)
}

fn get_control_system_reg_enable() -> u64 {
    let current_el = get_current_el();
    match current_el {
        0x08 => read_sysreg!(ICC_SRE_EL2),
        0x04 => read_sysreg!(ICC_SRE_EL1),
        _ => panic!("Invalid current EL {}", current_el),
    }
}

fn set_control_system_reg_enable(icc_sre: u64) -> u64 {
    let current_el = get_current_el();
    match current_el {
        0x08 => {
            write_sysreg!(reg ICC_SRE_EL2, icc_sre);
        }
        0x04 => {
            write_sysreg!(reg ICC_SRE_EL1, icc_sre);
        }
        _ => panic!("Invalid current EL {}", current_el),
    }

    get_control_system_reg_enable()
}

fn get_system_gic_version() -> GicVersion {
    let pfr0_el1 = read_sysreg!(ID_AA64PFR0_EL1);

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
    // Safety: this is a legal value for BPR1 register.
    // Refer to "Arm Generic Interrupt Controller Architecture Specification GIC
    // architecture version 3 and Version 4" (Arm IHI 0069H.b ID041224)
    // 12.2.5: "ICC_BPR1_EL1, Interrupt Controller Binary Point Register 1"
    write_sysreg!(reg ICC_BPR1_EL1, 0x7u64);

    // Set priority mask reg to 0xff to allow all priorities through
    GicV3::set_priority_mask(0xff);
    Ok(gic_v3)
}

pub struct AArch64InterruptInitializer<'a> {
    pub gic_v3: GicV3<'a>,
}

impl AArch64InterruptInitializer<'_> {
    fn source_to_intid(&self, interrupt_source: u64) -> Result<IntId, EfiError> {
        let int_id: u32 = interrupt_source.try_into().map_err(|_| EfiError::InvalidParameter)?;
        let int_id = match int_id {
            x if x < IntId::SGI_COUNT => IntId::sgi(x),
            x if x < IntId::SGI_COUNT + IntId::PPI_COUNT => IntId::ppi(x - IntId::SGI_COUNT),
            x => {
                let int_id = IntId::spi(x - IntId::SGI_COUNT + IntId::PPI_COUNT);
                if self.gic_v3.typer().num_spis() < int_id.into() {
                    Err(EfiError::InvalidParameter)?;
                }
                int_id
            }
        };
        Ok(int_id)
    }

    /// Enables the specified interrupt source.
    pub fn enable_interrupt_source(&mut self, interrupt_source: u64) -> Result<(), EfiError> {
        self.gic_v3.enable_interrupt(self.source_to_intid(interrupt_source)?, Some(0), true);
        Ok(())
    }

    /// Disables the specified interrupt source.
    pub fn disable_interrupt_source(&mut self, interrupt_source: u64) -> Result<(), EfiError> {
        self.gic_v3.enable_interrupt(self.source_to_intid(interrupt_source)?, Some(0), false);
        Ok(())
    }

    /// Returns the interrupt source state.
    pub fn get_interrupt_source_state(&mut self, interrupt_source: u64) -> Result<bool, EfiError> {
        let index = (interrupt_source / 32) as usize;
        let bit = 1 << (interrupt_source % 32);

        // validates the interrupt source
        let int_id = self.source_to_intid(interrupt_source)?;

        if int_id.is_private() {
            let mut sgi = self.gic_v3.sgi_ptr(0);
            Ok(field!(sgi, isenabler0).read() & bit != 0)
        } else {
            let mut gicd = self.gic_v3.gicd_ptr();
            //source_to_intid() validates the interrupt source number, so index computed must be valid.
            Ok(field!(gicd, isenabler).get(index).unwrap().read() & bit != 0)
        }
    }

    /// Excutes EOI for the specified interrupt.
    pub fn end_of_interrupt(&self, interrupt_source: u64) -> Result<(), EfiError> {
        GicV3::end_interrupt(self.source_to_intid(interrupt_source)?, InterruptGroup::Group1);
        Ok(())
    }

    /// Returns the trigger type for the specified interrupt.
    pub fn get_trigger_type(&mut self, interrupt_source: u64) -> Result<Trigger, EfiError> {
        let index = (interrupt_source / 16) as usize;
        let bit = 1 << (interrupt_source % 16);

        // validates the interrupt source
        let int_id = self.source_to_intid(interrupt_source)?;

        let level = if int_id.is_private() {
            let mut sgi = self.gic_v3.sgi_ptr(0);
            field!(sgi, icfgr).get(index).unwrap().read() & bit != 0
        } else {
            let mut gicd = self.gic_v3.gicd_ptr();
            //source_to_intid() validates the interrupt source number, so index computed must be valid.
            field!(gicd, icfgr).get(index).unwrap().read() & bit != 0
        };

        Ok(if level { Trigger::Level } else { Trigger::Edge })
    }

    /// Sets the trigger type for the specified interrupt.
    pub fn set_trigger_type(&mut self, interrupt_source: u64, trigger_type: Trigger) -> Result<(), EfiError> {
        self.gic_v3.set_trigger(self.source_to_intid(interrupt_source)?, Some(0), trigger_type);
        Ok(())
    }

    /// Instantiates a new AArch64InterruptInitializer
    pub fn new(gic_v3: GicV3<'static>) -> Self {
        AArch64InterruptInitializer { gic_v3 }
    }
}
