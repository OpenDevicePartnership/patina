//! MM Core Internal MMI Handlers
//!
//! These are the MMI handlers registered by the MM Core itself to handle
//! lifecycle events forwarded from the DXE phase. They mirror the C
//! `mMmCoreMmiHandlers[]` table in `StandaloneMmCore.c`.
//!
//! Each handler is registered with [`MmiDatabase::register_internal_handler`](crate::mmi::MmiDatabase::register_internal_handler)
//! during startup and dispatched when the supervisor forwards the corresponding
//! GUID-tagged MMI through the communication buffer.
//!
//! ## Lifecycle Events
//!
//! | GUID | Handler | Description |
//! |------|---------|-------------|
//! | `MM_DISPATCH_EVENT` | `mm_driver_dispatch_handler` | Dispatches discovered MM drivers |
//! | `MM_DXE_READY_TO_LOCK_PROTOCOL` | `mm_ready_to_lock_handler` | Unregisters one-shot handlers, installs lock protocol |
//! | `MM_END_OF_PEI_PROTOCOL` | `mm_end_of_pei_handler` | Installs end-of-PEI protocol |
//! | `EVENT_GROUP_END_OF_DXE` | `mm_end_of_dxe_handler` | Installs end-of-DXE protocol |
//! | `EVENT_EXIT_BOOT_SERVICES` | `mm_exit_boot_service_handler` | Installs exit-boot-services protocol |
//! | `EVENT_READY_TO_BOOT` | `mm_ready_to_boot_handler` | Installs ready-to-boot protocol |
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;

use patina::standard::efi;
use spin::Mutex;

use crate::{MmUserCore, mmi::InternalMmiHandler};
use patina::{BinaryGuid, Guid, management_mode::mm_services::MmServices};

/// Table of MMI handlers registered by the MM Core, mirroring the C `mMmCoreMmiHandlers[]`.
static CORE_MMI_HANDLERS: &[CoreMmiHandler] = &[
    CoreMmiHandler {
        handler: mm_driver_dispatch_handler,
        handler_type: &patina::guid::MM_DISPATCH_EVENT,
        unregister_on_lock: true,
    },
    CoreMmiHandler {
        handler: mm_ready_to_lock_handler,
        handler_type: &patina::guid::MM_DXE_READY_TO_LOCK_PROTOCOL,
        unregister_on_lock: true,
    },
    CoreMmiHandler {
        handler: mm_end_of_pei_handler,
        handler_type: &patina::guid::MM_END_OF_PEI_PROTOCOL,
        unregister_on_lock: true,
    },
    CoreMmiHandler {
        handler: mm_end_of_dxe_handler,
        handler_type: &patina::guid::EVENT_GROUP_END_OF_DXE,
        unregister_on_lock: false,
    },
    CoreMmiHandler {
        handler: mm_exit_boot_service_handler,
        handler_type: &patina::guid::EVENT_EXIT_BOOT_SERVICES,
        unregister_on_lock: false,
    },
    CoreMmiHandler {
        handler: mm_ready_to_boot_handler,
        handler_type: &patina::guid::EVENT_READY_TO_BOOT,
        unregister_on_lock: false,
    },
];

/// Dispatch handles returned from `register_internal_handler` for each core handler.
///
/// Index matches the `CORE_MMI_HANDLERS` table. Populated by [`register_core_mmi_handlers`].
static DISPATCH_HANDLES: Mutex<[SendHandle; 6]> = Mutex::new([SendHandle::NULL; 6]);

/// Description of a core MMI handler to be registered at startup.
struct CoreMmiHandler {
    /// The handler function (native Rust signature).
    handler: InternalMmiHandler,
    /// The GUID that triggers this handler.
    handler_type: &'static BinaryGuid,
    /// Whether this handler should be unregistered during ready-to-lock.
    unregister_on_lock: bool,
}

