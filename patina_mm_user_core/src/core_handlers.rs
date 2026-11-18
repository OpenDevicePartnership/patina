//! Core MMI Handlers
//!
//! MMI handlers the core registers for itself at startup.
//!
//! | GUID                                  | Handler                        | Action                                                                   |
//! | ------------------------------------- | ------------------------------ | ------------------------------------------------------------------------ |
//! | `MM_DISPATCH_EVENT_GROUP_GUID`        | `mm_driver_dispatch_handler`   | Runs components, then dispatches MM drivers (once)                       |
//! | `MM_DXE_READY_TO_LOCK_PROTOCOL_GUID`  | `mm_ready_to_lock_handler`     | Unregisters one-shot handlers, installs `MM_READY_TO_LOCK_PROTOCOL_GUID` |
//! | `MM_END_OF_PEI_PROTOCOL_GUID`         | `mm_end_of_pei_handler`        | Installs `MM_END_OF_PEI_PROTOCOL_GUID`                                   |
//! | `END_OF_DXE_EVENT_GROUP_GUID`         | `mm_end_of_dxe_handler`        | Installs `MM_END_OF_DXE_PROTOCOL_GUID`                                   |
//! | `EXIT_BOOT_SERVICES_EVENT_GROUP_GUID` | `mm_exit_boot_service_handler` | Installs the same GUID as a protocol (once; a failure is retried)        |
//! | `READY_TO_BOOT_EVENT_GROUP_GUID`      | `mm_ready_to_boot_handler`     | Installs the same GUID as a protocol (once; a failure is retried)        |
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    ffi::c_void,
    sync::atomic::{AtomicBool, Ordering},
};

use patina::standard::efi;
use spin::Mutex;

use crate::{MmUserCore, component_dispatcher::MmComponentInfo, mmi::InternalMmiHandler};
use patina::{
    BinaryGuid, Guid,
    management_mode::{
        event::MM_DISPATCH_EVENT_GROUP_GUID,
        guid::{
            MM_DXE_READY_TO_LOCK_PROTOCOL_GUID, MM_END_OF_DXE_PROTOCOL_GUID, MM_END_OF_PEI_PROTOCOL_GUID,
            MM_READY_TO_LOCK_PROTOCOL_GUID,
        },
        mm_services::MmServices,
    },
    pi::event::END_OF_DXE_EVENT_GROUP_GUID,
    uefi::event::{EXIT_BOOT_SERVICES_EVENT_GROUP_GUID, READY_TO_BOOT_EVENT_GROUP_GUID},
};

/// Number of entries in [`core_mmi_handlers`].
const CORE_MMI_HANDLER_COUNT: usize = 6;

/// Table of MMI handlers registered by the core.
///
/// Generic over the platform's [`MmComponentInfo`] so the driver dispatch handler can
/// dispatch the platform's components.
fn core_mmi_handlers<C: MmComponentInfo>() -> [CoreMmiHandler; CORE_MMI_HANDLER_COUNT] {
    [
        CoreMmiHandler {
            handler: mm_driver_dispatch_handler::<C>,
            handler_type: &MM_DISPATCH_EVENT_GROUP_GUID,
            unregister_on_lock: true,
        },
        CoreMmiHandler {
            handler: mm_ready_to_lock_handler::<C>,
            handler_type: &MM_DXE_READY_TO_LOCK_PROTOCOL_GUID,
            unregister_on_lock: true,
        },
        CoreMmiHandler {
            handler: mm_end_of_pei_handler,
            handler_type: &MM_END_OF_PEI_PROTOCOL_GUID,
            unregister_on_lock: true,
        },
        CoreMmiHandler {
            handler: mm_end_of_dxe_handler,
            handler_type: &END_OF_DXE_EVENT_GROUP_GUID,
            unregister_on_lock: false,
        },
        CoreMmiHandler {
            handler: mm_exit_boot_service_handler,
            handler_type: &EXIT_BOOT_SERVICES_EVENT_GROUP_GUID,
            unregister_on_lock: false,
        },
        CoreMmiHandler {
            handler: mm_ready_to_boot_handler,
            handler_type: &READY_TO_BOOT_EVENT_GROUP_GUID,
            unregister_on_lock: false,
        },
    ]
}

