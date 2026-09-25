//! MP Services Protocol wrapper and backing instance.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::boxed::Box;
use core::ffi::c_void;

use patina::{
    bit,
    protocol::ProtocolInterface,
    standard::efi::{self, protocols::mp_services},
    uefi::{
        boot_services::{BootServices, StandardBootServices},
        memory::EfiMemoryType,
    },
};

use super::services::{DispatchCompletion, MpError, MpServices};
use patina_internal_cpu::mp::{ApWorkItem, MpDispatcher, MpSupport};

/// Bit set in the `ProcessorNumber` parameter of `GetProcessorInfo` to request extended topology information.
///
/// Defined by the UEFI PI specification but not yet provided by `r_efi`
const CPU_V2_EXTENDED_TOPOLOGY: usize = bit!(24);

// Redefinitions of the function since the r_efi types provide rust function pointers
// we cannot properly validate the null use-case.
type RawStartupAllAps = unsafe extern "efiapi" fn(
    *mut mp_services::Protocol,
    Option<mp_services::ApProcedure>,
    efi::Boolean,
    efi::Event,
    usize,
    *mut c_void,
    *mut *mut usize,
) -> efi::Status;

type RawStartupThisAp = unsafe extern "efiapi" fn(
    *mut mp_services::Protocol,
    Option<mp_services::ApProcedure>,
    usize,
    efi::Event,
    usize,
    *mut c_void,
    *mut efi::Boolean,
) -> efi::Status;

/// Protocol interface wrapper which embeds the [`mp_services::Protocol`] as its first
/// field so a protocol `this` pointer can be cast back to recover `self_ref`,
/// letting the `extern "efiapi"` functions reach [`MpServices`] without a global.
#[repr(C)]
pub(super) struct MpProtocolWrapper<M: MpDispatcher + 'static = MpSupport> {
    protocol: mp_services::Protocol,
    service: &'static MpServices<M>,
    /// Boot services used by the ABI layer to allocate the caller-freeable `FailedCpuList`.
    boot_services: StandardBootServices,
}

// `protocol` must be at offset 0 for the `this`-pointer cast to be valid.
const _: () = assert!(core::mem::offset_of!(MpProtocolWrapper, protocol) == 0);

// SAFETY: `protocol` is the first field of this `#[repr(C)]` wrapper, so a consumer
// reading the interface as EFI_MP_SERVICES_PROTOCOL sees the correct function table;
// the trailing fields are invisible across the ABI.
unsafe impl<M: MpDispatcher + 'static> ProtocolInterface for MpProtocolWrapper<M> {
    const PROTOCOL_GUID: patina::BinaryGuid = patina::BinaryGuid(mp_services::PROTOCOL_GUID);
}