/// Newtype wrapper around `efi::Handle` so it can be stored in a `static Mutex`.
///
/// `efi::Handle` is `*mut c_void` which is `!Send`.  The dispatch handles are only
/// written by the BSP during single-threaded init and read during the ready-to-lock
/// handler (also on the BSP), so it is safe to share them.
#[derive(Clone, Copy)]
struct SendHandle(efi::Handle);
// SAFETY: The handles are only written by the BSP during single-threaded init and read back on
// the BSP during the ready-to-lock handler, so they are never shared across CPUs concurrently.
unsafe impl Send for SendHandle {}
// SAFETY: As above.
unsafe impl Sync for SendHandle {}

impl SendHandle {
    const NULL: Self = Self(core::ptr::null_mut());
}

/// Register all core MMI handlers with the global MMI database.
///
/// Registration installs the `MM_DISPATCH_EVENT` handler that performs the
/// deferred driver dispatch, so this runs before any drivers are dispatched.
pub fn register_core_mmi_handlers() {
    let mut handles = DISPATCH_HANDLES.lock();

    for (i, (slot, entry)) in handles.iter_mut().zip(CORE_MMI_HANDLERS.iter()).enumerate() {
        match MmUserCore::instance().mmi_db.register_internal_handler(entry.handler, Some(entry.handler_type)) {
            Ok(handle) => {
                *slot = SendHandle(handle);
                log::info!("Registered core MMI handler [{i}] for {}", entry.handler_type);
            }
            Err(status) => {
                log::error!(
                    "Failed to register core MMI handler [{i}] for {}: {:#x}",
                    entry.handler_type,
                    status.as_usize()
                );
            }
        }
    }
}

/// Install a protocol with a NULL interface on a new handle.
///
/// This mirrors the C pattern used in lifecycle handlers:
/// ```c
/// MmHandle = NULL;
/// Status = MmInstallProtocolInterface(&MmHandle, &guid, EFI_NATIVE_INTERFACE, NULL);
/// ```
fn install_lifecycle_protocol(guid: &efi::Guid) -> efi::Status {
    // SAFETY: installs a marker protocol (null interface) on a freshly allocated handle.
    match unsafe { MmUserCore::instance().install_protocol_interface(None, guid, core::ptr::null_mut()) } {
        Ok(handle) => {
            log::info!("Installed lifecycle protocol {} on handle {:p}", Guid::from_ref(guid), handle);
            efi::Status::SUCCESS
        }
        Err(status) => status,
    }
}

/// MM Driver Dispatch Handler.
///
/// Re-triggers driver dispatch for any previously discovered but not-yet-dispatched
/// drivers. Once dispatch completes, the handler unregisters itself (it is a
/// one-shot handler).
///
/// Corresponds to the C `MmDriverDispatchHandler`.
fn mm_driver_dispatch_handler(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    log::info!("MmDriverDispatchHandler");

    // Dispatch the MM drivers discovered during StartUserCore (single dependency-ordered pass).
    match MmUserCore::instance().dispatch_drivers() {
        Ok(count) => log::info!("Successfully dispatched {count} MM driver(s)."),
        Err(status) => log::error!("Driver dispatch failed: {:#x}", status.as_usize()),
    }

    // Self-unregister (one-shot).
    let handles = DISPATCH_HANDLES.lock();
    let dispatch_handle = handles.first().map_or(core::ptr::null_mut(), |handle| handle.0);
    drop(handles);

    if !dispatch_handle.is_null() {
        // SAFETY: unregistering by handle is safe even if the handle is already gone.
        let _ = unsafe { MmUserCore::instance().mmi_handler_unregister(dispatch_handle) };
    }

    log::info!("MmDriverDispatchHandler done");

    efi::Status::SUCCESS
}