/// Dispatch handles returned by `register_internal_handler`, indexed like [`core_mmi_handlers`].
/// A slot is cleared once its handler is unregistered.
static DISPATCH_HANDLES: Mutex<[SendHandle; CORE_MMI_HANDLER_COUNT]> =
    Mutex::new([SendHandle::NULL; CORE_MMI_HANDLER_COUNT]);

/// A core MMI handler to register at startup.
struct CoreMmiHandler {
    /// The handler function.
    handler: InternalMmiHandler,
    /// The GUID that triggers this handler.
    handler_type: &'static BinaryGuid,
    /// Whether the ready-to-lock handler unregisters this handler.
    unregister_on_lock: bool,
}

/// Wraps `efi::Handle`, a `!Send` raw pointer, so it can be stored in a `static Mutex`.
#[derive(Clone, Copy)]
struct SendHandle(efi::Handle);
// SAFETY: The handles are only written and read on the BSP, during startup and inside core MMI
// handlers, so they are never shared across CPUs concurrently.
unsafe impl Send for SendHandle {}
// SAFETY: As above.
unsafe impl Sync for SendHandle {}

impl SendHandle {
    const NULL: Self = Self(core::ptr::null_mut());
}

/// Registers every core MMI handler with the MMI database.
pub(crate) fn register_core_mmi_handlers<C: MmComponentInfo>() {
    let mut handles = DISPATCH_HANDLES.lock();

    for (index, (slot, entry)) in handles.iter_mut().zip(core_mmi_handlers::<C>().iter()).enumerate() {
        match MmUserCore::instance().mmi_db.register_internal_handler(entry.handler, Some(entry.handler_type)) {
            Ok(handle) => {
                *slot = SendHandle(handle);
                log::debug!("Registered core MMI handler [{index}] for {}", entry.handler_type);
            }
            Err(status) => {
                log::error!(
                    "Failed to register core MMI handler [{index}] for {}: {:#x}",
                    entry.handler_type,
                    status.as_usize()
                );
            }
        }
    }
}

/// Installs `guid` with a null interface on a new handle.
fn install_lifecycle_protocol(guid: &efi::Guid) -> efi::Status {
    // SAFETY: A null interface is a marker; there is nothing for the database to keep valid.
    match unsafe { MmUserCore::instance().install_protocol_interface(None, guid, core::ptr::null_mut()) } {
        Ok(handle) => {
            log::info!("Installed lifecycle protocol {} on handle {:p}", Guid::from_ref(guid), handle);
            efi::Status::SUCCESS
        }
        Err(status) => status,
    }
}

/// Runs the platform's components, then dispatches the drivers discovered during
/// `StartUserCore`.
fn mm_driver_dispatch_handler<C: MmComponentInfo>(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    log::info!("MM driver dispatch event received");

    // One-shot: retire before running any component or driver code, so a nested dispatch
    // event cannot re-enter. Clearing the slot keeps ready-to-lock from unregistering it again.
    let core = MmUserCore::instance();
    let mut handles = DISPATCH_HANDLES.lock();
    if let Some(slot) = handles.first_mut()
        && !slot.0.is_null()
    {
        if let Err(status) = core.mmi_handler_unregister(slot.0) {
            log::error!("Failed to unregister MM dispatch handler: {:#x}", status.as_usize());
            return status;
        }
        *slot = SendHandle::NULL;
    }
    drop(handles);

    if let Err(status) = core.dispatch_components::<C>() {
        return status;
    }

    match core.dispatch_drivers() {
        Ok(count) => {
            log::info!("Successfully dispatched {count} MM driver(s).");
            efi::Status::SUCCESS
        }
        Err(status) => {
            log::error!("Driver dispatch failed: {:#x}", status.as_usize());
            status
        }
    }
}

