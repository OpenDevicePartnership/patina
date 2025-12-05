use arm_gic::{
    IntId, Trigger, UniqueMmioPointer,
    gicv3::{
        GicCpuInterface, GicDistributorContext, GicRedistributorContext, GicRedistributorIterator, GicV3,
        InterruptGroup,
    },
};
use core::ptr::NonNull;
use patina::error::EfiError;

use patina::{read_sysreg, write_sysreg};

// This masks out bits in MPIDR_EL1 that are not part of the affinity fields used to identify a core.
// See definition of MPIDR_EL1 in the ARM Architecture Reference Manual for details.
const MPIDR_AFFINITY_MASK: u64 = 0x0000_00ff_00ff_ffff;

// Create basic enum for GIC version
#[allow(dead_code)]
#[derive(PartialEq)]
pub enum GicVersion {
    ArmGicV2 = 2,
    ArmGicV3 = 3,
}

// Determine the current exception level
pub fn get_current_el() -> u64 {
    read_sysreg!(CurrentEL)
}

#[allow(dead_code)]
fn get_control_system_reg_enable() -> u64 {
    let current_el = get_current_el();
    match current_el {
        0x08 => read_sysreg!(ICC_SRE_EL2),
        0x04 => read_sysreg!(ICC_SRE_EL1),
        _ => panic!("Invalid current EL {}", current_el),
    }
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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

pub struct AArch64InterruptInitializer {
    gic_v3: GicV3<'static>,
    cpu_r_idx: usize,
}

impl AArch64InterruptInitializer {
    /// Create AArch64InterruptInitializer from register bases and initialize GICv3/4 hardware for use by the current
    /// cpu.
    ///
    /// * Enable affinity routing and non-secure group 1 interrupts.
    /// * Enable gic cpu interface
    /// * Enable gic distributor
    ///
    /// # Safety
    ///
    /// `gicd_base` must point to the GIC Distributor register space.
    ///
    /// `gicr_base` must point to the GIC Redistributor register space.
    ///
    /// Caller must guarantee that access to these registers is exclusive to this AArch64InterruptInitializer instance
    ///
    pub unsafe fn new(gicd_base: *mut u64, gicr_base: *mut u64) -> Result<Self, EfiError> {
        let gic_v = get_system_gic_version();
        if gic_v == GicVersion::ArmGicV2 {
            debug_assert!(false, "GICv2 is not supported");
            return Err(EfiError::Unsupported);
        }

        // Convert raw GIC address pointers to appropriate types.
        // Safety: function safety requirements guarantee exclusive access to the GICR registers.
        let (gicd, gicr) = unsafe {
            let gicd = UniqueMmioPointer::new(NonNull::new(gicd_base as _).ok_or(EfiError::InvalidParameter)?);
            let gicr = NonNull::new(gicr_base as _).ok_or(EfiError::InvalidParameter)?;
            (gicd, gicr)
        };

        //Determine the linear index of the current core and the count of cores.
        let mut cpu_r_idx = usize::MAX;
        let mut r_count = 0;

        let mpidr = read_sysreg!(MPIDR_EL1) & MPIDR_AFFINITY_MASK;
        log::debug!("Current CPU MPIDR: {:#x}", mpidr);
        // Support for GIC v4 is backward compatible with GIC v3, so always enable it.
        // Safety: function safety requirements guarantee exclusive access to the GICR registers.
        for (index, redistributor) in unsafe { GicRedistributorIterator::new(gicr, true) }.enumerate() {
            r_count = index + 1;
            if redistributor.typer().core_mpidr() == mpidr {
                if cpu_r_idx != usize::MAX {
                    log::error!(
                        "Multiple redistributors found for current cpu mpidr {:#x} at index {} and {}",
                        mpidr,
                        cpu_r_idx,
                        index
                    );
                    return Err(EfiError::DeviceError);
                }
                cpu_r_idx = index;
            }
            log::debug!(
                "Redistributor Index: {}, MPIDR: {:#x} {}",
                index,
                redistributor.typer().core_mpidr(),
                if redistributor.typer().core_mpidr() == mpidr { "(Current CPU)" } else { "" }
            );
        }
        log::info!("Total Redistributors: {}, Current CPU Redistributor Index: {}", r_count, cpu_r_idx);
        if cpu_r_idx == usize::MAX {
            log::error!("Failed to find redistributor for current cpu");
            return Err(EfiError::DeviceError);
        }

        // Initialize the GIC:
        // Enable affinity routing and non-secure group 1 interrupts.
        // Enable gic cpu interface
        // Enable gic distributor
        // Enable support for GICv4; this is backward compatible with GICv3, so always enable it.

        // Safety: function safety requirements guarantee exclusive access to the GICR registers.
        let mut gic_v3 = unsafe { GicV3::new(gicd, gicr, r_count, true) };
        gic_v3.setup(cpu_r_idx);

        // Set binary point reg to 0x7 (no preemption)
        // Safety: this is a legal value for BPR1 register.
        // Refer to "Arm Generic Interrupt Controller Architecture Specification GIC
        // architecture version 3 and Version 4" (Arm IHI 0069H.b ID041224)
        // 12.2.5: "ICC_BPR1_EL1, Interrupt Controller Binary Point Register 1"
        write_sysreg!(reg ICC_BPR1_EL1, 0x7u64);

        // Set priority mask reg to 0xff to allow all priorities through
        GicCpuInterface::set_priority_mask(0xff);
        Ok(Self { gic_v3, cpu_r_idx })
    }

    fn source_to_intid(&self, interrupt_source: u64) -> Result<IntId, EfiError> {
        let int_id: u32 = interrupt_source.try_into().map_err(|_| EfiError::InvalidParameter)?;
        let int_id = match int_id {
            x if x < IntId::SGI_COUNT => IntId::sgi(x),
            x if x < IntId::SGI_COUNT + IntId::PPI_COUNT => IntId::ppi(x - IntId::SGI_COUNT),
            x => {
                let int_id = IntId::spi(x - IntId::SGI_COUNT - IntId::PPI_COUNT);
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
        self.gic_v3
            .enable_interrupt(self.source_to_intid(interrupt_source)?, Some(self.cpu_r_idx), true)
            .map_err(|_| EfiError::InvalidParameter)
    }

    /// Disables the specified interrupt source.
    pub fn disable_interrupt_source(&mut self, interrupt_source: u64) -> Result<(), EfiError> {
        self.gic_v3
            .enable_interrupt(self.source_to_intid(interrupt_source)?, Some(self.cpu_r_idx), false)
            .map_err(|_| EfiError::InvalidParameter)
    }

    // Helper constants for constructing context.
    const MAX_REDISTRIBUTOR_PPI: usize = GicRedistributorContext::ireg_count(96); //Maximum number of PPI + Extended PPI supported by GICv3
    const MAX_DISTRIBUTOR_SPI: usize = GicDistributorContext::ireg_count(988); // Maximum number of SPIs supported by GICv3
    const MAX_DISTRIBUTOR_ESPI: usize = GicDistributorContext::ireg_e_count(1024); // Maximum number of Extended SPIs supported by GICv3
    /// Returns the interrupt source state.
    pub fn get_interrupt_source_state(&mut self, interrupt_source: u64) -> Result<bool, EfiError> {
        let index = (interrupt_source / 32) as usize;
        let bit = 1 << (interrupt_source % 32);

        // validates the interrupt source
        let int_id = self.source_to_intid(interrupt_source)?;

        if int_id.is_private() {
            // arm-gic does not presently provide a way to directly read the interrupt state for a given interrupt.
            // so save the current redistributor context and read the ISENABLER from there.
            let redistributor = self.gic_v3.redistributor(self.cpu_r_idx).expect("Invalid redistributor");
            let mut context = GicRedistributorContext::<{ Self::MAX_REDISTRIBUTOR_PPI }>::default();
            redistributor.save(&mut context).map_err(|_| EfiError::DeviceError)?;
            Ok(context.isenabler()[index] & bit != 0)
        } else {
            // arm-gic does not presently provide a way to directly read the interrupt state for a given interrupt.
            // so save the current distributor context and read the ISENABLER from there.
            let distributor = self.gic_v3.distributor();
            let mut context =
                GicDistributorContext::<{ Self::MAX_DISTRIBUTOR_SPI }, { Self::MAX_DISTRIBUTOR_ESPI }>::default();
            distributor.save(&mut context).map_err(|_| EfiError::DeviceError)?;
            Ok(context.isenabler()[index] & bit != 0)
        }
    }

    /// Excutes EOI for the specified interrupt.
    pub fn end_of_interrupt(&self, interrupt_source: u64) -> Result<(), EfiError> {
        GicCpuInterface::end_interrupt(self.source_to_intid(interrupt_source)?, InterruptGroup::Group1);
        Ok(())
    }

    /// Returns the trigger type for the specified interrupt.
    pub fn get_trigger_type(&mut self, interrupt_source: u64) -> Result<Trigger, EfiError> {
        let index = (interrupt_source / 16) as usize;
        let bit = 1 << (interrupt_source % 16);

        // validates the interrupt source
        let int_id = self.source_to_intid(interrupt_source)?;

        let level = if int_id.is_private() {
            // arm-gic does not presently provide a way to directly read the interrupt state for a given interrupt.
            // so save the current redistributor context and read the ICFGR from there.
            let redistributor = self.gic_v3.redistributor(self.cpu_r_idx).expect("Invalid redistributor");
            let mut context = GicRedistributorContext::<{ Self::MAX_REDISTRIBUTOR_PPI }>::default();
            redistributor.save(&mut context).map_err(|_| EfiError::DeviceError)?;
            context.icfgr()[index] & bit != 0
        } else {
            // arm-gic does not presently provide a way to directly read the interrupt state for a given interrupt.
            // so save the current distributor context and read the ICFGR from there.
            let distributor = self.gic_v3.distributor();
            let mut context =
                GicDistributorContext::<{ Self::MAX_DISTRIBUTOR_SPI }, { Self::MAX_DISTRIBUTOR_ESPI }>::default();
            distributor.save(&mut context).map_err(|_| EfiError::DeviceError)?;
            context.icfgr()[index] & bit != 0
        };

        Ok(if level { Trigger::Level } else { Trigger::Edge })
    }

    /// Sets the trigger type for the specified interrupt.
    pub fn set_trigger_type(&mut self, interrupt_source: u64, trigger_type: Trigger) -> Result<(), EfiError> {
        self.gic_v3
            .set_trigger(self.source_to_intid(interrupt_source)?, Some(self.cpu_r_idx), trigger_type)
            .map_err(|_| EfiError::InvalidParameter)
    }

    pub fn max_int(&self) -> u32 {
        self.gic_v3.typer().num_spis()
    }
}
