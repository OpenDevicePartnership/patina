use core::sync::atomic::{AtomicBool, AtomicU64};

#[cfg(target_arch = "x86_64")]
pub use x64::X64 as Arch;

#[cfg(target_arch = "aarch64")]
pub use aarch64::Aarch64 as Arch;

#[derive(Debug, Clone, Copy)]
pub enum PmTimer {
    IoPort { port: u16 },
    Mmio { base: u64 },
}

pub trait ArchFunctionality {
    /// Value of the counter.
    fn cpu_count() -> u64;
    /// Value in Hz of how often the counter increment.
    fn perf_frequency(rsdp_address: usize) -> u64;
    /// Value that the performance counter ends with before it rolls over.
    fn cpu_count_end(used_acpi_fallback: bool) -> u64;
}

pub struct PerformanceTimer {
    rsdp_address: usize,
    perf_frequency: AtomicU64, // Frequency should be consistent across a single instance + boot, so calculate once and cache.
    used_acpi_fallback: AtomicBool, // The ACPI PM timer has a different max value than TSC.
}

/// Timer functionality.
impl PerformanceTimer {
    const DEFAULT_PM_PORT: u16 = 0x608; // This is a good default because this is the default port on Q35, which is most likely the only platform that will use the ACPI PM timer.

    pub fn new(rsdp_address: usize) -> Self {
        Self { rsdp_address, perf_frequency: AtomicU64::new(0), used_acpi_fallback: AtomicBool::new(false) }
    }

    pub fn cpu_count(&self) -> u64 {
        Arch::cpu_count()
    }

    pub fn perf_frequency(&self) -> u64 {
        use core::sync::atomic::Ordering;

        let cached = self.perf_frequency.load(Ordering::Relaxed);
        if cached != 0 {
            return cached;
        }

        let freq = Arch::perf_frequency(self.rsdp_address);
        self.perf_frequency.store(freq, Ordering::Relaxed);
        freq
    }

    pub fn cpu_count_start() -> u64 {
        0
    }

    pub fn cpu_count_end(&self) -> u64 {
        Arch::cpu_count_end(self.used_acpi_fallback.load(core::sync::atomic::Ordering::Relaxed))
    }
}

#[cfg(target_arch = "x86_64")]
mod x64 {
    use core::{arch::x86_64, mem};

    use crate::{
        acpi::{
            acpi_table::{AcpiFadt, AcpiTableHeader},
            signature::{self, ACPI_HEADER_LEN, DEFAULT_ACPI_TIMER_FREQUENCY},
            standard::StandardAcpiProvider,
        },
        performance::perf_timer::{ArchFunctionality, PerformanceTimer, PmTimer},
    };

    pub struct X64;

    impl ArchFunctionality for X64 {
        fn cpu_count() -> u64 {
            unsafe { x86_64::_rdtsc() }
        }

        fn perf_frequency(rsdp_address: usize) -> u64 {
            use core::arch::x86_64::CpuidResult;
            let mut frequency = 0u64;

            // CPUID leaf 0x15
            let CpuidResult { eax, ebx, ecx, .. } = unsafe { x86_64::__cpuid(0x15) };
            if eax != 0 && ebx != 0 && ecx != 0 {
                frequency = (ecx as u64 * ebx as u64) / eax as u64;
            } else {
                let CpuidResult { eax, .. } = unsafe { x86_64::__cpuid(0x16) };
                if eax != 0 {
                    frequency = (eax * 1_000_000) as u64;
                }
            }

            if frequency == 0 {
                // fallback to calibration
                let pm_timer = Self::read_fadt_timer_info(rsdp_address)
                    .unwrap_or(PmTimer::IoPort { port: PerformanceTimer::DEFAULT_PM_PORT });
                frequency = Self::calibrate_tsc_frequency(pm_timer);
            }

            frequency
        }

        fn cpu_count_end(used_acpi_fallback: bool) -> u64 {
            if used_acpi_fallback {
                0x1_0000_0000 // 32-bit wraparound
            } else {
                u64::MAX
            }
        }
    }