// Rust implementation of the protocol wrapper.
impl<M: MpDispatcher + 'static> MpProtocolWrapper<M> {
    /// Builds a wrapper around `services` populated with the protocol's function table.
    pub(super) fn new(services: &'static MpServices<M>, boot_services: StandardBootServices) -> Self {
        // SAFETY: The raw shims have the firmware ABI and machine-level parameter
        // layout of the protocol callbacks.
        let startup_all_aps =
            unsafe { core::mem::transmute::<RawStartupAllAps, mp_services::StartupAllAps>(Self::startup_all_aps_raw) };
        // SAFETY: Same ABI/layout argument as `startup_all_aps` above.
        let startup_this_ap =
            unsafe { core::mem::transmute::<RawStartupThisAp, mp_services::StartupThisAp>(Self::startup_this_ap_raw) };

        Self {
            protocol: mp_services::Protocol {
                get_number_of_processors: Self::get_number_of_processors,
                get_processor_info: Self::get_processor_info,
                startup_all_aps,
                startup_this_ap,
                switch_bsp: Self::switch_bsp,
                enable_disable_ap: Self::enable_disable_ap,
                who_am_i: Self::who_am_i,
            },
            service: services,
            boot_services,
        }
    }

    /// Recovers the wrapper from a protocol `this` pointer.
    ///
    /// # Safety
    ///
    /// `this` must be null or point to the `protocol` field of a live
    /// [`MpProtocolWrapper`] installed by this component.
    unsafe fn wrapper<'a>(this: *mut mp_services::Protocol) -> Option<&'a MpProtocolWrapper<M>> {
        // SAFETY: Per the contract, `this` is null or the base of a live wrapper.
        unsafe { (this as *const MpProtocolWrapper<M>).as_ref() }
    }

    /// Recovers the backing [`MpServices`] from a protocol `this` pointer.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::wrapper`].
    unsafe fn services<'a>(this: *mut mp_services::Protocol) -> Option<&'a MpServices<M>> {
        // SAFETY: forwarded contract.
        unsafe { Self::wrapper(this) }.map(|w| w.service)
    }

    /// Allocates and writes the `FailedCpuList`. See [`build_failed_cpu_list`] for details.
    fn build_failed_cpu_list(&self, failed: &[usize], out: *mut *mut usize) {
        build_failed_cpu_list(&self.boot_services, failed, out);
    }

    /// Builds the deferred reply for a non-blocking dispatch.
    fn create_completion_callback(
        &self,
        wait_event: efi::Event,
        finished: *mut efi::Boolean,
        failed_cpu_list: *mut *mut usize,
    ) -> DispatchCompletion {
        let boot_services = self.boot_services.clone();
        Box::new(move |failed: &[usize]| {
            if !finished.is_null() {
                // SAFETY: the caller guaranteed a valid, writable `Finished` out-pointer.
                unsafe { finished.write(efi::Boolean::from(failed.is_empty())) };
            }
            if !failed_cpu_list.is_null() && !failed.is_empty() {
                build_failed_cpu_list(&boot_services, failed, failed_cpu_list);
            }
            if let Err(e) = boot_services.signal_event(wait_event) {
                log::error!("Failed to signal MP wait event: {e:?}");
            }
        })
    }
}

