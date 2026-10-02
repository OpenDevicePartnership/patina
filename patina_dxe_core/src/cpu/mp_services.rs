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
use core::num::NonZeroUsize;

use patina::{
    component::{
        component,
        hob::Hob,
        service::{
            Service,
            memory::{AccessType, AllocationOptions, MemoryManager},
            perf_timer::ArchTimerFunctionality,
        },
    },
    error::EfiError,
    pi::{
        protocol::status_code::{self, StatusCodeProtocol},
        status_code::{EFI_COMPUTING_UNIT_HOST_PROCESSOR, EFI_CU_HP_EC_SELF_TEST, EFI_ERROR_CODE, EFI_ERROR_MAJOR},
    },
    standard::efi,
    uefi::{
        boot_services::{BootServices, StandardBootServices, tpl::Tpl},
        event::{CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP_GUID, EventTimerType, EventType},
    },
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

/// Period of the non-blocking notification poll timer, in 100 ns units (100 us).
const NOTIFICATION_POLL_PERIOD: u64 = 1_000;

/// Event group signaled when cache (MTRR) attributes change, driving AP resync.
static CACHE_ATTRIBUTE_CHANGE_GUID: efi::Guid = CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP_GUID.into_inner();

/// This component installs the MP Services Protocol.
#[derive(Default)]
pub(crate) struct MpServicesComponent;

#[component]
impl MpServicesComponent {
    fn entry_point(
        self,
        bs: StandardBootServices,
        mm: Service<dyn MemoryManager>,
        timer: Service<dyn ArchTimerFunctionality>,
        mp_info: Option<Hob<MpInformation2>>,
        mp_handoff: Option<Hob<MpHandOff>>,
        mp_handoff_config: Option<Hob<MpHandOffConfig>>,
    ) -> Result<(), EfiError> {
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

        self.report_or_defer_bist_errors(bs.clone(), parsed_hobs.bist_error_count());

        // Build the rust service.
        let services: &'static MpServices = Box::leak(Box::new(MpServices::new(mp, parsed_hobs.processors, *timer)));

        self.register_events(&bs, services)?;
        let protocol = Box::leak(Box::new(MpProtocolWrapper::new(services, bs.clone())));
        bs.install_protocol_interface(None, protocol)
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

    fn report_or_defer_bist_errors<B: BootServices + 'static>(&self, bs: B, bist_error_count: usize) {
        if bist_error_count == 0 {
            return;
        }

        if Self::report_bist_errors(&bs, bist_error_count) {
            return;
        }

        let context: &'static BistErrorReportContext<B> =
            Box::leak(Box::new(BistErrorReportContext { bs, bist_error_count }));
        let event = match context.bs.create_event(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(report_bist_errors_on_status_code_available::<B>),
            context,
        ) {
            Ok(event) => event,
            Err(status) => {
                log::error!("MP Services: Failed to create status code protocol notification event: {status}");
                return;
            }
        };

        if let Err(status) = context.bs.register_protocol_notify(status_code::PROTOCOL_GUID.as_efi_guid(), event) {
            log::error!("MP Services: Failed to register status code protocol notification: {status}");
            if let Err(status) = context.bs.close_event(event) {
                log::error!("MP Services: Failed to close status code protocol notification event: {status}");
            }
        }
    }

    fn report_bist_errors<B: BootServices>(bs: &B, bist_error_count: usize) -> bool {
        // SAFETY: This helper creates the only status-code protocol reference in its scope.
        let Ok(status_code) = (unsafe { bs.locate_protocol::<StatusCodeProtocol>(None) }) else {
            return false;
        };

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

        true
    }

    fn register_events<B: BootServices, M: MpDispatcher>(
        &self,
        bs: &B,
        services: &'static MpServices<M>,
    ) -> Result<(), EfiError> {
        // Create park event for EBS so that APs are parked before the OS takes over.
        if let Err(e) = bs.create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(park_aps_on_exit),
            services,
            &efi::EVENT_GROUP_EXIT_BOOT_SERVICES,
        ) {
            log::error!("Failed to register MP AP-park event: {e:?}");
            services.park();
            return Err(e.into());
        }

        bs.create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(mark_mp_ready_to_boot),
            services,
            &efi::EVENT_GROUP_READY_TO_BOOT,
        )
        .inspect_err(|e| log::error!("Failed to register MP ReadyToBoot event: {e:?}"))?;

        // Create cache-attribute-change event AP synchronization.
        bs.create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(sync_mtrrs_on_cache_change),
            services,
            &CACHE_ATTRIBUTE_CHANGE_GUID,
        )
        .inspect_err(|e| log::error!("Failed to register MP MTRR sync event: {e:?}"))?;

        // Create a periodic timer to poll for non-blocking dispatches.
        let timer_event = bs
            .create_event(
                EventType::TIMER | EventType::NOTIFY_SIGNAL,
                Tpl::NOTIFY,
                Some(poll_mp_notifications),
                services,
            )
            .inspect_err(|e| log::error!("Failed to create MP notification timer event: {e:?}"))?;

        // Set a timer callback to poll for non-blocking dispatches.
        bs.set_timer(timer_event, EventTimerType::Periodic, NOTIFICATION_POLL_PERIOD)
            .inspect_err(|e| log::error!("Failed to arm MP notification timer: {e:?}"))?;

        Ok(())
    }
}

