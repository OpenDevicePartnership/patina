//! MP Services Protocol installation.
//!
//! Hosts the [`MpServicesProtocolInstaller`] component, which owns the MP Services Protocol,
//! the underlying service, and managing the architecture specific code exposed by `patina_internal_cpu`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::{boxed::Box, vec::Vec};
use core::{num::NonZeroUsize, time::Duration};

use patina::{
    BinaryGuid,
    component::{
        component,
        hob::Hob,
        service::{
            Service,
            memory::{AccessType, AllocationOptions, MemoryManager},
            perf_timer::ArchTimerFunctionality,
            uefi_services::{
                event::{EventServices, EventServicesExt, Tpl},
                protocol::{ProtocolServices, ProtocolServicesExt},
                timer_event::{TimerEventServices, TimerEventServicesExt, TimerType},
            },
        },
    },
    error::EfiError,
    pi::{
        protocol::status_code::StatusCodeProtocol,
        status_code::{EFI_COMPUTING_UNIT_HOST_PROCESSOR, EFI_CU_HP_EC_SELF_TEST, EFI_ERROR_CODE, EFI_ERROR_MAJOR},
    },
    standard::efi,
    uefi::event::CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP_GUID,
    uefi_pages_to_size, uefi_size_to_pages,
};

mod dispatch;
mod hob;
mod notification;
mod protocol;
mod services;

use hob::{MpHandOff, MpHandOffConfig, MpInformation2};
use patina_internal_cpu::mp::{ApContext, MpDispatcher, MpSupport};
use protocol::MpProtocolWrapper;
use services::MpServices;

use crate::cpu::mp_services::hob::MpHobs;

/// Period of the non-blocking notification poll timer.
const NOTIFICATION_POLL_PERIOD: Duration = Duration::from_micros(100);

/// Event group signaled when cache (MTRR) attributes change, driving AP resync.
const CACHE_ATTRIBUTE_CHANGE_GUID: BinaryGuid = CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP_GUID;

/// This component installs the MP Services Protocol.
#[derive(Default)]
pub(crate) struct MpServicesComponent;

/// Optional MP HOBs consumed by [`MpServicesComponent`].
type MpHobParams<'h> = (Option<Hob<'h, MpInformation2>>, Option<Hob<'h, MpHandOff>>, Option<Hob<'h, MpHandOffConfig>>);

#[component]
impl MpServicesComponent {
    fn entry_point(
        self,
        services: (Service<dyn ProtocolServices>, Service<dyn EventServices>, Service<dyn TimerEventServices>),
        mm: Service<dyn MemoryManager>,
        timer: Service<dyn ArchTimerFunctionality>,
        hobs: MpHobParams<'_>,
    ) -> Result<(), EfiError> {
        let (protocols, events, timer_events) = services;
        let (mp_info, mp_handoff, mp_handoff_config) = hobs;

        // Collect context and config.
        let parsed_hobs = MpHobs::parse_hobs(mp_info, mp_handoff, mp_handoff_config);
        let handoff = parsed_hobs.build_handoff();
        let ap_count = handoff.as_ref().map_or(0, |handoff| handoff.processors.len().saturating_sub(1));

        // Build the context for each AP.
        let contexts = self.allocate_ap_contexts(&mm, ap_count)?;

        // Initialize the MP architecture support.
        let mut mp = MpSupport::initialize(*mm, *timer)
            .inspect_err(|e| log::error!("Failed to initialize MP architecture support: {e:?}"))?;
        mp.setup_aps(contexts, handoff)
            .inspect_err(|e| log::error!("Failed to set up application processors: {e:?}"))?;

        self.report_or_defer_bist_errors(&protocols, parsed_hobs.bist_error_count());

        // Build the rust service.
        let services: &'static MpServices = Box::leak(Box::new(MpServices::new(mp, parsed_hobs.processors, *timer)));

        self.register_events(&events, &timer_events, services)?;
        protocols
            .install_protocol(None, Box::new(MpProtocolWrapper::new(services, events)))
            .inspect_err(|_| log::error!("Failed to install MP_SERVICES_PROTOCOL"))?;

        Ok(())
    }

