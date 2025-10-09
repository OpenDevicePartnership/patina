//! Patina Performance Component
//!
//! This is the primary Patina Performance component, which enables performance analysis in the UEFI boot environment.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;

use crate::config;
use alloc::boxed::Box;
use core::arch::x86_64;
use core::cell::OnceCell;
use core::sync::atomic::AtomicU64;
use core::{clone::Clone, convert::AsRef};
use core::{mem, ptr, slice};
use patina_acpi::acpi::StandardAcpiProvider;
use patina_acpi::acpi_table::{AcpiRsdp, AcpiTableHeader, AcpiXsdt};
use patina_acpi::signature::{self, ACPI_HEADER_LEN, DEFAULT_ACPI_TIMER_FREQUENCY};
use patina_sdk::{
    boot_services::{BootServices, StandardBootServices, event::EventType, tpl::Tpl},
    component::{IntoComponent, hob::Hob, params::Config},
    error::EfiError,
    guids::{EVENT_GROUP_END_OF_DXE, PERFORMANCE_PROTOCOL},
    performance::{
        _smm::MmCommRegion,
        globals::{get_static_state, set_load_image_count, set_perf_measurement_mask, set_static_state},
        measurement::{PerformanceProperty, create_performance_measurement, event_callback},
        record::hob::{HobPerformanceData, HobPerformanceDataExtractor},
        table::FirmwareBasicBootPerfTable,
    },
    runtime_services::{RuntimeServices, StandardRuntimeServices},
    tpl_mutex::TplMutex,
    uefi_protocol::performance_measurement::EdkiiPerformanceMeasurement,
};
use r_efi::system::EVENT_GROUP_READY_TO_BOOT;

pub use mu_rust_helpers::function;

/// Performance Component.
#[derive(IntoComponent)]
pub struct Performance {
    pub perf_timer: OnceCell<PerformanceTimer>,
}

impl Performance {
    pub fn new() -> Self {
        Self { perf_timer: OnceCell::new() }
    }

    /// Entry point of [`Performance`]
    #[coverage(off)] // This is tested via the generic version, see _entry_point.
    pub fn entry_point(
        self,
        config: Config<config::PerfConfig>,
        boot_services: StandardBootServices,
        runtime_services: StandardRuntimeServices,
        records_buffers_hobs: Option<Hob<HobPerformanceData>>,
        mm_comm_region_hobs: Option<Hob<MmCommRegion>>,
    ) -> Result<(), EfiError> {
        if !config.enable_component {
            log::warn!("Patina Performance Component is not enabled, skipping entry point.");
            return Ok(());
        }

        set_perf_measurement_mask(config.enabled_measurements);

        set_static_state(StandardBootServices::clone(&boot_services)).unwrap_or_else(|_| {
            log::error!(
                "[{}]: Performance static state was set somewhere else. It should only be set here!",
                function!()
            );
        });

        let Some((_, fbpt)) = get_static_state() else {
            log::error!("[{}]: Performance static state was not initialized properly.", function!());
            return Err(EfiError::Aborted);
        };

        let Some(mm_comm_region_hobs) = mm_comm_region_hobs else {
            // If no MM communication region is provided, we can skip the SMM performance records.
            return self._entry_point(boot_services, runtime_services, records_buffers_hobs, None, fbpt);
        };

        let Some(mm_comm_region) = mm_comm_region_hobs.iter().find(|r| r.is_user_type()) else {
            return Ok(());
        };

        self.perf_timer.set(PerformanceTimer::new(config.rsdp_address));

        self._entry_point(boot_services, runtime_services, records_buffers_hobs, Some(*mm_comm_region), fbpt)
    }

    /// Entry point that have generic parameter.
    fn _entry_point<BB, B, RR, R, P, F>(
        self,
        boot_services: BB,
        runtime_services: RR,
        records_buffers_hobs: Option<P>,
        mm_comm_region: Option<MmCommRegion>,
        fbpt: &'static TplMutex<'static, F, B>,
    ) -> Result<(), EfiError>
    where
        BB: AsRef<B> + Clone + 'static,
        B: BootServices + 'static,
        RR: AsRef<R> + Clone + 'static,
        R: RuntimeServices + 'static,
        P: HobPerformanceDataExtractor,
        F: FirmwareBasicBootPerfTable,
    {
        // Register EndOfDxe event to allocate the boot performance table and report the table address through status code.
        boot_services.as_ref().create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(event_callback::report_fbpt_record_buffer),
            Box::new((BB::clone(&boot_services), RR::clone(&runtime_services), fbpt)),
            &EVENT_GROUP_END_OF_DXE,
        )?;