// Protocol function efiapi implementations.
impl<M: MpDispatcher + 'static> MpProtocolWrapper<M> {
    unsafe extern "efiapi" fn get_number_of_processors(
        this: *mut mp_services::Protocol,
        number_of_processors: *mut usize,
        number_of_enabled_processors: *mut usize,
    ) -> efi::Status {
        if number_of_processors.is_null() || number_of_enabled_processors.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // SAFETY: `this` is the protocol pointer this component installed.
        let Some(svc) = (unsafe { Self::services(this) }) else {
            return efi::Status::INVALID_PARAMETER;
        };

        let (total, enabled) = match svc.processor_count() {
            Ok(counts) => counts,
            Err(e) => return e.into(),
        };

        // SAFETY: Caller guarantees both pointers are valid and writable.
        unsafe {
            number_of_processors.write(total);
            number_of_enabled_processors.write(enabled);
        }

        efi::Status::SUCCESS
    }

    unsafe extern "efiapi" fn get_processor_info(
        this: *mut mp_services::Protocol,
        processor_index: usize,
        processor_info_buffer: *mut mp_services::ProcessorInformation,
    ) -> efi::Status {
        if processor_info_buffer.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // SAFETY: `this` is the protocol pointer this component installed.
        let Some(svc) = (unsafe { Self::services(this) }) else {
            return efi::Status::INVALID_PARAMETER;
        };

        // The high bit requests extended topology. The base index is the low bits.
        let index = processor_index & !CPU_V2_EXTENDED_TOPOLOGY;
        match svc.processor_info(index) {
            Ok(mut info) => {
                // Location2 is only defined when the caller asked for it.
                if processor_index & CPU_V2_EXTENDED_TOPOLOGY == 0 {
                    info.extended_information = mp_services::ExtendedProcessorInformation {
                        location2: mp_services::CpuPhysicalLocation2 {
                            package: 0,
                            module: 0,
                            tile: 0,
                            die: 0,
                            core: 0,
                            thread: 0,
                        },
                    };
                }
                // SAFETY: Caller guarantees the buffer is valid and writable.
                unsafe { processor_info_buffer.write(info) };
                efi::Status::SUCCESS
            }
            Err(e) => e.into(),
        }
    }

    unsafe extern "efiapi" fn startup_all_aps_raw(
        this: *mut mp_services::Protocol,
        procedure: Option<mp_services::ApProcedure>,
        single_thread: efi::Boolean,
        wait_event: efi::Event,
        timeout_in_microseconds: usize,
        procedure_argument: *mut c_void,
        failed_cpu_list: *mut *mut usize,
    ) -> efi::Status {
        // SAFETY: `this` is the protocol pointer this component installed.
        let Some(wrapper) = (unsafe { Self::wrapper(this) }) else {
            return efi::Status::INVALID_PARAMETER;
        };

        let Some(procedure) = procedure else {
            return efi::Status::INVALID_PARAMETER;
        };

        // A non-null WaitEvent selects non-blocking mode.
        let wait_event = (!wait_event.is_null()).then_some(wait_event);
        if !failed_cpu_list.is_null() {
            // SAFETY: Caller guarantees the out pointer is valid and writable.
            unsafe { failed_cpu_list.write(core::ptr::null_mut()) };
        }

        // SAFETY: the EFI MP Services Protocol requires the caller to pass a valid
        // `EFI_AP_PROCEDURE` and a matching argument.
        let work: ApWorkItem = unsafe { ApWorkItem::new_efi(procedure, procedure_argument) };
        let completion =
            wait_event.map(|event| wrapper.create_completion_callback(event, core::ptr::null_mut(), failed_cpu_list));

        match wrapper.service.startup_all_aps(work, single_thread.into(), timeout_in_microseconds, completion) {
            Ok(()) => efi::Status::SUCCESS,
            // Only blocking mode reports failures here; marshal the list for the caller.
            Err(MpError::Timeout(failed)) => {
                if !failed_cpu_list.is_null() {
                    wrapper.build_failed_cpu_list(&failed, failed_cpu_list);
                }
                efi::Status::TIMEOUT
            }
            Err(e) => e.into(),
        }
    }

    unsafe extern "efiapi" fn startup_this_ap_raw(
        this: *mut mp_services::Protocol,
        procedure: Option<mp_services::ApProcedure>,
        processor_index: usize,
        wait_event: efi::Event,
        timeout_in_microseconds: usize,
        procedure_argument: *mut c_void,
        finished: *mut efi::Boolean,
    ) -> efi::Status {
        // SAFETY: `this` is the protocol pointer this component installed.
        let Some(wrapper) = (unsafe { Self::wrapper(this) }) else {
            return efi::Status::INVALID_PARAMETER;
        };

        let Some(procedure) = procedure else {
            return efi::Status::INVALID_PARAMETER;
        };

        let wait_event = (!wait_event.is_null()).then_some(wait_event);
        if wait_event.is_some() && !finished.is_null() {
            // SAFETY: Caller guarantees the out pointer is valid and writable.
            unsafe { finished.write(efi::Boolean::FALSE) };
        }

        // SAFETY: the EFI MP Services Protocol requires the caller to pass a valid
        // `EFI_AP_PROCEDURE` and a matching argument; wrapping them here confines the
        // unsafety to this ABI boundary so the dispatch path can stay safe.
        let work = unsafe { ApWorkItem::new_efi(procedure, procedure_argument) };
        let completion =
            wait_event.map(|event| wrapper.create_completion_callback(event, finished, core::ptr::null_mut()));

        match wrapper.service.startup_this_ap(work, processor_index, timeout_in_microseconds, completion) {
            Ok(()) => efi::Status::SUCCESS,
            Err(e) => e.into(),
        }
    }

    unsafe extern "efiapi" fn switch_bsp(
        _this: *mut mp_services::Protocol,
        _processor_index: usize,
        _enable_old_bsp: efi::Boolean,
    ) -> efi::Status {
        log::warn!("switch_bsp is not supported");
        efi::Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn enable_disable_ap(
        this: *mut mp_services::Protocol,
        processor_index: usize,
        enable_ap: efi::Boolean,
        health_flag: *mut u32,
    ) -> efi::Status {
        // SAFETY: `this` is the protocol pointer this component installed.
        let Some(svc) = (unsafe { Self::services(this) }) else {
            return efi::Status::INVALID_PARAMETER;
        };

        // `HealthFlag` is an input: when supplied it carries the AP's new health.
        let healthy = if health_flag.is_null() {
            None
        } else {
            // SAFETY: Caller guarantees a non-null `HealthFlag` is readable.
            Some(unsafe { health_flag.read() } & mp_services::PROCESSOR_HEALTH_STATUS_BIT != 0)
        };

        match svc.enable_disable_ap(processor_index, enable_ap.into(), healthy) {
            Ok(()) => efi::Status::SUCCESS,
            Err(e) => e.into(),
        }
    }

    unsafe extern "efiapi" fn who_am_i(this: *mut mp_services::Protocol, processor_index: *mut usize) -> efi::Status {
        if processor_index.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // SAFETY: `this` is the protocol pointer this component installed.
        let Some(svc) = (unsafe { Self::services(this) }) else {
            return efi::Status::INVALID_PARAMETER;
        };

        match svc.who_am_i() {
            Some(index) => {
                // SAFETY: Caller guarantees processor_index is a valid pointer.
                unsafe { processor_index.write(index) };
                efi::Status::SUCCESS
            }
            // The calling processor is not one this core knows about.
            None => efi::Status::DEVICE_ERROR,
        }
    }
}