    fn allocate_ap_contexts(
        &self,
        mm: &Service<dyn MemoryManager>,
        ap_count: usize,
    ) -> Result<&'static mut [ApContext], EfiError> {
        // Initialize the data and leak to a static slice.
        let mut contexts = Vec::with_capacity(ap_count);
        contexts.resize_with(ap_count, ApContext::default);
        let contexts = Box::leak(contexts.into_boxed_slice());

        // Setup the AP data needed from the core.
        let stack_pages = uefi_size_to_pages!(ApContext::STACK_SIZE);
        let total_stack_pages = stack_pages + 1; // +1 for the guard page.
        for context in contexts.iter_mut() {
            let stack_allocation = mm.allocate_pages(total_stack_pages, AllocationOptions::new()).map_err(|e| {
                log::error!("Failed to allocate AP stack: {e:?}");
                EfiError::OutOfResources
            })?;

            let stack_base = stack_allocation.into_raw_ptr::<u8>().ok_or(EfiError::OutOfResources)? as usize;

            // SAFETY: The first page belongs to this AP stack allocation and is
            // intentionally made inaccessible as its stack-overflow guard.
            unsafe {
                mm.set_page_attributes(stack_base, 1, AccessType::NoAccess, None).map_err(|e| {
                    log::error!("Failed to set AP stack guard page attributes: {e:?}");
                    EfiError::DeviceError
                })?;
            }

            let stack_top = NonZeroUsize::new(stack_base + uefi_pages_to_size!(total_stack_pages))
                .ok_or(EfiError::OutOfResources)?;

            // SAFETY: The stack is page-aligned, writable above its guard page,
            // exclusively assigned to this context, and intentionally leaked.
            unsafe { context.set_stack_top(stack_top)? };
        }