/// MM Ready To Lock Handler.
///
/// Called when `gEfiDxeMmReadyToLockProtocolGuid` MMI is received. This:
/// 1. Unregisters handlers marked with `unregister_on_lock` (including itself).
/// 2. Installs the `gEfiMmReadyToLockProtocolGuid` protocol to notify MM drivers.
///
/// Corresponds to the C `MmReadyToLockHandler`.
fn mm_ready_to_lock_handler(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    log::info!("MmReadyToLockHandler");

    // Unregister handlers that are no longer needed after MM lock.
    let handles = DISPATCH_HANDLES.lock();
    for (handle, entry) in handles.iter().zip(CORE_MMI_HANDLERS.iter()) {
        if entry.unregister_on_lock && !handle.0.is_null() {
            // SAFETY: unregistering by handle is safe even if the handle is already gone.
            let _ = unsafe { MmUserCore::instance().mmi_handler_unregister(handle.0) };
        }
    }
    drop(handles);

    // Install the MM Ready To Lock Protocol.
    let status = install_lifecycle_protocol(&patina::guid::MM_READY_TO_LOCK_PROTOCOL);
    if status != efi::Status::SUCCESS {
        log::error!("Failed to install MM Ready To Lock Protocol: {:#x}", status.as_usize());
    }

    status
}

/// MM End of PEI Handler.
///
/// Installs the `gEfiMmEndOfPeiProtocol` protocol.
///
/// Corresponds to the C `MmEndOfPeiHandler`.
fn mm_end_of_pei_handler(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    log::info!("MmEndOfPeiHandler");

    install_lifecycle_protocol(&patina::guid::MM_END_OF_PEI_PROTOCOL)
}

/// MM End of DXE Handler.
///
/// Installs the `gEfiMmEndOfDxeProtocolGuid` protocol.
///
/// Corresponds to the C `MmEndOfDxeHandler`.
fn mm_end_of_dxe_handler(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    log::info!("MmEndOfDxeHandler");

    install_lifecycle_protocol(&patina::guid::MM_END_OF_DXE_PROTOCOL)
}

/// MM Exit Boot Service Handler.
///
/// Installs the `gEfiEventExitBootServicesGuid` protocol (once).
///
/// Corresponds to the C `MmExitBootServiceHandler`.
fn mm_exit_boot_service_handler(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    static FIRED: spin::Once<()> = spin::Once::new();
    let mut status = efi::Status::SUCCESS;

    FIRED.call_once(|| {
        status = install_lifecycle_protocol(&patina::guid::EVENT_EXIT_BOOT_SERVICES);
    });

    status
}