/// Unregisters the handlers marked `unregister_on_lock` (including itself), then installs
/// `MM_READY_TO_LOCK_PROTOCOL_GUID` to notify MM drivers.
fn mm_ready_to_lock_handler<C: MmComponentInfo>(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    log::info!("MM Ready To Lock event received");

    // Clear each slot as its handler goes, so no handle is unregistered twice.
    let mut handles = DISPATCH_HANDLES.lock();
    for (slot, entry) in handles.iter_mut().zip(core_mmi_handlers::<C>().iter()) {
        if entry.unregister_on_lock && !slot.0.is_null() {
            let handle = core::mem::replace(slot, SendHandle::NULL).0;
            if let Err(status) = MmUserCore::instance().mmi_handler_unregister(handle) {
                log::error!("Failed to unregister MMI handler for {}: {:#x}", entry.handler_type, status.as_usize());
            }
        }
    }
    drop(handles);

    let status = install_lifecycle_protocol(&MM_READY_TO_LOCK_PROTOCOL_GUID);
    if status != efi::Status::SUCCESS {
        log::error!("Failed to install MM Ready To Lock Protocol: {:#x}", status.as_usize());
    }

    status
}

/// Installs `MM_END_OF_PEI_PROTOCOL_GUID`.
fn mm_end_of_pei_handler(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    log::info!("MM End of PEI event received");

    install_lifecycle_protocol(&MM_END_OF_PEI_PROTOCOL_GUID)
}

/// Installs `MM_END_OF_DXE_PROTOCOL_GUID`.
fn mm_end_of_dxe_handler(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    log::info!("MM End of DXE event received");

    install_lifecycle_protocol(&MM_END_OF_DXE_PROTOCOL_GUID)
}

/// Installs `EXIT_BOOT_SERVICES_EVENT_GROUP_GUID` as a protocol, once.
fn mm_exit_boot_service_handler(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    install_lifecycle_protocol_once(&INSTALLED, &EXIT_BOOT_SERVICES_EVENT_GROUP_GUID)
}

/// Installs `READY_TO_BOOT_EVENT_GROUP_GUID` as a protocol, once.
fn mm_ready_to_boot_handler(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    install_lifecycle_protocol_once(&INSTALLED, &READY_TO_BOOT_EVENT_GROUP_GUID)
}

