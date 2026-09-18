//! DXE Core Patina Test MP Services Tests
//!
//! These tests validate that the MP Services Protocol brought all application
//! processors online during boot and can dispatch work to them.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use patina::standard::efi;
use patina::standard::efi::protocols::mp_services;
use patina::uefi::boot_services::{BootServices, StandardBootServices, tpl::Tpl};
use patina::uefi::event::{EventTimerType, EventType};
use patina_test::{patina_test, u_assert, u_assert_eq};

// Procedure dispatched to APs: increments the shared counter pointed to by `arg`.
extern "efiapi" fn increment_counter(arg: *mut c_void) {
    // SAFETY: The test passes the address of a live `AtomicUsize` as the argument.
    let counter = unsafe { &*(arg as *const AtomicUsize) };
    counter.fetch_add(1, Ordering::SeqCst);
}

// Procedure used to force the MP timeout and INIT-SIPI-SIPI recovery path.
extern "efiapi" fn never_returns(_arg: *mut c_void) {
    loop {
        core::hint::spin_loop();
    }
}

struct BspOnlyQueryProbe {
    protocol: *mut mp_services::Protocol,
    count_rejected: AtomicBool,
    info_rejected: AtomicBool,
}

extern "efiapi" fn call_bsp_only_queries(arg: *mut c_void) {
    // SAFETY: The blocking test passes a live `BspOnlyQueryProbe` as the argument.
    let probe = unsafe { &*(arg as *const BspOnlyQueryProbe) };
    let mut total = 0;
    let mut enabled = 0;
    // SAFETY: The protocol and output pointers remain valid until the blocking dispatch returns.
    let status =
        unsafe { ((*probe.protocol).get_number_of_processors)(probe.protocol, &raw mut total, &raw mut enabled) };
    probe.count_rejected.store(status == efi::Status::DEVICE_ERROR, Ordering::SeqCst);

    let mut info = core::mem::MaybeUninit::<mp_services::ProcessorInformation>::zeroed();
    // SAFETY: The protocol and output pointers remain valid until the blocking dispatch returns.
    let status = unsafe { ((*probe.protocol).get_processor_info)(probe.protocol, 0, info.as_mut_ptr()) };
    probe.info_rejected.store(status == efi::Status::DEVICE_ERROR, Ordering::SeqCst);
}

// No-op notify for the waitable event used by the non-blocking dispatch test.
unsafe extern "efiapi" fn noop_notify(_event: efi::Event, _context: *mut c_void) {}

fn locate_protocol(bs: &StandardBootServices) -> Result<Option<&'static mut mp_services::Protocol>, &'static str> {
    // SAFETY: A single reference to the protocol is taken and not aliased.
    match unsafe { bs.locate_protocol::<mp_services::Protocol>(None) } {
        Ok(protocol) => Ok(Some(protocol)),
        Err(efi::Status::NOT_FOUND) => Ok(None),
        Err(_) => Err("failed to locate MP Services Protocol"),
    }
}

// Verify that every logical processor reported by the MP Services Protocol has
// started (enabled count equals the total processor count).
#[patina_test]
fn mp_services_all_cores_started(bs: StandardBootServices) -> patina_test::error::Result {
    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;

    let mut total: usize = 0;
    let mut enabled: usize = 0;
    // SAFETY: `protocol_ptr` is a valid pointer to the located protocol and the out-params are valid for the call.
    let status = unsafe { (protocol.get_number_of_processors)(protocol_ptr, &raw mut total, &raw mut enabled) };
    u_assert_eq!(status, efi::Status::SUCCESS, "get_number_of_processors failed");

    u_assert!(total >= 2, "expected a multiprocessor system, found {total} processor(s)");
    u_assert_eq!(enabled, total, "not all processors started");

    Ok(())
}

// Verify that the test (running on the BSP) is reported as processor index 0.
#[patina_test]
fn mp_services_bsp_who_am_i(bs: StandardBootServices) -> patina_test::error::Result {
    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;

    let mut index: usize = usize::MAX;
    // SAFETY: `protocol_ptr` is a valid pointer to the located protocol and `index` is valid for the call.
    let status = unsafe { (protocol.who_am_i)(protocol_ptr, &raw mut index) };
    u_assert_eq!(status, efi::Status::SUCCESS, "who_am_i failed");
    u_assert_eq!(index, 0, "who_am_i did not return BSP index 0");

    Ok(())
}