        // Handle optional `records_buffers_hobs`
        if let Some(records_buffers_hobs) = records_buffers_hobs {
            let (hob_load_image_count, hob_perf_records) = records_buffers_hobs
                .extract_hob_perf_data()
                .inspect(|(_, perf_buf)| {
                    log::info!("Performance: {} Hob performance records found.", perf_buf.iter().count());
                })
                .inspect_err(|_| {
                    log::error!(
                        "Performance: Error while trying to insert hob performance records, using default values"
                    )
                })
                .unwrap_or_default();

            // Initialize perf data from hob values.

            set_load_image_count(hob_load_image_count);
            fbpt.lock().set_perf_records(hob_perf_records);
        } else {
            log::info!("Performance: No Hob performance records provided.");
        }

        // Install the protocol interfaces for DXE performance.
        boot_services.as_ref().install_protocol_interface(
            None,
            Box::new(EdkiiPerformanceMeasurement { create_performance_measurement }),
        )?;

        // Register ReadyToBoot event to update the boot performance table for SMM performance data.
        // Only register if mm_comm_region is available
        if let Some(mm_comm_region) = mm_comm_region {
            boot_services.as_ref().create_event_ex(
                EventType::NOTIFY_SIGNAL,
                Tpl::CALLBACK,
                Some(event_callback::fetch_and_add_mm_performance_records),
                Box::new((BB::clone(&boot_services), mm_comm_region, fbpt)),
                &EVENT_GROUP_READY_TO_BOOT,
            )?;
        } else {
            log::info!(
                "Performance: No MM communication region available, skipping SMM performance event registration."
            );
        }

        // Install configuration table for performance property.
        unsafe {
            boot_services.as_ref().install_configuration_table(
                &PERFORMANCE_PROTOCOL,
                Box::new(PerformanceProperty::new(
                    self.perf_timer.get().ok_or(EfiError::NotReady)?.perf_frequency(),
                    PerformanceTimer::cpu_count_start(),
                    PerformanceTimer::cpu_count_end(),
                )),
            )?
        };

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PmTimer {
    IoPort { port: u16 },
    Mmio { base: u64 },
}

struct PerformanceTimer {
    rsdp_address: usize,
    perf_frequency: AtomicU64, // Frequency should be consistent across a single instance + boot, so calculate once and cache.
}

/// Timer functionality.
impl PerformanceTimer {
    const DEFAULT_PM_PORT: u16 = 0x608; // This is a good default because this is the default port on Q35, which is most likely the only platform that will use the ACPI PM timer.

    pub fn new(rsdp_address: usize) -> Self {
        Self { rsdp_address, perf_frequency: AtomicU64::new(0) }
    }

