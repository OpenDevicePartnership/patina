//! MM Core Internal MMI Handlers
//!
//! MMI handlers the core registers for lifecycle events forwarded from DXE. They mirror
//! the C `mMmCoreMmiHandlers[]` table in `StandaloneMmCore.c`.
//!
//! Each handler is registered with [`MmiDatabase::register_internal_handler`](crate::mmi::MmiDatabase::register_internal_handler)
//! during startup and runs when the supervisor forwards an MMI for its GUID.
//!
//! | GUID | Handler | Description |
//! |------|---------|-------------|
//! | `MM_DISPATCH_EVENT_GROUP_GUID` | `mm_driver_dispatch_handler` | Prepares and runs components, then dispatches discovered MM drivers |
//! | `MM_DXE_READY_TO_LOCK_PROTOCOL_GUID` | `mm_ready_to_lock_handler` | Unregisters one-shot handlers, installs the ready-to-lock protocol |
//! | `MM_END_OF_PEI_PROTOCOL_GUID` | `mm_end_of_pei_handler` | Installs the end-of-PEI protocol |
//! | `END_OF_DXE_EVENT_GROUP_GUID` | `mm_end_of_dxe_handler` | Installs the end-of-DXE protocol |
//! | `EXIT_BOOT_SERVICES_EVENT_GROUP_GUID` | `mm_exit_boot_service_handler` | Installs the exit-boot-services protocol |
//! | `READY_TO_BOOT_EVENT_GROUP_GUID` | `mm_ready_to_boot_handler` | Installs the ready-to-boot protocol |
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

/// Table of MMI handlers registered by the MM Core, mirroring the C `mMmCoreMmiHandlers[]`.
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
///
/// Filled in by [`register_core_mmi_handlers`].
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
// SAFETY: The handles are only written by the BSP during single-threaded init and read back on
// the BSP during the ready-to-lock handler, so they are never shared across CPUs concurrently.
unsafe impl Send for SendHandle {}
// SAFETY: As above.
unsafe impl Sync for SendHandle {}

impl SendHandle {
    const NULL: Self = Self(core::ptr::null_mut());
}

/// Registers every core MMI handler with the MMI database.
///
/// This includes the `MM_DISPATCH_EVENT_GROUP_GUID` handler that performs the deferred
/// component and driver dispatch for the platform's `C`.
pub fn register_core_mmi_handlers<C: MmComponentInfo>() {
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

/// Installs a protocol with a null interface on a new handle, following the C pattern:
/// ```c
/// MmHandle = NULL;
/// Status = MmInstallProtocolInterface(&MmHandle, &guid, EFI_NATIVE_INTERFACE, NULL);
/// ```
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

/// MM Driver Dispatch Handler.
///
/// Prepares and runs components, then dispatches the drivers discovered during `StartUserCore`.
///
/// Corresponds to the C `MmDriverDispatchHandler`.
fn mm_driver_dispatch_handler<C: MmComponentInfo>(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    log::info!("MM driver dispatch event received");

    let handles = DISPATCH_HANDLES.lock();
    let dispatch_handle = handles.first().map_or(core::ptr::null_mut(), |handle| handle.0);
    drop(handles);

    // One-shot: retire before running any component or driver code, so a nested dispatch
    // event cannot re-enter.
    let core = MmUserCore::instance();
    if !dispatch_handle.is_null()
        && let Err(status) = core.mmi_handler_unregister(dispatch_handle)
    {
        log::error!("Failed to unregister MM dispatch handler: {:#x}", status.as_usize());
        return status;
    }

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

/// MM Ready To Lock Handler.
///
/// Runs on the `gEfiDxeMmReadyToLockProtocolGuid` MMI. Unregisters the handlers marked
/// `unregister_on_lock` (including itself), then installs `gEfiMmReadyToLockProtocolGuid`
/// to notify MM drivers.
///
/// Corresponds to the C `MmReadyToLockHandler`.
fn mm_ready_to_lock_handler<C: MmComponentInfo>(
    _handler_type: &efi::Guid,
    _comm_buffer: *mut c_void,
    _comm_buffer_size: *mut usize,
) -> efi::Status {
    log::info!("MM Ready To Lock event received");

    // Unregister the handlers that are not needed after the lock.
    let handles = DISPATCH_HANDLES.lock();
    for (handle, entry) in handles.iter().zip(core_mmi_handlers::<C>().iter()) {
        if entry.unregister_on_lock && !handle.0.is_null() {
            let _ = MmUserCore::instance().mmi_handler_unregister(handle.0);
        }
    }
    drop(handles);

    let status = install_lifecycle_protocol(&MM_READY_TO_LOCK_PROTOCOL_GUID);
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
    log::info!("MM End of PEI event received");

    install_lifecycle_protocol(&MM_END_OF_PEI_PROTOCOL_GUID)
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
    log::info!("MM End of DXE event received");

    install_lifecycle_protocol(&MM_END_OF_DXE_PROTOCOL_GUID)
}

/// MM Exit Boot Service Handler.
///
/// Installs the `gEfiEventExitBootServicesGuid` protocol on the first call only.
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
        status = install_lifecycle_protocol(&EXIT_BOOT_SERVICES_EVENT_GROUP_GUID);
    });

    status
}

/// MM Ready To Boot Handler.
///
/// Installs the `gEfiEventReadyToBootGuid` protocol on the first call only.
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
        status = install_lifecycle_protocol(&READY_TO_BOOT_EVENT_GROUP_GUID);
    });

    status
}
