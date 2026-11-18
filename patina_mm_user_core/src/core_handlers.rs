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