/// MM Ready To Boot Handler.
///
/// Installs the `gEfiEventReadyToBootGuid` protocol (once).
///
/// Corresponds to the C `MmReadyToBootHandler`.
fn mm_ready_to_boot_handler(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    static FIRED: spin::Once<()> = spin::Once::new();
    let mut status = efi::Status::SUCCESS;

    FIRED.call_once(|| {
        status = install_lifecycle_protocol(&patina::guid::EVENT_READY_TO_BOOT);
    });

    status
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

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

        register_core_mmi_handlers();

        let handles = dispatch_handles();
        assert_eq!(handles.len(), CORE_MMI_HANDLERS.len(), "one slot per table entry");
        assert!(handles.iter().all(|handle| !handle.is_null()), "every handler got a handle: {handles:?}");
        // The handles are distinct, so the table did not overwrite a slot.
        for (i, handle) in handles.iter().enumerate() {
            assert!(!handles[..i].contains(handle), "duplicate handle at index {i}");
        }
        assert!(handles.iter().all(|&handle| take_handler(core, handle)), "every handle resolves in the database");
    }

    #[test]
    fn test_install_lifecycle_protocol_publishes_on_a_fresh_handle() {
        let core = init_core();

        assert_eq!(install_lifecycle_protocol(&patina::guid::MM_END_OF_PEI_PROTOCOL), efi::Status::SUCCESS);

        assert_eq!(protocol_holders(core, &patina::guid::MM_END_OF_PEI_PROTOCOL), 1);
        // The interface is the null marker the PI pattern installs.
        assert_eq!(
            core.protocol_db.locate_protocol(&patina::guid::MM_END_OF_PEI_PROTOCOL),
            Some(core::ptr::null_mut())
        );
    }

    #[test]
    fn test_end_of_pei_handler_installs_its_protocol() {
        let core = init_core();

        let status =
            mm_end_of_pei_handler(&patina::guid::MM_END_OF_PEI_PROTOCOL, core::ptr::null_mut(), core::ptr::null_mut());

        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(protocol_holders(core, &patina::guid::MM_END_OF_PEI_PROTOCOL), 1);
    }

    #[test]
    fn test_end_of_dxe_handler_installs_its_protocol() {
        let core = init_core();

        let status =
            mm_end_of_dxe_handler(&patina::guid::EVENT_GROUP_END_OF_DXE, core::ptr::null_mut(), core::ptr::null_mut());

        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(protocol_holders(core, &patina::guid::MM_END_OF_DXE_PROTOCOL), 1);
    }

    #[test]
    fn test_exit_boot_service_handler_installs_only_on_the_first_call() {
        let core = init_core();

        let first = mm_exit_boot_service_handler(
            &patina::guid::EVENT_EXIT_BOOT_SERVICES,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        let second = mm_exit_boot_service_handler(
            &patina::guid::EVENT_EXIT_BOOT_SERVICES,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );

        assert_eq!(first, efi::Status::SUCCESS);
        assert_eq!(second, efi::Status::SUCCESS);
        // A repeat event must not publish the protocol a second time on a new handle.
        assert_eq!(protocol_holders(core, &patina::guid::EVENT_EXIT_BOOT_SERVICES), 1);
    }

    #[test]
    fn test_ready_to_boot_handler_installs_only_on_the_first_call() {
        let core = init_core();

        let first =
            mm_ready_to_boot_handler(&patina::guid::EVENT_READY_TO_BOOT, core::ptr::null_mut(), core::ptr::null_mut());
        let second =
            mm_ready_to_boot_handler(&patina::guid::EVENT_READY_TO_BOOT, core::ptr::null_mut(), core::ptr::null_mut());

        assert_eq!(first, efi::Status::SUCCESS);
        assert_eq!(second, efi::Status::SUCCESS);
        assert_eq!(protocol_holders(core, &patina::guid::EVENT_READY_TO_BOOT), 1);
    }

    #[test]
    fn test_driver_dispatch_handler_is_one_shot() {
        let core = init_core();
        register_core_mmi_handlers();
        let dispatch_handle = dispatch_handles()[0];

        let status =
            mm_driver_dispatch_handler(&patina::guid::MM_DISPATCH_EVENT, core::ptr::null_mut(), core::ptr::null_mut());

        assert_eq!(status, efi::Status::SUCCESS);
        // No drivers were discovered, but the handler must still retire itself so a later
        // MM_DISPATCH_EVENT does not re-enter the dispatcher.
        assert!(!take_handler(core, dispatch_handle), "the dispatch handler unregistered itself");
    }

    #[test]
    fn test_driver_dispatch_handler_tolerates_an_unregistered_handle() {
        init_core();

        // `register_core_mmi_handlers` was never called, so the handle slot is still null.
        let status =
            mm_driver_dispatch_handler(&patina::guid::MM_DISPATCH_EVENT, core::ptr::null_mut(), core::ptr::null_mut());

        assert_eq!(status, efi::Status::SUCCESS);
    }

    #[test]
    fn test_ready_to_lock_handler_retires_only_the_one_shot_handlers() {
        let core = init_core();
        register_core_mmi_handlers();
        let handles = dispatch_handles();

        let status = mm_ready_to_lock_handler(
            &patina::guid::MM_DXE_READY_TO_LOCK_PROTOCOL,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );

        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(protocol_holders(core, &patina::guid::MM_READY_TO_LOCK_PROTOCOL), 1);

        // Handlers flagged `unregister_on_lock` are gone; the rest survive the lock so they can
        // still service end-of-DXE, exit-boot-services and ready-to-boot.
        for (entry, &handle) in CORE_MMI_HANDLERS.iter().zip(handles.iter()) {
            assert_eq!(
                take_handler(core, handle),
                !entry.unregister_on_lock,
                "unexpected state for {} after ready-to-lock",
                entry.handler_type
            );
        }
    }
}