#[patina_test]
fn mp_services_bsp_only_queries_reject_ap_callers(bs: StandardBootServices) -> patina_test::error::Result {
    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;
    let probe = BspOnlyQueryProbe {
        protocol: protocol_ptr,
        count_rejected: AtomicBool::new(false),
        info_rejected: AtomicBool::new(false),
    };

    // SAFETY: `protocol_ptr` is valid and this blocking call keeps `probe` alive
    // until the AP procedure returns.
    let status = unsafe {
        (protocol.startup_this_ap)(
            protocol_ptr,
            call_bsp_only_queries,
            1,
            core::ptr::null_mut(),
            1_000_000,
            (&raw const probe).cast_mut().cast(),
            core::ptr::null_mut(),
        )
    };
    u_assert_eq!(status, efi::Status::SUCCESS, "failed to dispatch BSP-only query test");
    u_assert!(probe.count_rejected.load(Ordering::SeqCst), "GetNumberOfProcessors accepted an AP caller");
    u_assert!(probe.info_rejected.load(Ordering::SeqCst), "GetProcessorInfo accepted an AP caller");

    Ok(())
}

// Verify startup_all_aps dispatches the procedure to every started AP exactly once.
#[patina_test]
fn mp_services_startup_all_aps_runs_on_every_ap(bs: StandardBootServices) -> patina_test::error::Result {
    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;

    let mut total: usize = 0;
    let mut enabled: usize = 0;
    // SAFETY: The provided pointers are correct and valid for the duration of the call.
    unsafe {
        (protocol.get_number_of_processors)(protocol_ptr, &raw mut total, &raw mut enabled);
    }
    let ap_count = enabled - 1; // exclude the BSP

    let counter = AtomicUsize::new(0);
    // SAFETY: The provided pointers are correct and valid for the duration of the call.
    let status = unsafe {
        (protocol.startup_all_aps)(
            protocol_ptr,
            increment_counter,
            efi::Boolean::from(false), // run APs concurrently
            core::ptr::null_mut(),     // blocking (no wait event)
            1_000_000,                 // 1 second timeout
            (&raw const counter).cast_mut().cast(),
            core::ptr::null_mut(), // no failed CPU list
        )
    };
    u_assert_eq!(status, efi::Status::SUCCESS, "startup_all_aps failed");
    u_assert_eq!(counter.load(Ordering::SeqCst), ap_count, "procedure did not run on every AP");

    Ok(())
}

// Verify startup_this_ap dispatches to a single AP and ignores Finished in blocking mode.
#[patina_test]
fn mp_services_startup_this_ap_runs_on_one_ap(bs: StandardBootServices) -> patina_test::error::Result {
    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;

    let counter = AtomicUsize::new(0);
    let mut finished = efi::Boolean::from(false);
    // SAFETY: `protocol_ptr` is valid, `counter` outlives the (blocking) call, and the out-params are valid.
    let status = unsafe {
        (protocol.startup_this_ap)(
            protocol_ptr,
            increment_counter,
            1, // first AP
            core::ptr::null_mut(),
            1_000_000,
            (&raw const counter).cast_mut().cast(),
            &raw mut finished,
        )
    };
    u_assert_eq!(status, efi::Status::SUCCESS, "startup_this_ap failed");
    u_assert!(!bool::from(finished), "blocking startup_this_ap modified the ignored Finished value");
    u_assert_eq!(counter.load(Ordering::SeqCst), 1, "expected exactly one AP to run the procedure");

    Ok(())
}

// Verify startup_this_ap rejects a request targeting the BSP.
#[patina_test]
fn mp_services_startup_this_ap_rejects_bsp(bs: StandardBootServices) -> patina_test::error::Result {
    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;

    let counter = AtomicUsize::new(0);
    // SAFETY: `protocol_ptr` is valid, `counter` outlives the (blocking) call, and the out-params are valid.
    let status = unsafe {
        (protocol.startup_this_ap)(
            protocol_ptr,
            increment_counter,
            0, // the BSP
            core::ptr::null_mut(),
            1_000_000,
            (&raw const counter).cast_mut().cast(),
            core::ptr::null_mut(),
        )
    };
    u_assert_eq!(status, efi::Status::INVALID_PARAMETER, "startup_this_ap should reject the BSP");
    u_assert_eq!(counter.load(Ordering::SeqCst), 0, "no procedure should have run");

    Ok(())
}