/// Installs `guid` unless `installed` records an earlier success. A failed install leaves
/// `installed` clear, so the next event retries it.
fn install_lifecycle_protocol_once(installed: &AtomicBool, guid: &efi::Guid) -> efi::Status {
    // MMI handlers run one at a time on one core, so the check and the store cannot race.
    if installed.load(Ordering::Acquire) {
        return efi::Status::SUCCESS;
    }
    let status = install_lifecycle_protocol(guid);
    if status == efi::Status::SUCCESS {
        installed.store(true, Ordering::Release);
    }
    status
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    static CORE: MmUserCore = MmUserCore::new();

    /// Publishes the process-wide core instance that every handler reaches through.
    ///
    /// nextest runs each test in its own process, so the `Once` behind `set_instance` and the
    /// `DISPATCH_HANDLES` table both start empty for every test.
    fn init_core() -> &'static MmUserCore {
        assert!(CORE.set_instance(), "the core instance is set once per test process");
        MmUserCore::instance()
    }

    /// Returns the handles recorded by `register_core_mmi_handlers`.
    fn dispatch_handles() -> Vec<efi::Handle> {
        DISPATCH_HANDLES.lock().iter().map(|handle| handle.0).collect()
    }

    type Platform = crate::tests::BarePlatform;

    /// Reports whether a handler is still registered, by unregistering it.
    fn take_handler(core: &MmUserCore, handle: efi::Handle) -> bool {
        core.mmi_db.mmi_handler_unregister(handle).is_ok()
    }

    fn protocol_holders(core: &MmUserCore, guid: &efi::Guid) -> usize {
        core.protocol_db.locate_handle_by_protocol(guid).len()
    }

    #[test]
    fn test_register_core_mmi_handlers_registers_every_entry() {
        let core = init_core();

        register_core_mmi_handlers::<Platform>();

        let handles = dispatch_handles();
        assert_eq!(handles.len(), core_mmi_handlers::<Platform>().len(), "one slot per table entry");
        assert!(handles.iter().all(|handle| !handle.is_null()), "every handler got a handle: {handles:?}");
        // The handles are distinct, so the table did not overwrite a slot.
        for (index, handle) in handles.iter().enumerate() {
            assert!(!handles[..index].contains(handle), "duplicate handle at index {index}");
        }
        assert!(handles.iter().all(|&handle| take_handler(core, handle)), "every handle resolves in the database");
    }

    #[test]
    fn test_install_lifecycle_protocol_publishes_on_a_fresh_handle() {
        let core = init_core();

        assert_eq!(install_lifecycle_protocol(&MM_END_OF_PEI_PROTOCOL_GUID), efi::Status::SUCCESS);

        assert_eq!(protocol_holders(core, &MM_END_OF_PEI_PROTOCOL_GUID), 1);
        // The interface is the null marker the PI pattern installs.
        assert_eq!(core.protocol_db.locate_protocol(&MM_END_OF_PEI_PROTOCOL_GUID), Some(core::ptr::null_mut()));
    }

    #[test]
    fn test_end_of_pei_handler_installs_its_protocol() {
        let core = init_core();

        let status = mm_end_of_pei_handler(&MM_END_OF_PEI_PROTOCOL_GUID, core::ptr::null_mut(), core::ptr::null_mut());

        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(protocol_holders(core, &MM_END_OF_PEI_PROTOCOL_GUID), 1);
    }

    #[test]
    fn test_end_of_dxe_handler_installs_its_protocol() {
        let core = init_core();

        let status = mm_end_of_dxe_handler(&END_OF_DXE_EVENT_GROUP_GUID, core::ptr::null_mut(), core::ptr::null_mut());

        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(protocol_holders(core, &MM_END_OF_DXE_PROTOCOL_GUID), 1);
    }

    #[test]
    fn test_exit_boot_service_handler_installs_only_on_the_first_call() {
        let core = init_core();

        let first = mm_exit_boot_service_handler(
            &EXIT_BOOT_SERVICES_EVENT_GROUP_GUID,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        let second = mm_exit_boot_service_handler(
            &EXIT_BOOT_SERVICES_EVENT_GROUP_GUID,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );

        assert_eq!(first, efi::Status::SUCCESS);
        assert_eq!(second, efi::Status::SUCCESS);
        // A repeat event must not publish the protocol a second time on a new handle.
        assert_eq!(protocol_holders(core, &EXIT_BOOT_SERVICES_EVENT_GROUP_GUID), 1);
    }

    #[test]
    fn test_ready_to_boot_handler_installs_only_on_the_first_call() {
        let core = init_core();

        let first =
            mm_ready_to_boot_handler(&READY_TO_BOOT_EVENT_GROUP_GUID, core::ptr::null_mut(), core::ptr::null_mut());
        let second =
            mm_ready_to_boot_handler(&READY_TO_BOOT_EVENT_GROUP_GUID, core::ptr::null_mut(), core::ptr::null_mut());

        assert_eq!(first, efi::Status::SUCCESS);
        assert_eq!(second, efi::Status::SUCCESS);
        assert_eq!(protocol_holders(core, &READY_TO_BOOT_EVENT_GROUP_GUID), 1);
    }

    #[test]
    fn test_lifecycle_handlers_retry_a_failed_install() {
        type Handler = fn(&efi::Guid, *mut c_void, *mut usize) -> efi::Status;
        let core = init_core();
        let handlers: [(Handler, &efi::Guid); 2] = [
            (mm_exit_boot_service_handler, &EXIT_BOOT_SERVICES_EVENT_GROUP_GUID),
            (mm_ready_to_boot_handler, &READY_TO_BOOT_EVENT_GROUP_GUID),
        ];

        for (index, (handler, guid)) in handlers.into_iter().enumerate() {
            // With no handle id left, the install fails and nothing is published.
            core.protocol_db.set_next_id(core::num::NonZeroUsize::MAX);
            assert_eq!(handler(guid, core::ptr::null_mut(), core::ptr::null_mut()), efi::Status::OUT_OF_RESOURCES);
            assert_eq!(protocol_holders(core, guid), 0);

            // The next event installs it, and later events do not install it again. Each pass
            // resumes from its own unused id.
            let fresh = core::num::NonZeroUsize::new(0x1000 * (index + 1)).expect("non-zero");
            core.protocol_db.set_next_id(fresh);
            assert_eq!(handler(guid, core::ptr::null_mut(), core::ptr::null_mut()), efi::Status::SUCCESS);
            assert_eq!(handler(guid, core::ptr::null_mut(), core::ptr::null_mut()), efi::Status::SUCCESS);
            assert_eq!(protocol_holders(core, guid), 1);
        }
    }

    #[test]
    fn test_driver_dispatch_handler_is_one_shot() {
        let core = init_core();
        let hobs = crate::tests::HobListBuffer::new().build();
        assert_eq!(
            core.entry_point_worker::<Platform>(
                patina::management_mode::supervisor::UserCommandType::StartUserCore as u64,
                hobs.as_ptr() as u64,
                0,
            ),
            efi::Status::SUCCESS.as_usize() as u64
        );
        let dispatch_handle = dispatch_handles()[0];

        let status = mm_driver_dispatch_handler::<Platform>(
            &MM_DISPATCH_EVENT_GROUP_GUID,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );

        assert_eq!(status, efi::Status::SUCCESS);
        // No drivers were discovered, but the handler must still retire itself so a later
        // MM_DISPATCH_EVENT_GROUP_GUID does not re-enter the dispatcher.
        assert!(!take_handler(core, dispatch_handle), "the dispatch handler unregistered itself");
        // Its slot is cleared, so ready-to-lock has no stale handle to unregister again.
        assert!(dispatch_handles()[0].is_null());
    }

    #[test]
    fn test_ready_to_lock_after_dispatch_unregisters_each_handler_once() {
        let core = init_core();
        let hobs = crate::tests::HobListBuffer::new().build();
        core.entry_point_worker::<Platform>(
            patina::management_mode::supervisor::UserCommandType::StartUserCore as u64,
            hobs.as_ptr() as u64,
            0,
        );
        assert_eq!(
            mm_driver_dispatch_handler::<Platform>(
                &MM_DISPATCH_EVENT_GROUP_GUID,
                core::ptr::null_mut(),
                core::ptr::null_mut()
            ),
            efi::Status::SUCCESS
        );

        let status = mm_ready_to_lock_handler::<Platform>(
            &MM_DXE_READY_TO_LOCK_PROTOCOL_GUID,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );

        assert_eq!(status, efi::Status::SUCCESS);
        // Every one-shot slot is cleared, and the others still hold live handlers.
        for (entry, handle) in core_mmi_handlers::<Platform>().iter().zip(dispatch_handles()) {
            assert_eq!(handle.is_null(), entry.unregister_on_lock, "slot for {}", entry.handler_type);
        }
    }

    #[test]
    fn test_driver_dispatch_handler_rejects_a_missing_hob_list() {
        init_core();

        // `StartUserCore` has not recorded the HOB list or registered a dispatch handle.
        let status = mm_driver_dispatch_handler::<Platform>(
            &MM_DISPATCH_EVENT_GROUP_GUID,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );

        assert_eq!(status, efi::Status::NOT_READY);
    }

    #[test]
    fn test_driver_dispatch_handler_rejects_a_retired_handle() {
        let core = init_core();
        register_core_mmi_handlers::<Platform>();
        assert!(take_handler(core, dispatch_handles()[0]));

        let status = mm_driver_dispatch_handler::<Platform>(
            &MM_DISPATCH_EVENT_GROUP_GUID,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );

        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_ready_to_lock_handler_retires_only_the_one_shot_handlers() {
        let core = init_core();
        register_core_mmi_handlers::<Platform>();
        let handles = dispatch_handles();

        let status = mm_ready_to_lock_handler::<Platform>(
            &MM_DXE_READY_TO_LOCK_PROTOCOL_GUID,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );

        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(protocol_holders(core, &MM_READY_TO_LOCK_PROTOCOL_GUID), 1);

        // Handlers flagged `unregister_on_lock` are gone; the rest survive the lock so they can
        // still service end-of-DXE, exit-boot-services and ready-to-boot.
        for (entry, &handle) in core_mmi_handlers::<Platform>().iter().zip(handles.iter()) {
            assert_eq!(
                take_handler(core, handle),
                !entry.unregister_on_lock,
                "unexpected state for {} after ready-to-lock",
                entry.handler_type
            );
        }
    }

    #[test]
    fn test_ready_to_lock_handler_continues_past_a_handler_already_unregistered() {
        crate::test_support::init_test_logger();
        let core = init_core();
        register_core_mmi_handlers::<Platform>();
        let handles = dispatch_handles();
        // The end-of-PEI handler is gone before ready-to-lock reaches its slot.
        assert!(take_handler(core, handles[2]));

        let status = mm_ready_to_lock_handler::<Platform>(
            &MM_DXE_READY_TO_LOCK_PROTOCOL_GUID,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );

        // The failed unregister is only logged, so the protocol is still published and the
        // one-shot handlers listed before and after it are still retired.
        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(protocol_holders(core, &MM_READY_TO_LOCK_PROTOCOL_GUID), 1);
        for (entry, (&handle, slot)) in
            core_mmi_handlers::<Platform>().iter().zip(handles.iter().zip(dispatch_handles()))
        {
            assert_eq!(slot.is_null(), entry.unregister_on_lock, "slot for {}", entry.handler_type);
            if entry.unregister_on_lock {
                assert!(!take_handler(core, handle), "{} is no longer registered", entry.handler_type);
            }
        }
    }

    #[test]
    fn test_driver_dispatch_handler_reports_a_nested_driver_dispatch() {
        static NESTED_STATUS: AtomicUsize = AtomicUsize::new(0);

        /// Raises the dispatch event again from inside a driver entry point.
        extern "efiapi" fn redispatching_driver(_handle: efi::Handle, _table: *const c_void) -> efi::Status {
            let status = mm_driver_dispatch_handler::<Platform>(
                &MM_DISPATCH_EVENT_GROUP_GUID,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            );
            NESTED_STATUS.store(status.as_usize(), Ordering::SeqCst);
            efi::Status::SUCCESS
        }

        crate::test_support::init_test_logger();
        let core = init_core();
        let hobs = crate::tests::HobListBuffer::new().driver_at(redispatching_driver as *const () as u64).build();
        assert_eq!(
            core.entry_point_worker::<Platform>(
                patina::management_mode::supervisor::UserCommandType::StartUserCore as u64,
                hobs.as_ptr() as u64,
                0,
            ),
            efi::Status::SUCCESS.as_usize() as u64
        );

        let status = mm_driver_dispatch_handler::<Platform>(
            &MM_DISPATCH_EVENT_GROUP_GUID,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );

        // The outer pass completes, while the nested one is refused instead of dispatching the
        // same drivers a second time.
        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(NESTED_STATUS.load(Ordering::SeqCst), efi::Status::ALREADY_STARTED.as_usize());
    }
}