    pub fn read_fadt_timer_info(rsdp_address: usize) -> Option<PmTimer> {
        let xsdt_address =
            StandardAcpiProvider::<StandardBootServices>::get_xsdt_address_from_rsdp(rsdp_address as u64).ok()?;
        let xsdt_ptr = xsdt_address as *const AcpiTableHeader;
        let xsdt_length = (unsafe { *xsdt_ptr }).length;

        let entries = (xsdt_length as usize - ACPI_HEADER_LEN) / mem::size_of::<u64>();
        for i in 0..entries {
            // Find the address value of the next XSDT entry.
            let entry_addr = StandardAcpiProvider::<StandardBootServices>::get_xsdt_entry_from_hob(
                i,
                xsdt_address as *const u8,
                xsdt_length as usize,
            )
            .ok()?;
            let tbl_header = unsafe { *(entry_addr as *const AcpiTableHeader) };
            if tbl_header.signature == signature::FACP {
                let fadt = unsafe { *(entry_addr as *const patina_acpi::acpi_table::AcpiFadt) };
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

            log::info!("Calibrated TSC frequency: {} Hz over {} ns ({} PM ticks)", freq_hz, delta_time_ns, delta_pm);
            freq_hz
        }
    }

    #[cfg(target_arch = "x86_64")]
    fn cpu_count() -> u64 {
        #[cfg(feature = "validate_cpu_features")]
        {
            // TSC support in bit 4.
            if (unsafe { x86_64::__cpuid(0x01) }.edx & 0x10) != 0x10 {
                panic!("CPU does not support TSC");
            }
            // Invariant TSC support in bit 8.
            if (unsafe { x86_64::__cpuid(0x80000007) }.edx & 0x100) != 0x100 {
                panic!("CPU does not support Invariant TSC");
            }
        }
        unsafe { x86_64::_rdtsc() }
    }

    #[cfg(target_arch = "aarch64")]
    fn cpu_count() -> u64 {
        registers::CNTPCT_EL0.get()
    }

    #[cfg(target_arch = "x86_64")]
    fn perf_frequency(&self) -> u64 {
        use core::{arch::x86_64::CpuidResult, sync::atomic::Ordering};

        let cached = self.perf_frequency.load(Ordering::Relaxed);
        if cached != 0 {
            return cached;
        }

        let hypervisor_leaf = unsafe { x86_64::__cpuid(0x1) };
        let is_vm = (hypervisor_leaf.ecx & (1 << 31)) != 0;

        if is_vm {
            log::warn!("Running in a VM - CPUID-based frequency may not be reliable.");
        }

        let CpuidResult {
            eax, // Ratio of TSC frequency to Core Crystal Clock frequency, denominator.
            ebx, // Ratio of TSC frequency to Core Crystal Clock frequency, numerator.
            ecx, // Core Crystal Clock frequency, in units of Hz.
            ..
        } = unsafe { x86_64::__cpuid(0x15) };

        // If not a VM, attempt to use CPUID leaf 0x15
        if !is_vm && ecx != 0 && eax != 0 && ebx != 0 {
            let frequency = (ecx as u64 * ebx as u64) / eax as u64;
            self.perf_frequency.store(frequency, Ordering::Relaxed);
            log::trace!("Used CPUID leaf 0x15 to determine CPU frequency: {}", frequency);
            return frequency;
        }

        // If VM or CPUID 0x15 fails, attempt to use CPUID 0x16
        // Based on testing in QEMU, leaf 0x16 is generally more reliable on VMs
        let CpuidResult { eax, .. } = unsafe { x86_64::__cpuid(0x16) };
        if eax != 0 {
            // Leaf 0x16 gives the frequency in MHz.
            let frequency = (eax * 1_000_000) as u64;
            self.perf_frequency.store(frequency, Ordering::Relaxed);
            log::trace!("Used CPUID leaf 0x16 to determine CPU frequency: {}", frequency);
            return frequency;
        }

        log::warn!("Unable to determine CPU frequency using CPUID leaves, using default ACPI timer frequency");
        let pm_timer = Self::read_fadt_timer_info(self.rsdp_address).expect("ACPI PM timer unavailable");
        let alt_freq = Self::calibrate_tsc_frequency(pm_timer);
        self.perf_frequency.store(alt_freq, Ordering::Relaxed);

        alt_freq
    }

    // This can also use the ACPI PM timer but seems like it doesn't need to.
    #[cfg(target_arch = "aarch64")]
    fn perf_frequency(&self) -> u64 {
        registers::CNTFRQ_EL0.get()
    }

    fn cpu_count_start() -> u64 {
        0
    }
    /// Value that the performance counter ends with before it rolls over.
    fn cpu_count_end() -> u64 {
        u64::MAX
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    use alloc::rc::Rc;
    use core::{assert_eq, ptr};
    use r_efi::efi;

    use patina_sdk::{
        boot_services::{MockBootServices, c_ptr::CPtr},
        runtime_services::MockRuntimeServices,
        uefi_protocol::{ProtocolInterface, performance_measurement::EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID},
    };

    use patina_sdk::performance::{
        measurement::event_callback, record::PerformanceRecordBuffer, record::hob::MockHobPerformanceDataExtractor,
        table::MockFirmwareBasicBootPerfTable,
    };

    #[test]
    fn test_entry_point() {
        let mut boot_services = MockBootServices::new();
        boot_services.expect_raise_tpl().return_const(Tpl::APPLICATION);
        boot_services.expect_restore_tpl().return_const(());

        // Test that the protocol in installed.
        boot_services
            .expect_install_protocol_interface::<EdkiiPerformanceMeasurement, Box<_>>()
            .once()
            .withf_st(|handle, _protocol_interface| {
                assert_eq!(&None, handle);
                assert_eq!(EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID, EdkiiPerformanceMeasurement::PROTOCOL_GUID);
                true
            })
            .returning(|_, protocol_interface| Ok((1 as efi::Handle, protocol_interface.metadata())));

        // Test that an event to report the fbpt at the end of dxe is created.
        boot_services
            .expect_create_event_ex::<Box<(
                Rc<MockBootServices>,
                Rc<MockRuntimeServices>,
                &TplMutex<'static, MockFirmwareBasicBootPerfTable, MockBootServices>,
            )>>()
            .once()
            .withf_st(|event_type, notify_tpl, notify_function, _notify_context, event_group| {
                assert_eq!(&EventType::NOTIFY_SIGNAL, event_type);
                assert_eq!(&Tpl::CALLBACK, notify_tpl);
                assert_eq!(
                    event_callback::report_fbpt_record_buffer::<
                        Rc<_>,
                        MockBootServices,
                        Rc<_>,
                        MockRuntimeServices,
                        MockFirmwareBasicBootPerfTable,
                    > as usize,
                    notify_function.unwrap() as usize
                );
                assert_eq!(&EVENT_GROUP_END_OF_DXE, event_group);
                true
            })
            .return_const_st(Ok(1_usize as efi::Event));

        // Test that an event to update the fbpt with smm data when ready to boot is created.
        boot_services
            .expect_create_event_ex::<Box<(
                Rc<MockBootServices>,
                MmCommRegion,
                &TplMutex<'static, MockFirmwareBasicBootPerfTable, MockBootServices>,
            )>>()
            .once()
            .withf_st(|event_type, notify_tpl, notify_function, _notify_context, event_group| {
                assert_eq!(&EventType::NOTIFY_SIGNAL, event_type);
                assert_eq!(&Tpl::CALLBACK, notify_tpl);
                assert_eq!(
                    event_callback::fetch_and_add_mm_performance_records::<
                        Rc<_>,
                        MockBootServices,
                        MockFirmwareBasicBootPerfTable,
                    > as usize,
                    notify_function.unwrap() as usize
                );
                assert_eq!(&EVENT_GROUP_READY_TO_BOOT, event_group);
                true
            })
            .return_const_st(Ok(1_usize as efi::Event));

        // Test that the address of the fbpt is installed to the configuration table.
        boot_services
            .expect_install_configuration_table::<Box<PerformanceProperty>>()
            .once()
            .withf(|guid, _data| {
                assert_eq!(&PERFORMANCE_PROTOCOL, guid);
                true
            })
            .return_const(Ok(()));

        let runtime_services = MockRuntimeServices::new();

        let mut hob_perf_data_extractor = MockHobPerformanceDataExtractor::new();
        hob_perf_data_extractor
            .expect_extract_hob_perf_data()
            .once()
            .returning(|| Ok((10, PerformanceRecordBuffer::new())));

        let mm_comm_region = MmCommRegion { region_type: 1, region_address: 10, region_nb_pages: 1 };

        let mut fbpt = MockFirmwareBasicBootPerfTable::new();
        fbpt.expect_set_perf_records().once().return_const(());

        let fbpt = TplMutex::new(unsafe { &*ptr::addr_of!(boot_services) }, Tpl::NOTIFY, fbpt);
        let fbpt = unsafe { &*ptr::addr_of!(fbpt) };

        let _ = Performance::new()._entry_point(
            Rc::new(boot_services),
            Rc::new(runtime_services),
            Some(hob_perf_data_extractor),
            Some(mm_comm_region),
            fbpt,
        );
    }
}