    impl X64 {
        pub fn calibrate_tsc_frequency(pm_timer: PmTimer) -> u64 {
            unsafe {
                // Wait for a PM timer edge to avoid partial intervals
                let mut start_pm = Self::read_pm_timer(pm_timer);
                let mut next_pm;
                loop {
                    next_pm = Self::read_pm_timer(pm_timer);
                    if next_pm != start_pm {
                        break;
                    }
                }
                start_pm = next_pm;

                // Record starting TSC
                let start_tsc = x86_64::_rdtsc();

                // Hz = ticks/second. Divided by 20 ~ ticks / 50 ms
                const TARGET_INTERVAL_SIZE: u64 = 20;
                let target_ticks = (DEFAULT_ACPI_TIMER_FREQUENCY / TARGET_INTERVAL_SIZE) as u32;

                let mut end_pm;
                loop {
                    end_pm = Self::read_pm_timer(pm_timer);
                    let delta = end_pm.wrapping_sub(start_pm);
                    if delta >= target_ticks {
                        break;
                    }
                }

                // Record ending TSC
                let end_tsc = x86_64::_rdtsc();

                // Time elapsed based on PM timer ticks
                let delta_pm = end_pm.wrapping_sub(start_pm) as u64;
                let delta_time_ns = (delta_pm * 1_000_000_000) / DEFAULT_ACPI_TIMER_FREQUENCY;

                // Rdtsc ticks
                let delta_tsc = end_tsc - start_tsc;

                // Frequency = Rdstc ticks / elapsed time
                let freq_hz = (delta_tsc * 1_000_000_000) / delta_time_ns;

                log::info!(
                    "Calibrated TSC frequency: {} Hz over {} ns ({} PM ticks)",
                    freq_hz,
                    delta_time_ns,
                    delta_pm
                );
                freq_hz
            }
        }

        pub fn read_fadt_timer_info(rsdp_address: usize) -> Option<PmTimer> {
            let xsdt_address = StandardAcpiProvider::get_xsdt_address_from_rsdp(rsdp_address as u64).ok()?;
            let xsdt_ptr = xsdt_address as *const AcpiTableHeader;
            let xsdt_length = (unsafe { *xsdt_ptr }).length;

            let entries = (xsdt_length as usize - ACPI_HEADER_LEN) / mem::size_of::<u64>();
            for i in 0..entries {
                // Find the address value of the next XSDT entry.
                let entry_addr =
                    StandardAcpiProvider::get_xsdt_entry_from_hob(i, xsdt_address as *const u8, xsdt_length as usize)
                        .ok()?;
                let tbl_header = unsafe { *(entry_addr as *const AcpiTableHeader) };
                if tbl_header.signature == signature::FACP {
                    let fadt = unsafe { *(entry_addr as *const AcpiFadt) };
                    if let Some(tmr_info) = fadt.x_pm_timer_blk() {
                        if tmr_info.address_space_id == 0 {
                            // MMIO case.
                            return Some(PmTimer::Mmio { base: tmr_info.address });
                        } else if tmr_info.address_space_id == 1 {
                            // I/O Port case. Mask to 16 bits.
                            return Some(PmTimer::IoPort { port: (tmr_info.address & 0xFFFF) as u16 });
                        } else {
                            log::warn!(
                                "FADT PM Timer Block has unsupported address space ID: {}",
                                tmr_info.address_space_id
                            );
                            return None;
                        }
                    } else {
                        log::warn!("FADT PM Timer Block not found or invalid.");
                        return None;
                    }
                }
            }

            log::warn!("FADT table not found in XSDT.");
            None
        }

        fn read_pm_timer(pm_timer: PmTimer) -> u32 {
            match pm_timer {
                PmTimer::IoPort { port } => {
                    let value: u32;
                    unsafe {
                        core::arch::asm!(
                            "in eax, dx",
                            in("dx") port,
                            out("eax") value,
                            options(nomem, nostack, preserves_flags),
                        );
                    }
                    value
                }
                PmTimer::Mmio { base } => unsafe { core::ptr::read_volatile(base as *const u32) },
            }
        }
    }
}

#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use crate::perf_timer::ArchFunctionality;

    pub struct Aarch64;
    use aarch64_cpu::registers::{self, Readable};

    impl ArchFunctionality for Aarch64 {
        fn cpu_count() -> u64 {
            registers::CNTPCT_EL0.get()
        }

        fn perf_frequency(_: usize) -> u64 {
            registers::CNTFRQ_EL0.get()
        }

        fn cpu_count_end(_: bool) -> u64 {
            u64::MAX
        }
    }
}
