//! Patina Performance Component
//!
//! This is the primary Patina Performance component, which enables performance analysis in the UEFI boot environment.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

extern crate alloc;

use crate::config;
use alloc::boxed::Box;
use core::{clone::Clone, convert::AsRef};
use mu_rust_helpers::perf_timer::{Arch, ArchFunctionality};
use patina_sdk::{
    boot_services::{BootServices, StandardBootServices, event::EventType, tpl::Tpl},
    component::{IntoComponent, hob::Hob, params::Config},
    error::EfiError,
    guid::{EVENT_GROUP_END_OF_DXE, PERFORMANCE_PROTOCOL},
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
pub struct Performance;

impl Performance {
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
                    Arch::perf_frequency(),
                    Arch::cpu_count_start(),
                    Arch::cpu_count_end(),
                )),
            )?
        };

        Ok(())
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

        let _ = Performance._entry_point(
            Rc::new(boot_services),
            Rc::new(runtime_services),
            Some(hob_perf_data_extractor),
            Some(mm_comm_region),
            fbpt,
        );
    }
}