// Verify the raw firmware ABI rejects a null procedure before constructing a
// Rust function pointer.
#[patina_test]
fn mp_services_startup_this_ap_rejects_null_procedure(bs: StandardBootServices) -> patina_test::error::Result {
    type RawStartupThisAp = unsafe extern "efiapi" fn(
        *mut mp_services::Protocol,
        *mut c_void,
        usize,
        efi::Event,
        usize,
        *mut c_void,
        *mut efi::Boolean,
    ) -> efi::Status;

    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;
    // SAFETY: The protocol callback and raw test type have the same firmware ABI
    // and machine-level parameter layout.
    let startup =
        unsafe { core::mem::transmute::<mp_services::StartupThisAp, RawStartupThisAp>(protocol.startup_this_ap) };
    // SAFETY: This deliberately supplies a null procedure to verify ABI validation.
    let status = unsafe {
        startup(
            protocol_ptr,
            core::ptr::null_mut(),
            1,
            core::ptr::null_mut(),
            1_000_000,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    };
    u_assert_eq!(status, efi::Status::INVALID_PARAMETER, "null procedure should be rejected");

    Ok(())
}

#[patina_test]
fn mp_services_startup_this_ap_reports_missing_processor(bs: StandardBootServices) -> patina_test::error::Result {
    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;
    // SAFETY: The protocol pointer and procedure are valid; the processor index is
    // deliberately out of range.
    let status = unsafe {
        (protocol.startup_this_ap)(
            protocol_ptr,
            increment_counter,
            usize::MAX,
            core::ptr::null_mut(),
            1_000_000,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    };
    u_assert_eq!(status, efi::Status::NOT_FOUND, "missing processor should report NOT_FOUND");

    Ok(())
}

// Verify that timing out an executing AP terminates the procedure through the
// INIT-SIPI-SIPI recovery entry and leaves the same processor available for later work.
#[patina_test]
fn mp_services_timeout_recovers_ap(bs: StandardBootServices) -> patina_test::error::Result {
    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;

    // SAFETY: The protocol pointer and procedure are valid. The procedure is
    // intentionally non-returning so the finite timeout exercises recovery.
    let status = unsafe {
        (protocol.startup_this_ap)(
            protocol_ptr,
            never_returns,
            1,
            core::ptr::null_mut(),
            100_000,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    };
    u_assert_eq!(status, efi::Status::TIMEOUT, "non-returning AP procedure did not time out");

    let counter = AtomicUsize::new(0);
    // SAFETY: The blocking call keeps `counter` alive and targets the AP that
    // must have acknowledged recovery before the timeout above returned.
    let status = unsafe {
        (protocol.startup_this_ap)(
            protocol_ptr,
            increment_counter,
            1,
            core::ptr::null_mut(),
            1_000_000,
            (&raw const counter).cast_mut().cast(),
            core::ptr::null_mut(),
        )
    };
    u_assert_eq!(status, efi::Status::SUCCESS, "recovered AP did not accept another dispatch");
    u_assert_eq!(counter.load(Ordering::SeqCst), 1, "recovered AP did not execute the next procedure");

    let mut info = core::mem::MaybeUninit::<mp_services::ProcessorInformation>::zeroed();
    // SAFETY: The protocol pointer is valid and `info` is writable for the call.
    let status = unsafe { (protocol.get_processor_info)(protocol_ptr, 1, info.as_mut_ptr()) };
    u_assert_eq!(status, efi::Status::SUCCESS, "failed to query the recovered AP");
    // SAFETY: The successful protocol call initialized `info`.
    let info = unsafe { info.assume_init() };
    u_assert!(info.status_flag & mp_services::PROCESSOR_ENABLED_BIT != 0, "recovered AP was disabled");
    u_assert!(info.status_flag & mp_services::PROCESSOR_HEALTH_STATUS_BIT != 0, "recovered AP became unhealthy");

    Ok(())
}

// Verify get_processor_info reports the BSP (processor 0) with the BSP flag set.
#[patina_test]
fn mp_services_get_processor_info_bsp(bs: StandardBootServices) -> patina_test::error::Result {
    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;

    let mut info = core::mem::MaybeUninit::<mp_services::ProcessorInformation>::zeroed();
    // SAFETY: `protocol_ptr` is valid and `info` is valid for writes for the duration of the call.
    let status = unsafe { (protocol.get_processor_info)(protocol_ptr, 0, info.as_mut_ptr()) };
    u_assert_eq!(status, efi::Status::SUCCESS, "get_processor_info(0) failed");
    // SAFETY: The call succeeded, so the buffer is initialized.
    let info = unsafe { info.assume_init() };
    u_assert!(info.status_flag & mp_services::PROCESSOR_AS_BSP_BIT != 0, "processor 0 should be the BSP");
    u_assert!(info.status_flag & mp_services::PROCESSOR_ENABLED_BIT != 0, "BSP should be enabled");

    Ok(())
}

// Verify get_processor_info rejects an out-of-range processor number.
#[patina_test]
fn mp_services_get_processor_info_rejects_out_of_range(bs: StandardBootServices) -> patina_test::error::Result {
    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;

    let mut info = core::mem::MaybeUninit::<mp_services::ProcessorInformation>::zeroed();
    // SAFETY: `protocol_ptr` is valid and `info` is valid for writes for the duration of the call.
    let status = unsafe { (protocol.get_processor_info)(protocol_ptr, 9999, info.as_mut_ptr()) };
    u_assert_eq!(status, efi::Status::NOT_FOUND, "out-of-range processor should be rejected");

    Ok(())
}

// Verify enable_disable_ap removes an AP from dispatch and restores it, and that
// the change is reflected by get_number_of_processors and get_processor_info.
#[patina_test]
fn mp_services_enable_disable_ap(bs: StandardBootServices) -> patina_test::error::Result {
    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;

    let mut total = 0usize;
    let mut enabled_before = 0usize;
    // SAFETY: `protocol_ptr` and both out-pointers are valid for the call.
    unsafe { (protocol.get_number_of_processors)(protocol_ptr, &raw mut total, &raw mut enabled_before) };

    // SAFETY: `protocol_ptr` is valid; a null health flag leaves the health unchanged.
    let status = unsafe { (protocol.enable_disable_ap)(protocol_ptr, 1, efi::Boolean::FALSE, core::ptr::null_mut()) };
    u_assert_eq!(status, efi::Status::SUCCESS, "disabling processor 1 failed");

    // SAFETY: The protocol pointer and procedure are valid; processor 1 is
    // deliberately disabled above.
    let status = unsafe {
        (protocol.startup_this_ap)(
            protocol_ptr,
            increment_counter,
            1,
            core::ptr::null_mut(),
            1_000_000,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    };
    u_assert_eq!(status, efi::Status::INVALID_PARAMETER, "disabled processor should be rejected");

    let mut enabled_after = 0usize;
    // SAFETY: `protocol_ptr` and both out-pointers are valid for the call.
    unsafe { (protocol.get_number_of_processors)(protocol_ptr, &raw mut total, &raw mut enabled_after) };
    u_assert_eq!(enabled_after, enabled_before - 1, "disabled processor is still counted as enabled");

    let mut info = core::mem::MaybeUninit::<mp_services::ProcessorInformation>::zeroed();
    // SAFETY: `protocol_ptr` is valid and `info` is valid for writes for the duration of the call.
    let status = unsafe { (protocol.get_processor_info)(protocol_ptr, 1, info.as_mut_ptr()) };
    u_assert_eq!(status, efi::Status::SUCCESS, "get_processor_info(1) failed");
    // SAFETY: The call succeeded, so the buffer is initialized.
    let info = unsafe { info.assume_init() };
    u_assert!(info.status_flag & mp_services::PROCESSOR_ENABLED_BIT == 0, "disabled processor reports as enabled");

    // SAFETY: `protocol_ptr` is valid; a null health flag leaves the health unchanged.
    let status = unsafe { (protocol.enable_disable_ap)(protocol_ptr, 1, efi::Boolean::TRUE, core::ptr::null_mut()) };
    u_assert_eq!(status, efi::Status::SUCCESS, "re-enabling processor 1 failed");

    let mut enabled_restored = 0usize;
    // SAFETY: `protocol_ptr` and both out-pointers are valid for the call.
    unsafe { (protocol.get_number_of_processors)(protocol_ptr, &raw mut total, &raw mut enabled_restored) };
    u_assert_eq!(enabled_restored, enabled_before, "re-enabled processor was not restored");

    // The BSP cannot be enabled or disabled.
    // SAFETY: `protocol_ptr` is valid; a null health flag leaves the health unchanged.
    let status = unsafe { (protocol.enable_disable_ap)(protocol_ptr, 0, efi::Boolean::FALSE, core::ptr::null_mut()) };
    u_assert_eq!(status, efi::Status::INVALID_PARAMETER, "the BSP should not be disableable");

    Ok(())
}

// Verify every AP can complete the PI disable/enable lifecycle and accept work afterward.
#[patina_test]
fn mp_services_enable_disable_all_aps(bs: StandardBootServices) -> patina_test::error::Result {
    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;

    let mut total = 0usize;
    let mut enabled = 0usize;
    // SAFETY: `protocol_ptr` and both out-pointers are valid for the call.
    let status = unsafe { (protocol.get_number_of_processors)(protocol_ptr, &raw mut total, &raw mut enabled) };
    u_assert_eq!(status, efi::Status::SUCCESS, "failed to query processor counts");
    u_assert!(total >= 2, "expected at least one AP");

    for processor_index in 1..total {
        // SAFETY: Each processor index came from GetNumberOfProcessors; a null health flag preserves health.
        let status = unsafe {
            (protocol.enable_disable_ap)(protocol_ptr, processor_index, efi::Boolean::FALSE, core::ptr::null_mut())
        };
        u_assert_eq!(status, efi::Status::SUCCESS, "failed to disable an AP");
    }

    // SAFETY: `protocol_ptr` and both out-pointers are valid for the call.
    let status = unsafe { (protocol.get_number_of_processors)(protocol_ptr, &raw mut total, &raw mut enabled) };
    u_assert_eq!(status, efi::Status::SUCCESS, "failed to query disabled processor counts");
    u_assert_eq!(enabled, 1, "an AP remained enabled");

    for processor_index in 1..total {
        // SAFETY: Each processor index came from GetNumberOfProcessors; a null health flag preserves health.
        let status = unsafe {
            (protocol.enable_disable_ap)(protocol_ptr, processor_index, efi::Boolean::TRUE, core::ptr::null_mut())
        };
        u_assert_eq!(status, efi::Status::SUCCESS, "failed to enable an AP");
    }

    // SAFETY: `protocol_ptr` and both out-pointers are valid for the call.
    let status = unsafe { (protocol.get_number_of_processors)(protocol_ptr, &raw mut total, &raw mut enabled) };
    u_assert_eq!(status, efi::Status::SUCCESS, "failed to query restored processor counts");
    u_assert_eq!(enabled, total, "not all APs were re-enabled");

    let counter = AtomicUsize::new(0);
    // SAFETY: The blocking call keeps `counter` alive and all protocol arguments are valid.
    let status = unsafe {
        (protocol.startup_all_aps)(
            protocol_ptr,
            increment_counter,
            efi::Boolean::FALSE,
            core::ptr::null_mut(),
            1_000_000,
            (&raw const counter).cast_mut().cast(),
            core::ptr::null_mut(),
        )
    };
    u_assert_eq!(status, efi::Status::SUCCESS, "re-enabled APs did not accept dispatch");
    u_assert_eq!(counter.load(Ordering::SeqCst), total - 1, "work did not run on every re-enabled AP");

    Ok(())
}

// Verify startup_all_aps in non-blocking mode returns immediately, runs the
// procedure on every AP, and signals the supplied wait event once they finish.
// This exercises the dxe_core event-based notification path driven by the
// component's periodic timer callback.
#[patina_test]
fn mp_services_startup_all_aps_nonblocking_signals_event(bs: StandardBootServices) -> patina_test::error::Result {
    let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
    let protocol_ptr: *mut mp_services::Protocol = protocol;

    let mut total: usize = 0;
    let mut enabled: usize = 0;
    // SAFETY: The provided pointers are correct and valid for the duration of the call.
    unsafe {
        (protocol.get_number_of_processors)(protocol_ptr, &raw mut total, &raw mut enabled);
    }
    let ap_count = enabled - 1; // exclude the BSP

    // A waitable (non-signal) event the notification poll will signal on completion.
    let wait_event = bs
        .create_event(EventType::NOTIFY_WAIT, Tpl::CALLBACK, Some(noop_notify), core::ptr::null_mut::<c_void>())
        .map_err(|_| "create wait event")?;

    let counter = AtomicUsize::new(0);
    let mut failed_cpu_list = usize::MAX as *mut usize;
    // SAFETY: `counter` outlives the dispatch (the test blocks on the wait event
    // below) and the protocol pointer/out-params are valid for the call.
    let status = unsafe {
        (protocol.startup_all_aps)(
            protocol_ptr,
            increment_counter,
            efi::Boolean::from(false), // run APs concurrently
            wait_event,                // non-blocking: signaled when the APs finish
            1_000_000,                 // 1 second timeout
            (&raw const counter).cast_mut().cast(),
            &raw mut failed_cpu_list,
        )
    };
    u_assert_eq!(status, efi::Status::SUCCESS, "non-blocking startup_all_aps did not start");
    u_assert!(failed_cpu_list.is_null(), "FailedCpuList was not cleared before returning");

    // Manual patina tests run at TPL_CALLBACK, so `wait_for_event` (TPL_APPLICATION
    // only) cannot be used. Bound the wait by real time with a relative timer event
    // and spin on `check_event` (valid at TPL_CALLBACK); the TPL raise/restore and the
    // timer ISR drive the TPL_NOTIFY poll that signals the event once every AP finishes.
    const WAIT_TIMEOUT_100NS: u64 = 50_000_000; // 5 seconds
    let timeout_event = bs
        .create_event(EventType::TIMER, Tpl::CALLBACK, None, core::ptr::null_mut::<c_void>())
        .map_err(|_| "create timeout event")?;
    bs.set_timer(timeout_event, EventTimerType::Relative, WAIT_TIMEOUT_100NS).map_err(|_| "arm timeout event")?;

    let mut signaled = false;
    loop {
        if bs.check_event(wait_event).is_ok() {
            signaled = true;
            break;
        }
        if bs.check_event(timeout_event).is_ok() {
            break;
        }
        core::hint::spin_loop();
    }
    let _ = bs.close_event(timeout_event);
    u_assert!(signaled, "wait event was not signaled by the notification poll");
    u_assert_eq!(counter.load(Ordering::SeqCst), ap_count, "procedure did not run on every AP");
    u_assert!(failed_cpu_list.is_null(), "successful dispatch allocated a FailedCpuList");

    let _ = bs.close_event(wait_event);
    Ok(())
}

#[cfg(target_arch = "x86_64")]
mod x64 {
    use super::{
        AtomicUsize, Ordering, StandardBootServices, c_void, efi, locate_protocol, mp_services, patina_test, u_assert,
        u_assert_eq,
    };
    // Snapshot of the architectural MTRR MSRs, read on any processor and compared
    // across processors to confirm they were synchronized from the BSP.
    #[derive(PartialEq, Eq)]
    struct MtrrSnapshot {
        def_type: u64,
        fixed: [u64; 11],
        variable: [u64; MtrrSnapshot::MAX_VARIABLE * 2],
        variable_count: usize,
    }

    impl MtrrSnapshot {
        const MAX_VARIABLE: usize = 32;
        const IA32_MTRR_CAP: u32 = 0xFE;
        const IA32_MTRR_DEF_TYPE: u32 = 0x2FF;
        const IA32_MTRR_PHYSBASE0: u32 = 0x200;
        const FIXED_MSRS: [u32; 11] = [0x250, 0x258, 0x259, 0x268, 0x269, 0x26A, 0x26B, 0x26C, 0x26D, 0x26E, 0x26F];

        // Reads the calling processor's MTRR MSRs into a snapshot.
        fn read() -> Self {
            // SAFETY: MTRR and its capability/default-type MSRs are architectural on
            // any x86_64 processor; reads have no side effects.
            let cap = unsafe { patina::arch::x64::read_msr(Self::IA32_MTRR_CAP) };
            let fixed_supported = cap & (1 << 8) != 0;
            let variable_count = ((cap & 0xFF) as usize).min(Self::MAX_VARIABLE);

            // SAFETY: see above.
            let def_type = unsafe { patina::arch::x64::read_msr(Self::IA32_MTRR_DEF_TYPE) };

            let mut fixed = [0u64; 11];
            if fixed_supported {
                for (slot, &msr) in fixed.iter_mut().zip(Self::FIXED_MSRS.iter()) {
                    // SAFETY: see above.
                    *slot = unsafe { patina::arch::x64::read_msr(msr) };
                }
            }

            let mut variable = [0u64; Self::MAX_VARIABLE * 2];
            for (n, pair) in variable.chunks_mut(2).take(variable_count).enumerate() {
                let base_msr = Self::IA32_MTRR_PHYSBASE0 + (2 * n as u32);
                if let [base, mask] = pair {
                    // SAFETY: see above.
                    *base = unsafe { patina::arch::x64::read_msr(base_msr) };
                    // SAFETY: see above.
                    *mask = unsafe { patina::arch::x64::read_msr(base_msr + 1) };
                }
            }

            Self { def_type, fixed, variable, variable_count }
        }
    }

    // Shared context for the MTRR-synchronization check dispatched to the APs.
    struct MtrrCheckContext {
        expected: MtrrSnapshot,
        ran: AtomicUsize,
        mismatches: AtomicUsize,
    }

    // AP procedure: reads this processor's MTRRs and compares them to the BSP's.
    extern "efiapi" fn verify_mtrrs(arg: *mut c_void) {
        // SAFETY: The test passes the address of a live `MtrrCheckContext` that
        // outlives the (blocking) dispatch.
        let ctx = unsafe { &*(arg as *const MtrrCheckContext) };
        ctx.ran.fetch_add(1, Ordering::SeqCst);
        if MtrrSnapshot::read() != ctx.expected {
            ctx.mismatches.fetch_add(1, Ordering::SeqCst);
        }
    }
    // Verify the MTRR synchronization: every started AP must observe the same
    // architectural MTRR MSRs as the BSP (established at PEI handoff and on cache
    // attribute changes). Reads the MSRs directly, independent of the MTRR library
    // that performs the synchronization.
    #[patina_test]
    fn mp_services_mtrrs_synchronized_with_bsp(bs: StandardBootServices) -> patina_test::error::Result {
        let Some(protocol) = locate_protocol(&bs)? else { return Ok(()) };
        let protocol_ptr: *mut mp_services::Protocol = protocol;

        let mut total: usize = 0;
        let mut enabled: usize = 0;
        // SAFETY: The provided pointers are correct and valid for the duration of the call.
        unsafe {
            (protocol.get_number_of_processors)(protocol_ptr, &raw mut total, &raw mut enabled);
        }
        let ap_count = enabled - 1; // exclude the BSP
        u_assert!(ap_count >= 1, "expected at least one AP to verify against");

        let ctx = MtrrCheckContext {
            expected: MtrrSnapshot::read(),
            ran: AtomicUsize::new(0),
            mismatches: AtomicUsize::new(0),
        };
        // SAFETY: `ctx` outlives the blocking dispatch and the protocol pointer/out-params are valid.
        let status = unsafe {
            (protocol.startup_all_aps)(
                protocol_ptr,
                verify_mtrrs,
                efi::Boolean::from(false), // run APs concurrently
                core::ptr::null_mut(),     // blocking (no wait event)
                1_000_000,                 // 1 second timeout
                (&raw const ctx).cast_mut().cast(),
                core::ptr::null_mut(),
            )
        };
        u_assert_eq!(status, efi::Status::SUCCESS, "startup_all_aps failed");
        u_assert_eq!(ctx.ran.load(Ordering::SeqCst), ap_count, "verification did not run on every AP");
        u_assert_eq!(ctx.mismatches.load(Ordering::SeqCst), 0, "one or more APs have MTRRs that differ from the BSP");

        Ok(())
    }
}