        Ok(contexts)
    }

    fn report_or_defer_bist_errors(&self, protocols: &Service<dyn ProtocolServices>, bist_error_count: usize) {
        if bist_error_count == 0 {
            return;
        }

        if Self::report_bist_errors(protocols, bist_error_count) {
            return;
        }

        let deferred_protocols = *protocols;
        let mut reported = false;
        if let Err(e) = protocols.on_protocol_installed::<StatusCodeProtocol>(Tpl::Callback, move |_handle| {
            if !reported {
                reported = Self::report_bist_errors(&deferred_protocols, bist_error_count);
            }
        }) {
            log::error!("MP Services: Failed to register status code protocol notification: {e:?}");
        }
    }

    fn report_bist_errors(protocols: &Service<dyn ProtocolServices>, bist_error_count: usize) -> bool {
        protocols
            .with_protocol::<StatusCodeProtocol, _>(|status_code| {
                for _ in 0..bist_error_count {
                    if let Err(status) = status_code.report_status_code(
                        EFI_ERROR_CODE | EFI_ERROR_MAJOR,
                        EFI_COMPUTING_UNIT_HOST_PROCESSOR | EFI_CU_HP_EC_SELF_TEST,
                        0,
                        patina::guid::DXE_CORE_ID.as_efi_guid(),
                    ) {
                        log::error!("MP Services: Failed to report BIST error status code: {status}");
                    }
                }
            })
            .is_ok()
    }

    fn register_events<M: MpDispatcher>(
        &self,
        events: &Service<dyn EventServices>,
        timer_events: &Service<dyn TimerEventServices>,
        services: &'static MpServices<M>,
    ) -> Result<(), EfiError> {
        // Create park event for EBS so that APs are parked before the OS takes over.
        if let Err(e) =
            events.on_event_group(BinaryGuid(efi::EVENT_GROUP_EXIT_BOOT_SERVICES), Tpl::Callback, move || {
                services.park();
                log::info!("MP Services: APs parked for ExitBootServices");
            })
        {
            log::error!("Failed to register MP AP-park event: {e:?}");
            services.park();
            return Err(e.into());
        }

        events
            .on_event_group(BinaryGuid(efi::EVENT_GROUP_READY_TO_BOOT), Tpl::Callback, move || {
                services.mark_ready_to_boot();
            })
            .inspect_err(|e| log::error!("Failed to register MP ReadyToBoot event: {e:?}"))?;

        // Create cache-attribute-change event AP synchronization.
        events
            .on_event_group(CACHE_ATTRIBUTE_CHANGE_GUID, Tpl::Callback, move || {
                services.synchronize();
            })
            .inspect_err(|e| log::error!("Failed to register MP MTRR sync event: {e:?}"))?;

        // Create a periodic timer to poll for non-blocking dispatches.
        let timer_event = timer_events
            .on_timer_event(Tpl::Notify, move || {
                services.poll_notifications();
            })
            .inspect_err(|e| log::error!("Failed to create MP notification timer event: {e:?}"))?;

        timer_events
            .set_timer(timer_event, TimerType::Periodic(NOTIFICATION_POLL_PERIOD))
            .inspect_err(|e| log::error!("Failed to arm MP notification timer: {e:?}"))?;

        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    use core::{
        alloc::Layout,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use patina::{
        component::hob::FromHob,
        component::service::memory::{MemoryError, MockMemoryManager, PageAllocation},
        component::service::perf_timer::MockArchTimerFunctionality,
        component::service::uefi_services::{
            event::{Event, EventError, MockEventServices},
            protocol::{Handle, MockProtocolServices, NotifyRegistration, ProtocolError, ProtocolPtr},
            timer_event::MockTimerEventServices,
        },
        protocol::ProtocolInterface,
    };
    use patina_internal_cpu::mp::{MockMpDispatcher, Processor};

    struct TestTimer;

    impl ArchTimerFunctionality for TestTimer {
        fn cpu_count(&self) -> u64 {
            0
        }

        fn perf_frequency(&self) -> u64 {
            1_000_000
        }
    }

    static TIMER: TestTimer = TestTimer;

    fn page_allocation(page_count: usize) -> PageAllocation {
        let layout = Layout::from_size_align(uefi_pages_to_size!(page_count), patina::UEFI_PAGE_SIZE)
            .expect("page allocation layout should be valid");
        // SAFETY: The layout is non-zero and page-aligned. The allocation is intentionally
        // leaked because production AP stacks also remain allocated for the boot lifetime.
        let allocation = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!allocation.is_null());
        let owner = Box::leak(Box::new(MockMemoryManager::new()));
        // SAFETY: `allocation` identifies `page_count` writable, page-aligned pages.
        unsafe { PageAllocation::new(allocation.expose_provenance(), page_count, owner) }
            .expect("test page allocation should be valid")
    }

    fn memory_service(memory: MockMemoryManager) -> Service<dyn MemoryManager> {
        Service::mock(Box::new(memory))
    }

    fn test_services(mp: MockMpDispatcher) -> &'static MpServices<MockMpDispatcher> {
        Box::leak(Box::new(MpServices::new(mp, Vec::new(), &TIMER)))
    }

    fn test_event() -> Event {
        Event::from_raw(core::ptr::dangling_mut()).expect("dangling pointer is non-null")
    }

    fn protocol_services_with_status_code(protocol: &'static StatusCodeProtocol) -> MockProtocolServices {
        let address = core::ptr::from_ref(protocol) as usize;
        let mut protocols = MockProtocolServices::new();
        protocols.expect_locate_interface().returning(move |guid| {
            assert_eq!(guid, StatusCodeProtocol::PROTOCOL_GUID);
            Ok(ProtocolPtr::from_raw(address as *mut core::ffi::c_void).expect("address is non-null"))
        });
        protocols
    }

    fn mp_handoff(processor_ids: &[u32]) -> MpHandOff {
        let mut bytes = Vec::with_capacity(8 + processor_ids.len() * 24);
        bytes.extend_from_slice(&0_u32.to_ne_bytes());
        bytes.extend_from_slice(&(processor_ids.len() as u32).to_ne_bytes());
        for processor_id in processor_ids {
            bytes.extend_from_slice(&processor_id.to_ne_bytes());
            bytes.extend_from_slice(&0_u32.to_ne_bytes());
            bytes.extend_from_slice(&0_u64.to_ne_bytes());
            bytes.extend_from_slice(&0_u64.to_ne_bytes());
        }
        <MpHandOff as FromHob>::parse(&bytes).unwrap()
    }

    fn mp_handoff_with_health(processors: &[(u32, u32)]) -> MpHandOff {
        let mut bytes = Vec::with_capacity(8 + processors.len() * 24);
        bytes.extend_from_slice(&0_u32.to_ne_bytes());
        bytes.extend_from_slice(&(processors.len() as u32).to_ne_bytes());
        for (processor_id, health) in processors {
            bytes.extend_from_slice(&processor_id.to_ne_bytes());
            bytes.extend_from_slice(&health.to_ne_bytes());
            bytes.extend_from_slice(&0_u64.to_ne_bytes());
            bytes.extend_from_slice(&0_u64.to_ne_bytes());
        }
        <MpHandOff as FromHob>::parse(&bytes).unwrap()
    }

    #[test]
    fn test_mp_services_component_reports_bist_errors() {
        static REPORT_COUNT: AtomicUsize = AtomicUsize::new(0);
        static REPORT_IS_VALID: AtomicBool = AtomicBool::new(true);

        extern "efiapi" fn report_status_code(
            status_code_type: u32,
            status_code_value: u32,
            instance: u32,
            caller_id: *const efi::Guid,
            data: *const patina::pi::protocol::status_code::EfiStatusCodeData,
        ) -> efi::Status {
            let caller_id_is_valid =
                // SAFETY: The production helper passes a valid caller-ID reference.
                unsafe { caller_id.as_ref() } == Some(patina::guid::DXE_CORE_ID.as_efi_guid());
            REPORT_IS_VALID.fetch_and(
                status_code_type == EFI_ERROR_CODE | EFI_ERROR_MAJOR
                    && status_code_value == EFI_COMPUTING_UNIT_HOST_PROCESSOR | EFI_CU_HP_EC_SELF_TEST
                    && instance == 0
                    && caller_id_is_valid
                    && data.is_null(),
                Ordering::Relaxed,
            );
            REPORT_COUNT.fetch_add(1, Ordering::Relaxed);
            efi::Status::SUCCESS
        }

        REPORT_COUNT.store(0, Ordering::Relaxed);
        REPORT_IS_VALID.store(true, Ordering::Relaxed);
        let protocol = Box::leak(Box::new(StatusCodeProtocol { report_status_code }));
        let protocols: Service<dyn ProtocolServices> =
            Service::mock(Box::new(protocol_services_with_status_code(protocol)));
        let hobs =
            MpHobs::parse_hobs(None, Some(Hob::mock(vec![mp_handoff_with_health(&[(0, 0), (1, 1), (2, 2)])])), None);

        assert!(MpServicesComponent::report_bist_errors(&protocols, hobs.bist_error_count()));

        assert_eq!(REPORT_COUNT.load(Ordering::Relaxed), 2);
        assert!(REPORT_IS_VALID.load(Ordering::Relaxed));
    }

    #[test]
    fn test_mp_services_component_defers_bist_errors_until_status_code_protocol_is_available() {
        let mut protocols = MockProtocolServices::new();
        protocols.expect_locate_interface().once().returning(|_| Err(ProtocolError::NotFound));
        protocols
            .expect_register_install_notify()
            .once()
            .withf(|guid, tpl, _| *guid == StatusCodeProtocol::PROTOCOL_GUID && *tpl == Tpl::Callback)
            .returning(|_, _, _| {
                Ok(NotifyRegistration::from_raw(
                    core::ptr::dangling_mut(),
                    core::ptr::dangling_mut(),
                    core::ptr::dangling_mut(),
                ))
            });
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(protocols));

        MpServicesComponent.report_or_defer_bist_errors(&protocols, 1);
    }

    #[test]
    fn test_mp_services_component_skips_bist_reporting_without_errors() {
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(MockProtocolServices::new()));

        MpServicesComponent.report_or_defer_bist_errors(&protocols, 0);
    }

    #[test]
    fn test_mp_services_component_reports_deferred_bist_errors() {
        static REPORT_COUNT: AtomicUsize = AtomicUsize::new(0);

        extern "efiapi" fn report_status_code(
            _status_code_type: u32,
            _status_code_value: u32,
            _instance: u32,
            _caller_id: *const efi::Guid,
            _data: *const patina::pi::protocol::status_code::EfiStatusCodeData,
        ) -> efi::Status {
            REPORT_COUNT.fetch_add(1, Ordering::Relaxed);
            efi::Status::SUCCESS
        }

        REPORT_COUNT.store(0, Ordering::Relaxed);
        let protocol: &'static StatusCodeProtocol = Box::leak(Box::new(StatusCodeProtocol { report_status_code }));
        let address = core::ptr::from_ref(protocol) as usize;
        let mut protocols = MockProtocolServices::new();
        protocols.expect_locate_interface().once().returning(|_| Err(ProtocolError::NotFound));
        protocols.expect_locate_interface().returning(move |_| {
            Ok(ProtocolPtr::from_raw(address as *mut core::ffi::c_void).expect("address is non-null"))
        });
        protocols.expect_register_install_notify().once().returning(|_, _, mut callback| {
            // Delivered twice to prove the report only happens once.
            let handle = Handle::from_raw(core::ptr::dangling_mut()).expect("dangling pointer is non-null");
            callback(handle);
            callback(handle);
            Ok(NotifyRegistration::from_raw(
                core::ptr::dangling_mut(),
                core::ptr::dangling_mut(),
                core::ptr::dangling_mut(),
            ))
        });
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(protocols));

        MpServicesComponent.report_or_defer_bist_errors(&protocols, 2);

        assert_eq!(REPORT_COUNT.load(Ordering::Relaxed), 2);
    }

    fn mock_services(
        protocols: MockProtocolServices,
        events: MockEventServices,
        timer_events: MockTimerEventServices,
    ) -> (Service<dyn ProtocolServices>, Service<dyn EventServices>, Service<dyn TimerEventServices>) {
        (Service::mock(Box::new(protocols)), Service::mock(Box::new(events)), Service::mock(Box::new(timer_events)))
    }

    #[test]
    fn test_mp_services_component_entry_point_propagates_stack_allocation_failure() {
        let mut memory = MockMemoryManager::new();
        memory.expect_allocate_pages().once().returning(|_, _| Err(MemoryError::NoAvailableMemory));
        let handoff = Hob::mock(vec![mp_handoff(&[0, 1])]);
        let config = Hob::mock(vec![MpHandOffConfig { wait_loop_execution_mode: 8, startup_signal_value: 1 }]);

        assert!(matches!(
            MpServicesComponent.entry_point(
                mock_services(MockProtocolServices::new(), MockEventServices::new(), MockTimerEventServices::new()),
                memory_service(memory),
                Service::mock(Box::new(MockArchTimerFunctionality::new())),
                (None, Some(handoff), Some(config)),
            ),
            Err(EfiError::OutOfResources)
        ));
    }

    #[test]
    fn test_mp_services_component_allocates_guarded_ap_contexts() {
        let expected_pages = uefi_size_to_pages!(ApContext::STACK_SIZE) + 1;
        let mut memory = MockMemoryManager::new();
        memory
            .expect_allocate_pages()
            .times(2)
            .withf(move |page_count, _| *page_count == expected_pages)
            .returning(|page_count, _| Ok(page_allocation(page_count)));
        memory
            .expect_set_page_attributes()
            .times(2)
            .withf(|address, page_count, access, caching| {
                address.is_multiple_of(patina::UEFI_PAGE_SIZE)
                    && *page_count == 1
                    && *access == AccessType::NoAccess
                    && caching.is_none()
            })
            .returning(|_, _, _, _| Ok(()));
        let memory = memory_service(memory);

        let contexts = MpServicesComponent.allocate_ap_contexts(&memory, 2).expect("AP contexts should be allocated");

        assert_eq!(contexts.len(), 2);
    }

    #[test]
    fn test_mp_services_component_allocates_no_stacks_for_bsp_only() {
        let memory = memory_service(MockMemoryManager::new());

        let contexts =
            MpServicesComponent.allocate_ap_contexts(&memory, 0).expect("BSP-only context allocation should succeed");

        assert!(contexts.is_empty());
    }

    #[test]
    fn test_mp_services_component_maps_stack_allocation_failure() {
        let mut memory = MockMemoryManager::new();
        memory.expect_allocate_pages().once().returning(|_, _| Err(MemoryError::NoAvailableMemory));
        let memory = memory_service(memory);

        assert!(matches!(MpServicesComponent.allocate_ap_contexts(&memory, 1), Err(EfiError::OutOfResources)));
    }

    #[test]
    fn test_mp_services_component_maps_guard_page_failure() {
        let mut memory = MockMemoryManager::new();
        memory.expect_allocate_pages().once().returning(|page_count, _| Ok(page_allocation(page_count)));
        memory.expect_set_page_attributes().once().returning(|_, _, _, _| Err(MemoryError::InternalError));
        let memory = memory_service(memory);

        assert!(matches!(MpServicesComponent.allocate_ap_contexts(&memory, 1), Err(EfiError::DeviceError)));
    }

    #[test]
    fn test_mp_services_component_registers_lifecycle_events() {
        let services = test_services(MockMpDispatcher::new());
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().times(3).returning(|_, _, _| Ok(test_event()));
        let mut timer_events = MockTimerEventServices::new();
        timer_events.expect_create_timer_event().once().returning(|tpl, _| {
            assert_eq!(tpl, Tpl::Notify);
            Ok(test_event())
        });
        timer_events
            .expect_set_timer()
            .once()
            .withf(|event, timer_type| {
                *event == test_event() && *timer_type == TimerType::Periodic(NOTIFICATION_POLL_PERIOD)
            })
            .return_const(Ok(()));
        let events: Service<dyn EventServices> = Service::mock(Box::new(events));
        let timer_events: Service<dyn TimerEventServices> = Service::mock(Box::new(timer_events));

        assert!(MpServicesComponent.register_events(&events, &timer_events, services).is_ok());
    }

    #[test]
    fn test_mp_services_component_parks_aps_when_exit_event_registration_fails() {
        let mut mp = MockMpDispatcher::new();
        mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
        mp.expect_park().once().return_const(());
        let services = test_services(mp);
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().once().returning(|_, _, _| Err(EventError::InvalidParameter));
        let events: Service<dyn EventServices> = Service::mock(Box::new(events));
        let timer_events: Service<dyn TimerEventServices> = Service::mock(Box::new(MockTimerEventServices::new()));

        assert!(matches!(
            MpServicesComponent.register_events(&events, &timer_events, services),
            Err(EfiError::InvalidParameter)
        ));
    }

    #[test]
    fn test_mp_services_component_callbacks_forward_to_service() {
        let mut mp = MockMpDispatcher::new();
        mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
        mp.expect_park().once().return_const(());
        mp.expect_sync_aps().once().return_const(true);
        let services = test_services(mp);
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().times(3).returning(|_, _, mut callback| {
            callback(test_event());
            Ok(test_event())
        });
        let mut timer_events = MockTimerEventServices::new();
        timer_events.expect_create_timer_event().once().returning(|_, mut callback| {
            callback(test_event());
            Ok(test_event())
        });
        timer_events.expect_set_timer().once().return_const(Ok(()));
        let events: Service<dyn EventServices> = Service::mock(Box::new(events));
        let timer_events: Service<dyn TimerEventServices> = Service::mock(Box::new(timer_events));

        assert!(MpServicesComponent.register_events(&events, &timer_events, services).is_ok());
    }
}