struct BistErrorReportContext<B: BootServices> {
    bs: B,
    bist_error_count: usize,
}

extern "efiapi" fn report_bist_errors_on_status_code_available<B: BootServices + 'static>(
    event: efi::Event,
    context: &'static BistErrorReportContext<B>,
) {
    if MpServicesComponent::report_bist_errors(&context.bs, context.bist_error_count)
        && let Err(status) = context.bs.close_event(event)
    {
        log::error!("MP Services: Failed to close status code protocol notification event: {status}");
    }
}

extern "efiapi" fn poll_mp_notifications<M: MpDispatcher>(_event: efi::Event, services: &'static MpServices<M>) {
    services.poll_notifications();
}

extern "efiapi" fn mark_mp_ready_to_boot<M: MpDispatcher>(_event: efi::Event, services: &'static MpServices<M>) {
    services.mark_ready_to_boot();
}
extern "efiapi" fn park_aps_on_exit<M: MpDispatcher>(_event: efi::Event, services: &'static MpServices<M>) {
    services.park();
    log::info!("MP Services: APs parked for ExitBootServices");
}

extern "efiapi" fn sync_mtrrs_on_cache_change<M: MpDispatcher>(_event: efi::Event, services: &'static MpServices<M>) {
    services.synchronize();
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
        uefi::boot_services::MockBootServices,
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

    fn test_event() -> efi::Event {
        core::ptr::dangling_mut()
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
        <MpHandOff as FromHob>::parse(&bytes)
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
        <MpHandOff as FromHob>::parse(&bytes)
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
        let protocol_ptr = core::ptr::from_mut(protocol);
        let mut boot_services = MockBootServices::new();
        boot_services
            .expect_locate_protocol()
            .once()
            // SAFETY: The leaked protocol remains valid for the test process lifetime.
            .returning_st(move |_| Ok(unsafe { &mut *protocol_ptr }));
        let hobs =
            MpHobs::parse_hobs(None, Some(Hob::mock(vec![mp_handoff_with_health(&[(0, 0), (1, 1), (2, 2)])])), None);

        assert!(MpServicesComponent::report_bist_errors(&boot_services, hobs.bist_error_count()));

        assert_eq!(REPORT_COUNT.load(Ordering::Relaxed), 2);
        assert!(REPORT_IS_VALID.load(Ordering::Relaxed));
    }

    #[test]
    fn test_mp_services_component_defers_bist_errors_until_status_code_protocol_is_available() {
        let mut boot_services = MockBootServices::new();
        boot_services.expect_locate_protocol::<StatusCodeProtocol>().once().returning(|_| Err(efi::Status::NOT_FOUND));
        boot_services
            .expect_create_event::<&'static BistErrorReportContext<MockBootServices>>()
            .once()
            .returning(|_, _, _, _| Ok(test_event()));
        boot_services
            .expect_register_protocol_notify()
            .once()
            .withf(|protocol, event| *protocol == *status_code::PROTOCOL_GUID.as_efi_guid() && *event == test_event())
            .returning(|_, _| Ok(core::ptr::NonNull::dangling()));

        MpServicesComponent.report_or_defer_bist_errors(boot_services, 1);
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
        let protocol = Box::leak(Box::new(StatusCodeProtocol { report_status_code }));
        let protocol_ptr = core::ptr::from_mut(protocol);
        let mut boot_services = MockBootServices::new();
        boot_services
            .expect_locate_protocol()
            .once()
            // SAFETY: The leaked protocol remains valid for the test process lifetime.
            .returning_st(move |_| Ok(unsafe { &mut *protocol_ptr }));
        boot_services.expect_close_event().once().withf(|event| *event == test_event()).return_const(Ok(()));
        let context = Box::leak(Box::new(BistErrorReportContext { bs: boot_services, bist_error_count: 2 }));

        report_bist_errors_on_status_code_available(test_event(), context);

        assert_eq!(REPORT_COUNT.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_mp_services_component_entry_point_propagates_stack_allocation_failure() {
        let mut memory = MockMemoryManager::new();
        memory.expect_allocate_pages().once().returning(|_, _| Err(MemoryError::NoAvailableMemory));
        let handoff = Hob::mock(vec![mp_handoff(&[0, 1])]);
        let config = Hob::mock(vec![MpHandOffConfig { wait_loop_execution_mode: 8, startup_signal_value: 1 }]);

        assert!(matches!(
            MpServicesComponent.entry_point(
                StandardBootServices::new_uninit(),
                memory_service(memory),
                Service::mock(Box::new(MockArchTimerFunctionality::new())),
                None,
                Some(handoff),
                Some(config),
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
        let mut boot_services = MockBootServices::new();
        boot_services
            .expect_create_event_ex::<&'static MpServices<MockMpDispatcher>>()
            .times(3)
            .returning(|_, _, _, _, _| Ok(test_event()));
        boot_services
            .expect_create_event::<&'static MpServices<MockMpDispatcher>>()
            .once()
            .returning(|_, _, _, _| Ok(test_event()));
        boot_services
            .expect_set_timer()
            .once()
            .withf(|event, timer_type, period| {
                *event == test_event()
                    && matches!(timer_type, EventTimerType::Periodic)
                    && *period == NOTIFICATION_POLL_PERIOD
            })
            .return_const(Ok(()));

        assert!(MpServicesComponent.register_events(&boot_services, services).is_ok());
    }

    #[test]
    fn test_mp_services_component_parks_aps_when_exit_event_registration_fails() {
        let mut mp = MockMpDispatcher::new();
        mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
        mp.expect_park().once().return_const(());
        let services = test_services(mp);
        let mut boot_services = MockBootServices::new();
        boot_services
            .expect_create_event_ex::<&'static MpServices<MockMpDispatcher>>()
            .once()
            .returning(|_, _, _, _, _| Err(efi::Status::DEVICE_ERROR));

        assert!(matches!(MpServicesComponent.register_events(&boot_services, services), Err(EfiError::DeviceError)));
    }

    #[test]
    fn test_mp_services_component_callbacks_forward_to_service() {
        let services = test_services(MockMpDispatcher::new());
        poll_mp_notifications(core::ptr::null_mut(), services);
        mark_mp_ready_to_boot(core::ptr::null_mut(), services);

        let mut mp = MockMpDispatcher::new();
        mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
        mp.expect_park().once().return_const(());
        park_aps_on_exit(core::ptr::null_mut(), test_services(mp));

        let mut mp = MockMpDispatcher::new();
        mp.expect_sync_aps().once().return_const(true);
        sync_mtrrs_on_cache_change(core::ptr::null_mut(), test_services(mp));
    }
}