/// Allocates pool memory, writes the processor numbers in `failed` terminated by
/// [`mp_services::END_OF_CPU_LIST`], and stores the buffer pointer in `out`. The
/// buffer is caller-freeable pool memory.
fn build_failed_cpu_list(bs: &StandardBootServices, failed: &[usize], out: *mut *mut usize) {
    let mut list = failed.to_vec();
    list.push(mp_services::END_OF_CPU_LIST);
    let size = list.len() * core::mem::size_of::<usize>();
    match bs.allocate_pool(EfiMemoryType::BootServicesData, size) {
        Ok(ptr) => {
            let dst = ptr as *mut usize;
            for (i, &v) in list.iter().enumerate() {
                // SAFETY: `dst` was allocated with room for `list.len()`.
                unsafe { dst.add(i).write(v) };
            }
            // SAFETY: the caller verified `out` is non-null before calling.
            unsafe { out.write(dst) };
        }
        Err(e) => log::error!("Failed to allocate FailedCpuList: {e:?}"),
    }
}

#[cfg_attr(coverage, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use patina::component::service::perf_timer::ArchTimerFunctionality;
    use patina_internal_cpu::mp::MockMpDispatcher;

    type TestProtocolWrapper = MpProtocolWrapper<MockMpDispatcher>;

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

    fn test_wrapper(mp: MockMpDispatcher) -> TestProtocolWrapper {
        let service = Box::leak(Box::new(MpServices::new(mp, Vec::new(), &TIMER)));
        TestProtocolWrapper::new(service, StandardBootServices::new_uninit())
    }

    #[test]
    fn test_mp_protocol_rejects_invalid_required_pointers() {
        let mut total = 0;
        let mut enabled = 0;
        let mut processor_info = core::mem::MaybeUninit::<mp_services::ProcessorInformation>::uninit();
        let mut processor_index = 0;

        // SAFETY: Every call intentionally passes null before the implementation can dereference it.
        unsafe {
            assert_eq!(
                TestProtocolWrapper::get_number_of_processors(
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    &raw mut enabled,
                ),
                efi::Status::INVALID_PARAMETER
            );
            assert_eq!(
                TestProtocolWrapper::get_number_of_processors(
                    core::ptr::null_mut(),
                    &raw mut total,
                    core::ptr::null_mut(),
                ),
                efi::Status::INVALID_PARAMETER
            );
            assert_eq!(
                TestProtocolWrapper::get_processor_info(core::ptr::null_mut(), 0, core::ptr::null_mut()),
                efi::Status::INVALID_PARAMETER
            );
            assert_eq!(
                TestProtocolWrapper::get_processor_info(core::ptr::null_mut(), 0, processor_info.as_mut_ptr()),
                efi::Status::INVALID_PARAMETER
            );
            assert_eq!(
                TestProtocolWrapper::who_am_i(core::ptr::null_mut(), core::ptr::null_mut()),
                efi::Status::INVALID_PARAMETER
            );
            assert_eq!(
                TestProtocolWrapper::who_am_i(core::ptr::null_mut(), &raw mut processor_index),
                efi::Status::INVALID_PARAMETER
            );
        }
    }

    #[test]
    fn test_mp_protocol_rejects_null_wrapper_for_dispatch_and_enable() {
        // SAFETY: The null wrapper is rejected before any other argument is accessed.
        unsafe {
            assert_eq!(
                TestProtocolWrapper::startup_all_aps_raw(
                    core::ptr::null_mut(),
                    None,
                    efi::Boolean::FALSE,
                    core::ptr::null_mut(),
                    0,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                ),
                efi::Status::INVALID_PARAMETER
            );
            assert_eq!(
                TestProtocolWrapper::startup_this_ap_raw(
                    core::ptr::null_mut(),
                    None,
                    1,
                    core::ptr::null_mut(),
                    0,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                ),
                efi::Status::INVALID_PARAMETER
            );
            assert_eq!(
                TestProtocolWrapper::enable_disable_ap(
                    core::ptr::null_mut(),
                    1,
                    efi::Boolean::TRUE,
                    core::ptr::null_mut(),
                ),
                efi::Status::INVALID_PARAMETER
            );
        }
    }

    #[test]
    fn test_mp_protocol_rejects_null_procedures() {
        let mut wrapper = test_wrapper(MockMpDispatcher::new());
        let protocol = &raw mut wrapper.protocol;

        // SAFETY: `protocol` points to the live wrapper and the null procedures are
        // rejected before the remaining arguments are accessed.
        unsafe {
            assert_eq!(
                TestProtocolWrapper::startup_all_aps_raw(
                    protocol,
                    None,
                    efi::Boolean::FALSE,
                    core::ptr::null_mut(),
                    0,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                ),
                efi::Status::INVALID_PARAMETER
            );
            assert_eq!(
                TestProtocolWrapper::startup_this_ap_raw(
                    protocol,
                    None,
                    1,
                    core::ptr::null_mut(),
                    0,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                ),
                efi::Status::INVALID_PARAMETER
            );
        }
    }

    #[test]
    fn test_mp_protocol_writes_bsp_processor_information() {
        let mut mp = MockMpDispatcher::new();
        mp.expect_who_am_i().once().return_const(Some(patina_internal_cpu::mp::Processor::Bsp));
        mp.expect_bsp_processor_id().once().return_const(0x2Au32);
        let mut wrapper = test_wrapper(mp);
        let protocol = &raw mut wrapper.protocol;
        let mut info = core::mem::MaybeUninit::<mp_services::ProcessorInformation>::uninit();

        // SAFETY: `protocol` points to the live wrapper and `info` is writable.
        let status = unsafe { TestProtocolWrapper::get_processor_info(protocol, 0, info.as_mut_ptr()) };

        assert_eq!(status, efi::Status::SUCCESS);
        // SAFETY: a successful call initializes the output.
        let info = unsafe { info.assume_init() };
        assert_eq!(info.processor_id, 0x2A);
        assert_ne!(info.status_flag & mp_services::PROCESSOR_AS_BSP_BIT, 0);
        assert_ne!(info.status_flag & mp_services::PROCESSOR_ENABLED_BIT, 0);
    }

    #[test]
    fn test_mp_protocol_maps_service_errors_to_efi_status() {
        assert_eq!(efi::Status::from(MpError::InvalidProcessor), efi::Status::INVALID_PARAMETER);
        assert_eq!(efi::Status::from(MpError::NotFound), efi::Status::NOT_FOUND);
        assert_eq!(efi::Status::from(MpError::NotBsp), efi::Status::DEVICE_ERROR);
        assert_eq!(efi::Status::from(MpError::Busy), efi::Status::NOT_READY);
        assert_eq!(efi::Status::from(MpError::NotStarted), efi::Status::NOT_STARTED);
        assert_eq!(efi::Status::from(MpError::NotSupported), efi::Status::UNSUPPORTED);
        assert_eq!(efi::Status::from(MpError::Timeout(vec![1])), efi::Status::TIMEOUT);
    }
}
