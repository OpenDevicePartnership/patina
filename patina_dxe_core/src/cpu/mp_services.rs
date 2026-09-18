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

    fn register_events(&self, bs: &StandardBootServices, services: &'static MpServices) -> Result<(), EfiError> {
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

extern "efiapi" fn poll_mp_notifications(_event: efi::Event, services: &'static MpServices) {
    services.poll_notifications();
}

extern "efiapi" fn mark_mp_ready_to_boot(_event: efi::Event, services: &'static MpServices) {
    services.mark_ready_to_boot();
}
extern "efiapi" fn park_aps_on_exit(_event: efi::Event, services: &'static MpServices) {
    services.park();
    log::info!("MP Services: APs parked for ExitBootServices");
}

extern "efiapi" fn sync_mtrrs_on_cache_change(_event: efi::Event, services: &'static MpServices) {
    services.synchronize();
}
