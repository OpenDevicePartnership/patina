//! DXE Core Protocol
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{ffi::c_void, mem::size_of, ptr::NonNull};

use alloc::{slice, vec, vec::Vec};
use patina::standard::efi::{self, protocols::device_path::Protocol};
use patina::{
    OwnedGuid,
    error::EfiError,
    uefi::device_path::walker::{is_device_path_end, remaining_device_path},
};
use tpl_mutex::TplMutex;

use crate::{
    allocator::core_allocate_pool,
    driver_services::{core_connect_controller, core_disconnect_controller},
    events::{EVENT_DB, signal_event},
    protocol_db::{DXE_CORE_HANDLE, SpinLockedProtocolDb},
    systemtables::EfiSystemTable,
    tpl_mutex,
};

pub static PROTOCOL_DB: SpinLockedProtocolDb = SpinLockedProtocolDb::new();

/// Installs a protocol interface on a handle.
///
/// This function is safe because `interface` is an opaque pointer that is stored but never
/// dereferenced. All other parameters are value types.
pub fn core_install_protocol_interface(
    handle: Option<efi::Handle>,
    protocol: efi::Guid,
    interface: *mut c_void,
) -> Result<efi::Handle, EfiError> {
    log::info!("InstallProtocolInterface: {:?} @ {:#x?}", OwnedGuid::from(protocol), interface);
    let (handle, notifies) = PROTOCOL_DB.install_protocol_interface(handle, protocol, interface)?;

    let mut closed_events = Vec::new();

    for notify in notifies {
        if signal_event(notify.event) == efi::Status::INVALID_PARAMETER {
            //means event doesn't exist (probably closed).
            closed_events.push(notify.event); // Other error cases not actionable.
        }
    }

    PROTOCOL_DB.unregister_protocol_notify_events(closed_events);

    Ok(handle)
}

/// Installs a protocol interface on a handle.
///
/// # Safety
///
/// `handle` must be a valid pointer to an `efi::Handle` (may point to a null handle for new handle
/// creation). `protocol` must be a valid pointer to an `efi::Guid`. Both are null checked, but
/// validity of the referenced memory is the caller's responsibility. The `interface` pointer is
/// stored in the protocol database and the caller must ensure it remains valid for as long as the
/// interface is registered in the protocol database (i.e. until it is uninstalled via
/// `uninstall_protocol_interface` or `uninstall_multiple_protocol_interfaces`, or replaced via
/// `reinstall_protocol_interface`).
unsafe extern "efiapi" fn install_protocol_interface(
    handle: *mut efi::Handle,
    protocol: *mut efi::Guid,
    interface_type: efi::InterfaceType,
    interface: *mut c_void,
) -> efi::Status {
    if handle.is_null() || protocol.is_null() || interface_type != efi::NATIVE_INTERFACE {
        return efi::Status::INVALID_PARAMETER;
    }
    // SAFETY: Caller must ensure that handle and protocol are valid pointers. They are null-checked above.
    let caller_handle = unsafe { handle.read_unaligned() };
    // SAFETY: Caller must ensure that protocol is a valid pointer. It is checked for null above.
    let caller_protocol = unsafe { protocol.read_unaligned() };

    let caller_handle = if caller_handle.is_null() { None } else { Some(caller_handle) };

    let installed_handle = match core_install_protocol_interface(caller_handle, caller_protocol, interface) {
        Err(err) => return err.into(),
        Ok(handle) => handle,
    };

    // SAFETY: Caller must ensure that handle is a valid pointer. It is checked for null above.
    unsafe { handle.write_unaligned(installed_handle) };

    efi::Status::SUCCESS
}

/// Uninstalls a protocol interface on a handle.
///
/// This function is safe because `interface` is an opaque pointer used for
/// comparison but never dereferenced. All other parameters are value types.
pub fn core_uninstall_protocol_interface(
    handle: efi::Handle,
    protocol: efi::Guid,
    interface: *mut c_void,
) -> Result<(), EfiError> {
    log::info!("UninstallProtocolInterface: {:?} @ {:#x?}", OwnedGuid::from(protocol), interface);

    // Check if the handle/protocol/interface triple is legitimate
    match PROTOCOL_DB.get_interface_for_handle(handle, protocol) {
        Err(err) => return Err(err),
        Ok(found_interface) => {
            if found_interface != interface {
                return Err(EfiError::NotFound);
            }
        }
    }

    //attempt to close all OPEN_BY_DRIVER usages.
    let mut usage_close_status = Ok(());
    loop {
        let mut item_found = false;
        let usages = match PROTOCOL_DB.get_open_protocol_information_by_protocol(handle, protocol) {
            Ok(usages) => usages,
            Err(EfiError::NotFound) => Vec::new(),
            Err(err) => return Err(err),
        };

        for usage in usages {
            if (usage.attributes & efi::OPEN_PROTOCOL_BY_DRIVER) != 0 {
                debug_assert!(usage.agent_handle.is_some());
                // SAFETY: handles are validated inside core_disconnect_controller. The implicit UEFI
                // spec assumption that driver bindings remain valid for the duration of the call
                // cannot be verified here; see core_disconnect_controller's safety documentation.
                usage_close_status = unsafe { core_disconnect_controller(handle, usage.agent_handle, None) };
                if usage_close_status.is_ok() {
                    item_found = true;
                }
                break;
            }
        }

        if !item_found {
            break;
        }
    }

    //Attempt to remove BY_HANDLE_PROTOCOL, GET_PROTOCOL, and TEST_PROTOCOL usages.
    let mut unclosed_usages = false;
    if usage_close_status.is_ok() {
        let usages = match PROTOCOL_DB.get_open_protocol_information_by_protocol(handle, protocol) {
            Ok(usages) => usages,
            Err(EfiError::NotFound) => Vec::new(),
            Err(err) => return Err(err),
        };

        for usage in usages {
            if usage.attributes
                & (efi::OPEN_PROTOCOL_BY_HANDLE_PROTOCOL
                    | efi::OPEN_PROTOCOL_GET_PROTOCOL
                    | efi::OPEN_PROTOCOL_TEST_PROTOCOL)
                != 0
            {
                let result = PROTOCOL_DB.remove_protocol_usage(
                    handle,
                    protocol,
                    usage.agent_handle,
                    usage.controller_handle,
                    Some(usage.attributes),
                );
                if result.is_err() {
                    unclosed_usages = true;
                }
            } else {
                unclosed_usages = true;
            }
        }
    }

    if usage_close_status.is_err() || unclosed_usages {
        // SAFETY: Handle is validated above and reconnect is best-effort to restore state.
        unsafe {
            let _result = core_connect_controller(handle, Vec::new(), None, true);
        }
        return Err(EfiError::AccessDenied);
    }

    PROTOCOL_DB.uninstall_protocol_interface(handle, protocol, interface)
}

/// Uninstalls a protocol interface from a handle.
///
/// # Safety
///
/// `protocol` must be a valid pointer to an `efi::Guid`. It is null checked, but validity of the
/// referenced memory is the caller's responsibility. NOTE: `interface` is not dereferenced in this
/// function, so its validity is not required for safety.
unsafe extern "efiapi" fn uninstall_protocol_interface(
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    interface: *mut c_void,
) -> efi::Status {
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: Caller must ensure that protocol is a valid pointer. It is checked for null above.
    let caller_protocol = unsafe { protocol.read_unaligned() };

    core_uninstall_protocol_interface(handle, caller_protocol, interface)
        .map_or_else(core::convert::Into::into, |()| efi::Status::SUCCESS)
}

// {2ED6CB57-3A78-4C39-9A2A-CA037841D286}
const PRIVATE_DUMMY_INTERFACE_GUID: efi::Guid =
    efi::Guid::from_fields(0x2ed6cb57, 0x3a78, 0x4c39, 0x9a, 0x2a, &[0xca, 0x03, 0x78, 0x41, 0xd2, 0x86]);

fn install_dummy_interface(handle: efi::Handle) -> Result<(), EfiError> {
    PROTOCOL_DB
        .install_protocol_interface(Some(handle), PRIVATE_DUMMY_INTERFACE_GUID, core::ptr::null_mut())
        .map(|_| ())
}

fn uninstall_dummy_interface(handle: efi::Handle) -> Result<(), EfiError> {
    PROTOCOL_DB.uninstall_protocol_interface(handle, PRIVATE_DUMMY_INTERFACE_GUID, core::ptr::null_mut())
}

/// Reinstalls a protocol interface on a handle.
///
/// # Safety
///
/// `protocol` must be a valid pointer to an `efi::Guid`. It is null checked, but validity of the
/// referenced memory is the caller's responsibility. Throughout the lifetime of the interface
/// reference, the caller must ensure it remains valid.
unsafe extern "efiapi" fn reinstall_protocol_interface(
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    old_interface: *mut c_void,
    new_interface: *mut c_void,
) -> efi::Status {
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // A corner case can occur where the uninstall_protocol_interface below could uninstall the last interface on a handle
    // thus causing the handle to be deleted. The handle would then be invalid, and the following install would fail. To
    // deal with this, first install a dummy interface before attempting the uninstall. This dummy interface will prevent
    // the handle from becoming empty and invalidated. Failure here means that the reinstall has failed (e.g. due to
    // invalid handle).
    if let Err(err) = install_dummy_interface(handle) {
        return err.into();
    }

    // Call uninstall to close all agents that are currently consuming old_interface.
    // SAFETY: `protocol` is checked for null above. `handle` and `old_interface` are passed
    // through from the caller per the function-level safety contract.
    match unsafe { uninstall_protocol_interface(handle, protocol, old_interface) } {
        efi::Status::SUCCESS => (),
        err => {
            let result = uninstall_dummy_interface(handle);
            debug_assert!(result.is_ok());
            return err;
        }
    }

    // SAFETY: Caller must ensure that protocol is a valid pointer. It is checked for null above.
    let protocol = unsafe { protocol.read_unaligned() };

    // Call install to install the new interface and trigger any notifies
    if let Err(err) = core_install_protocol_interface(Some(handle), protocol, new_interface) {
        let result = uninstall_dummy_interface(handle);
        debug_assert!(result.is_ok());
        return err.into();
    }

    // Dummy interface is no longer required. Proceed if uninstall fails, but assert for debug.
    let result = uninstall_dummy_interface(handle);
    debug_assert!(result.is_ok());

    // Connect controller so agents that were forced to release old_interface can now consume new_interface. Error
    // status is ignored.
    // SAFETY: handle is valid and reconnect is best-effort to restore state after reinstall.
    unsafe {
        let _ = core_connect_controller(handle, Vec::new(), None, true);
    }

    efi::Status::SUCCESS
}

/// Registers for notification when a protocol interface is installed.
///
/// # Safety
///
/// `protocol` must be a valid pointer to an `efi::Guid`. `registration` must be a valid pointer
/// to receive the registration key. Both are null checked, but validity of the referenced memory
/// is the caller's responsibility.
unsafe extern "efiapi" fn register_protocol_notify(
    protocol: *mut efi::Guid,
    event: efi::Event,
    registration: *mut *mut c_void,
) -> efi::Status {
    if protocol.is_null() || registration.is_null() || !EVENT_DB.is_valid(event) {
        return efi::Status::INVALID_PARAMETER;
    }
    let protocol_guid = {
        // SAFETY: Caller must ensure that protocol is a valid pointer. It is checked for null above.
        unsafe { protocol.read_unaligned() }
    };
    match PROTOCOL_DB.register_protocol_notify(protocol_guid, event) {
        Err(err) => err.into(),
        Ok(new_registration) => {
            // SAFETY: Caller must ensure that registration is a valid pointer. It is checked for null above.
            unsafe { registration.write_unaligned(new_registration) };
            efi::Status::SUCCESS
        }
    }
}

/// Locates handles that support a specified protocol.
///
/// # Safety
///
/// `protocol` must be a valid pointer to an `efi::Guid` when `search_type` is `BY_PROTOCOL`.
/// `search_key` must be a valid registration key when `search_type` is `BY_REGISTER_NOTIFY`.
/// `buffer_size` must be a valid pointer to a `usize`.
/// `handle_buffer` must be valid for writes of the number of handles indicated by `buffer_size`.
///  All pointers are null checked, but validity of the referenced memory is the caller's responsibility.
unsafe extern "efiapi" fn locate_handle(
    search_type: efi::LocateSearchType,
    protocol: *mut efi::Guid,
    search_key: *mut c_void,
    buffer_size: *mut usize,
    handle_buffer: *mut efi::Handle,
) -> efi::Status {
    let search_result = match search_type {
        efi::ALL_HANDLES => PROTOCOL_DB.locate_handles(None),
        efi::BY_REGISTER_NOTIFY => {
            if search_key.is_null() {
                return efi::Status::INVALID_PARAMETER;
            }
            if let Some(handle) = PROTOCOL_DB.next_handle_for_registration(search_key) {
                Ok(vec![handle])
            } else {
                Err(EfiError::NotFound)
            }
        }
        efi::BY_PROTOCOL => {
            if protocol.is_null() {
                return efi::Status::INVALID_PARAMETER;
            }
            // SAFETY: Caller must ensure that protocol is a valid pointer. It is checked for null above.
            PROTOCOL_DB.locate_handles(Some(unsafe { protocol.read_unaligned() }))
        }
        _ => return efi::Status::INVALID_PARAMETER,
    };

    match search_result {
        Err(err) => err.into(),
        Ok(mut list) => {
            if list.is_empty() {
                return efi::Status::NOT_FOUND;
            }
            if buffer_size.is_null() {
                return efi::Status::INVALID_PARAMETER;
            }

            list.shrink_to_fit();
            // SAFETY: Caller must ensure that buffer_size is a valid pointer. It is checked for null above.
            let input_size = unsafe { buffer_size.read_unaligned() };
            // SAFETY: Caller must ensure that buffer_size is a valid pointer. It is checked for null above.
            unsafe {
                buffer_size.write_unaligned(list.len() * size_of::<efi::Handle>());
            }
            if input_size < list.len() * size_of::<efi::Handle>() {
                return efi::Status::BUFFER_TOO_SMALL;
            }
            if handle_buffer.is_null() {
                return efi::Status::INVALID_PARAMETER;
            }

            // SAFETY: Caller must ensure that handle_buffer is valid for writes of list.len() handles. It is checked for null above.
            unsafe {
                core::ptr::copy(
                    list.as_ptr() as *const u8,
                    handle_buffer as *mut u8,
                    list.len() * core::mem::size_of::<efi::Handle>(),
                );
            }

            efi::Status::SUCCESS
        }
    }
}

/// Retrieves a protocol interface from a handle.
///
/// # Safety
///
/// `protocol` must be a valid pointer to an `efi::Guid`. `interface` must be a valid pointer to
/// receive the protocol interface pointer. Both are null checked by the underlying `open_protocol`
/// implementation, but validity of the referenced memory is the caller's responsibility.
pub unsafe extern "efiapi" fn handle_protocol(
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    interface: *mut *mut c_void,
) -> efi::Status {
    // SAFETY: `protocol` and `interface` are forwarded from the caller per this function's safety
    // contract. `DXE_CORE_HANDLE` is a valid static handle. A null controller handle and
    // `BY_HANDLE_PROTOCOL` attributes are safe constant values.
    unsafe {
        open_protocol(
            handle,
            protocol,
            interface,
            DXE_CORE_HANDLE,
            core::ptr::null_mut(),
            efi::OPEN_PROTOCOL_BY_HANDLE_PROTOCOL,
        )
    }
}

/// Opens a protocol interface on a handle with usage tracking.
///
/// # Safety
///
/// `protocol` must be a valid pointer to an `efi::Guid`. `interface` must be a valid pointer to
/// receive the protocol interface pointer (may be null only when `attributes` is
/// `OPEN_PROTOCOL_TEST_PROTOCOL`). Both are null checked, but validity of the referenced memory
/// is the caller's responsibility.
unsafe extern "efiapi" fn open_protocol(
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    interface: *mut *mut c_void,
    agent_handle: efi::Handle,
    controller_handle: efi::Handle,
    attributes: u32,
) -> efi::Status {
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: Caller must ensure that protocol is a valid pointer. It is checked for null above.
    let protocol = unsafe { protocol.read_unaligned() };

    if interface.is_null() && attributes != efi::OPEN_PROTOCOL_TEST_PROTOCOL {
        return efi::Status::INVALID_PARAMETER;
    }

    let agent_handle = PROTOCOL_DB.validate_handle(agent_handle).map_or_else(|_err| None, |_ok| Some(agent_handle));

    let controller_handle =
        PROTOCOL_DB.validate_handle(controller_handle).map_or_else(|_err| None, |_ok| Some(controller_handle));

    // if attributes has exclusive flag set, then attempt to disconnect any other drivers that have the requested protocol
    // open on this handle BY_DRIVER.
    if (attributes & efi::OPEN_PROTOCOL_EXCLUSIVE) != 0 {
        let usages = match PROTOCOL_DB.get_open_protocol_information_by_protocol(handle, protocol) {
            Err(EfiError::NotFound) => Vec::new(),
            Err(err) => return err.into(),
            Ok(usages) => usages,
        };
        if let Some(usage) = usages.iter().find(|x| {
            (x.attributes & efi::OPEN_PROTOCOL_BY_DRIVER) != 0
                && (x.attributes & efi::OPEN_PROTOCOL_EXCLUSIVE) == 0
                && x.agent_handle != agent_handle
        }) && {
            // SAFETY: handles are validated inside core_disconnect_controller. The implicit UEFI
            // spec assumption that driver bindings remain valid for the duration of the call
            // cannot be verified here; see core_disconnect_controller's safety documentation.
            unsafe { core_disconnect_controller(handle, usage.agent_handle, None) }.is_err()
        } {
            return efi::Status::ACCESS_DENIED;
        }
    }

    match PROTOCOL_DB.add_protocol_usage(handle, protocol, agent_handle, controller_handle, attributes) {
        Err(EfiError::Unsupported) => {
            if !interface.is_null() {
                // SAFETY: Caller must ensure that interface is a valid pointer if it is non-null.
                unsafe { interface.write_unaligned(core::ptr::null_mut()) };
            }
            return efi::Status::UNSUPPORTED;
        }
        Err(EfiError::AlreadyStarted) if (attributes & efi::OPEN_PROTOCOL_BY_DRIVER) != 0 => {
            //For already started interface is still returned.
            let desired_interface = PROTOCOL_DB
                .get_interface_for_handle(handle, protocol)
                .expect("Already Started can't happen if protocol doesn't exist.");
            if !interface.is_null() {
                // SAFETY: Caller must ensure that interface is a valid pointer if it is non-null.
                unsafe { interface.write_unaligned(desired_interface) };
            }
            return efi::Status::ALREADY_STARTED;
        }
        Ok(()) | Err(EfiError::AlreadyStarted) => (),
        Err(err) => return err.into(),
    }

    let desired_interface = match PROTOCOL_DB.get_interface_for_handle(handle, protocol) {
        Err(err) => return err.into(),
        Ok(found) => found,
    };

    if attributes != efi::OPEN_PROTOCOL_TEST_PROTOCOL {
        // SAFETY: Caller must ensure that interface is a valid pointer if it is non-null.
        unsafe { interface.write_unaligned(desired_interface) };
    }
    efi::Status::SUCCESS
}

/// Closes a protocol on a handle that was previously opened.
///
/// # Safety
///
/// `protocol` must be a valid pointer to an `efi::Guid`. It is null checked, but validity of the
/// referenced memory is the caller's responsibility.
unsafe extern "efiapi" fn close_protocol(
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    agent_handle: efi::Handle,
    controller_handle: efi::Handle,
) -> efi::Status {
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    if PROTOCOL_DB.validate_handle(agent_handle).is_err() {
        return efi::Status::INVALID_PARAMETER;
    }

    let controller_handle = match controller_handle {
        _ if controller_handle.is_null() => None,
        _ => {
            if PROTOCOL_DB.validate_handle(controller_handle).is_err() {
                return efi::Status::INVALID_PARAMETER;
            }
            Some(controller_handle)
        }
    };

    let protocol_guid = {
        // SAFETY: Caller must ensure that protocol is a valid pointer. It is checked for null above.
        unsafe { protocol.read_unaligned() }
    };
    match PROTOCOL_DB.remove_protocol_usage(handle, protocol_guid, Some(agent_handle), controller_handle, None) {
        Err(err) => err.into(),
        Ok(()) => efi::Status::SUCCESS,
    }
}

/// Retrieves the list of agents that currently have a protocol interface opened.
///
/// # Safety
///
/// `protocol` must be a valid pointer to an `efi::Guid`. `entry_buffer` must be a valid pointer
/// to receive an allocated array of `OpenProtocolInformationEntry`. `entry_count` must be a valid
/// pointer to receive the number of entries. All pointers are null checked, but validity of the
/// referenced memory is the caller's responsibility.
unsafe extern "efiapi" fn open_protocol_information(
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    entry_buffer: *mut *mut efi::OpenProtocolInformationEntry,
    entry_count: *mut usize,
) -> efi::Status {
    if protocol.is_null() || entry_buffer.is_null() || entry_count.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let protocol_guid = {
        // SAFETY: Caller must ensure that protocol is a valid pointer. It is checked for null above.
        unsafe { protocol.read_unaligned() }
    };
    let mut open_info: Vec<efi::OpenProtocolInformationEntry> =
        match PROTOCOL_DB.get_open_protocol_information_by_protocol(handle, protocol_guid) {
            Err(err) => return err.into(),
            Ok(info) => info.into_iter().map(efi::OpenProtocolInformationEntry::from).collect(),
        };

    open_info.shrink_to_fit();

    let buffer_size = open_info.len() * size_of::<efi::OpenProtocolInformationEntry>();
    //caller is supposed to free the entry buffer using FreePool, so we need to allocate it using allocate pool.
    match core_allocate_pool(efi::BOOT_SERVICES_DATA, buffer_size) {
        Err(err) => err.into(),
        Ok(allocation) =>
        // SAFETY: Caller must ensure that entry_buffer and entry_count are valid pointers. They are null-checked above.
        unsafe {
            entry_buffer.write_unaligned(allocation as *mut efi::OpenProtocolInformationEntry);
            entry_count.write_unaligned(open_info.len());
            core::ptr::copy(
                open_info.as_ptr() as *const u8,
                allocation as *mut u8,
                open_info.len() * size_of::<efi::OpenProtocolInformationEntry>(),
            );
            efi::Status::SUCCESS
        },
    }
}

/// # Safety
///
/// `handle` must be a valid pointer to an `efi::Handle` (may point to a null handle for new
/// handle creation). `args` must consist of paired `(*mut efi::Guid, *mut c_void)` entries
/// terminated by a null `*mut efi::Guid` sentinel.
unsafe extern "C" fn install_multiple_protocol_interfaces(handle: *mut efi::Handle, mut args: ...) -> efi::Status {
    // The UEFI spec does not indicate whether the protocols installed here are atomic with respect to notify  - i.e.
    // whether any registered notifies should be invoked between the installation of the multiple protocols, or only
    // after all protocols are installed. Despite the spec ambiguity, the reference EDK2 C implementation does raise to
    // TPL_NOTIFY prior to installing any of the interfaces, which has the effect of deferring any protocol notify
    // callbacks until after all protocols are installed. This code matches those semantics by using a TPL guard here
    // to ensure the logic of this function is conducted at TPL_NOTIFY.
    let tpl_mutex = TplMutex::new(efi::TPL_NOTIFY, (), "atomic_protocol_install");
    let _tpl_guard = tpl_mutex.lock();

    if handle.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let mut interfaces_to_install = Vec::new();
    loop {
        //consume the protocol, break the loop if it is null.
        // SAFETY: Variadic argument list is controlled by the caller and accessed in order.
        let protocol: *mut efi::Guid = unsafe { args.arg() };
        if protocol.is_null() {
            break;
        }
        // SAFETY: Variadic argument list is controlled by the caller and accessed in order.
        let interface: *mut c_void = unsafe { args.arg() };
        // SAFETY: protocol is checked for null above before dereferencing.
        //         The caller-supplied pointer may be unaligned.
        if unsafe { protocol.read_unaligned() } == efi::protocols::device_path::PROTOCOL_GUID
            // `interface` is the value for the device_path::PROTOCOL_GUID being installed.
            // The caller is responsible for ensuring that it points to a valid device path when
            // it is dereferenced; here we only need a non-null guard.
            && let Some(device_path) = NonNull::new(interface as *mut _)
            && let Ok((remaining_path, handle)) =
                core_locate_device_path(efi::protocols::device_path::PROTOCOL_GUID, device_path)
            && PROTOCOL_DB.validate_handle(handle).is_ok()
            && {
                // SAFETY: remaining_path is returned from core_locate_device_path and is a valid device path pointer.
                unsafe { is_device_path_end(remaining_path.as_ptr()) }
            }
        {
            return efi::Status::ALREADY_STARTED;
        }

        interfaces_to_install.push((protocol, interface));
    }

    let mut interfaces_to_uninstall_on_error = Vec::new();
    for (protocol, interface) in interfaces_to_install {
        // SAFETY: `handle` is null checked above. `protocol` is null checked when building
        // interfaces_to_install. `interface` validity is the caller's responsibility per the
        // function level safety contract.
        match unsafe { install_protocol_interface(handle, protocol, efi::NATIVE_INTERFACE, interface) } {
            efi::Status::SUCCESS => interfaces_to_uninstall_on_error.push((protocol, interface)),
            err => {
                //on error, attempt to uninstall all the previously installed interfaces. best-effort, errors are ignored.
                for (protocol, interface) in interfaces_to_uninstall_on_error {
                    // SAFETY: handle is validated for null above.
                    let handle_value = unsafe { handle.read_unaligned() };
                    // SAFETY: `protocol` was null-checked when building interfaces_to_install.
                    // Best-effort rollback; errors are ignored.
                    let _ = unsafe { uninstall_protocol_interface(handle_value, protocol, interface) };
                }
                return err;
            }
        }
    }

    efi::Status::SUCCESS
}

/// Uninstalls multiple protocol interfaces from a handle.
///
/// # Safety
///
/// `args` must consist of paired `(*mut efi::Guid, *mut c_void)` entries terminated by a null
/// `*mut efi::Guid` sentinel.
unsafe extern "C" fn uninstall_multiple_protocol_interfaces(handle: efi::Handle, mut args: ...) -> efi::Status {
    if handle.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let mut interfaces_to_uninstall = Vec::new();
    loop {
        // SAFETY: Variadic argument list is controlled by the caller and accessed in order.
        let protocol: *mut efi::Guid = unsafe { args.arg() };
        if protocol.is_null() {
            break;
        }
        // SAFETY: Variadic argument list is controlled by the caller and accessed in order.
        let interface: *mut c_void = unsafe { args.arg() };
        interfaces_to_uninstall.push((protocol, interface));
    }

    let mut interfaces_to_reinstall_on_error = Vec::new();
    for (protocol, interface) in interfaces_to_uninstall {
        // SAFETY: `protocol` was null-checked when building interfaces_to_uninstall.
        // `handle` is validated for null above. `interface` is passed through from the caller.
        match unsafe { uninstall_protocol_interface(handle, protocol, interface) } {
            efi::Status::SUCCESS => interfaces_to_reinstall_on_error.push((protocol, interface)),
            _err => {
                //on error, attempt to re-install all the previously uninstall interfaces. best-effort, errors are ignored.
                for (protocol, interface) in interfaces_to_reinstall_on_error {
                    // SAFETY: protocol was checked for null when building interfaces_to_uninstall.
                    let protocol = unsafe { protocol.read_unaligned() };
                    let _ = core_install_protocol_interface(Some(handle), protocol, interface);
                }
                return efi::Status::INVALID_PARAMETER;
            }
        }
    }

    efi::Status::SUCCESS
}

/// # Safety
///
/// `protocol_buffer` must be a valid pointer to receive an allocated array of GUID pointers.
/// `protocol_buffer_count` must be a valid pointer to receive the number of entries. Both are
/// null checked, but validity of the referenced memory is the caller's responsibility.
unsafe extern "efiapi" fn protocols_per_handle(
    handle: efi::Handle,
    protocol_buffer: *mut *mut *mut efi::Guid,
    protocol_buffer_count: *mut usize,
) -> efi::Status {
    if protocol_buffer.is_null() || protocol_buffer_count.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }
    if PROTOCOL_DB.validate_handle(handle).is_err() {
        return efi::Status::INVALID_PARAMETER;
    }

    let mut protocol_list = match PROTOCOL_DB.get_protocols_on_handle(handle) {
        Ok(list) => list,
        Err(err) => return err.into(),
    };
    protocol_list.shrink_to_fit();

    //ProtocolsPerHandle is given a pointer to receive the allocation of a list of pointers to GUIDs.
    //Don't hand out pointers to our internal memory with the GUIDs - instead, allocate enough space
    //for both the list of pointers and the list of actual GUIDs they point to in the same allocated chunk.
    //When caller frees the list of pointers, the memory containing the GUIDs will also be freed. The UEFI
    //spec is not clear about the lifetime of the GUID pointers in the returned list; this code assumes that
    //callers of this routine treat the lifetime of the GUID pointers as coeval with the list itself.
    let ptr_buffer_size = protocol_list.len() * size_of::<*mut efi::Guid>();
    let guid_buffer_size = protocol_list.len() * size_of::<efi::Guid>();
    //caller is supposed to free the entry buffer using free pool, so we need to allocate it using allocate pool.
    match core_allocate_pool(efi::BOOT_SERVICES_DATA, ptr_buffer_size + guid_buffer_size) {
        Err(err) => err.into(),
        // SAFETY: Caller must ensure that protocol_buffer and protocol_buffer_count are valid pointers. They are null-checked above.
        Ok(allocation) => unsafe {
            protocol_buffer.write_unaligned(allocation as *mut *mut efi::Guid);
            protocol_buffer_count.write_unaligned(protocol_list.len());

            let guid_buffer = (allocation as usize + ptr_buffer_size) as *mut efi::Guid;
            let guids = slice::from_raw_parts_mut(guid_buffer, protocol_list.len());
            guids.copy_from_slice(&protocol_list);

            let guid_ptrs: Vec<*mut efi::Guid> = guids.iter_mut().map(core::ptr::from_mut::<efi::Guid>).collect();
            slice::from_raw_parts_mut(protocol_buffer.read_unaligned(), protocol_list.len())
                .copy_from_slice(&guid_ptrs);
            efi::Status::SUCCESS
        },
    }
}

/// # Safety
///
/// `protocol` must be a valid pointer to an `efi::Guid` when `search_type` is `BY_PROTOCOL`.
/// `search_key` must be a valid registration key when `search_type` is `BY_REGISTER_NOTIFY`.
/// `no_handles` must be a valid pointer to receive the handle count. `buffer` must be a valid
/// pointer to receive the allocated handle array. Both are null checked, but validity of the
/// referenced memory is the caller's responsibility.
unsafe extern "efiapi" fn locate_handle_buffer(
    search_type: efi::LocateSearchType,
    protocol: *mut efi::Guid,
    search_key: *mut c_void,
    no_handles: *mut usize,
    buffer: *mut *mut efi::Handle,
) -> efi::Status {
    if no_handles.is_null() || buffer.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // EDK2 C reference code unconditionally sets no_handles and buffer to default values regardless of success or failure
    // of the function, and some callers expect this behavior (and don't check return status before using no_handles).
    // SAFETY: Caller must ensure that no_handles and buffer are valid pointers. They are null-checked above.
    unsafe {
        no_handles.write_unaligned(0);
        buffer.write_unaligned(core::ptr::null_mut());
    }

    let handles = match search_type {
        efi::ALL_HANDLES => PROTOCOL_DB.locate_handles(None),
        efi::BY_REGISTER_NOTIFY => {
            if search_key.is_null() {
                return efi::Status::INVALID_PARAMETER;
            }
            if let Some(handle) = PROTOCOL_DB.next_handle_for_registration(search_key) {
                Ok(vec![handle])
            } else {
                Err(EfiError::NotFound)
            }
        }
        efi::BY_PROTOCOL => {
            if protocol.is_null() {
                return efi::Status::INVALID_PARAMETER;
            }
            // SAFETY: Caller must ensure that protocol is a valid pointer. It is checked for null above.
            unsafe { PROTOCOL_DB.locate_handles(Some(protocol.read_unaligned())) }
        }
        _ => return efi::Status::INVALID_PARAMETER,
    };
    let handles = match handles {
        Err(err) => return err.into(),
        Ok(handles) => handles,
    };

    if handles.is_empty() {
        efi::Status::NOT_FOUND
    } else {
        // caller is supposed to free the handle buffer using free pool, so we need to allocate it using allocate pool.
        let buffer_size = handles.len() * size_of::<efi::Handle>();
        match core_allocate_pool(efi::BOOT_SERVICES_DATA, buffer_size) {
            Err(err) => err.into(),
            // SAFETY: Caller must ensure that no_handles and buffer are valid pointers. They are null-checked above.
            Ok(allocation) => unsafe {
                buffer.write_unaligned(allocation as *mut efi::Handle);
                no_handles.write_unaligned(handles.len());
                slice::from_raw_parts_mut(buffer.read_unaligned(), handles.len()).copy_from_slice(&handles);
                efi::Status::SUCCESS
            },
        }
    }
}

/// Locates the first protocol instance matching a protocol GUID.
///
/// # Safety
///
/// `protocol` must be a valid pointer to an `efi::Guid`. `interface` must be a valid pointer to
/// receive the protocol interface pointer. `registration` must be null or a valid registration key
/// from a prior call to `register_protocol_notify`. All pointers are null checked, but validity
/// of the referenced memory is the caller's responsibility.
unsafe extern "efiapi" fn locate_protocol(
    protocol: *mut efi::Guid,
    registration: *mut c_void,
    interface: *mut *mut c_void,
) -> efi::Status {
    if protocol.is_null() || interface.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    if registration.is_null() {
        let protocol_guid = {
            // SAFETY: Caller must ensure that protocol is a valid pointer. It is checked for null above.
            unsafe { protocol.read_unaligned() }
        };
        match PROTOCOL_DB.locate_protocol(protocol_guid) {
            Err(err) => {
                // SAFETY: Caller must ensure that interface is a valid pointer. It is checked for null above.
                unsafe { interface.write_unaligned(core::ptr::null_mut()) };
                return err.into();
            }
            // SAFETY: Caller must ensure that interface is a valid pointer. It is checked for null above.
            Ok(i_face) => unsafe { interface.write_unaligned(i_face) },
        }
    } else if let Some(handle) = PROTOCOL_DB.next_handle_for_registration(registration) {
        let protocol_guid = {
            // SAFETY: Caller must ensure that protocol is a valid pointer. It is checked for null above.
            unsafe { protocol.read_unaligned() }
        };
        let i_face = PROTOCOL_DB
            .get_interface_for_handle(handle, protocol_guid)
            .expect("Protocol should exist on handle if it is returned for registration key.");
        // SAFETY: Caller must ensure that interface is a valid pointer. It is checked for null above.
        unsafe { interface.write_unaligned(i_face) };
    } else {
        return efi::Status::NOT_FOUND;
    }
    efi::Status::SUCCESS
}

pub fn core_locate_device_path(
    protocol: efi::Guid,
    device_path: NonNull<Protocol>,
) -> Result<(NonNull<Protocol>, efi::Handle), EfiError> {
    let device_path_protocol_guid = core::ptr::from_ref(&efi::protocols::device_path::PROTOCOL_GUID).cast_mut();

    let mut best_device: efi::Handle = core::ptr::null_mut();
    let mut best_match: isize = -1;
    let mut best_remaining_path: Option<NonNull<Protocol>> = None;

    let handles = PROTOCOL_DB.locate_handles(Some(protocol))?;

    for handle in handles {
        let mut temp_device_path: *mut efi::protocols::device_path::Protocol = core::ptr::null_mut();
        let temp_device_path_ptr: *mut *mut c_void = &raw mut temp_device_path as *mut *mut c_void;
        // SAFETY: `handle` comes from `locate_handles` and is valid. `device_path_protocol_guid`
        // points to a valid static GUID. `temp_device_path_ptr` is derived from a local variable
        // and is valid for writes.
        let status = unsafe { handle_protocol(handle, device_path_protocol_guid, temp_device_path_ptr) };
        if status != efi::Status::SUCCESS {
            continue;
        }

        // `temp_device_path` was written by `handle_protocol` returning SUCCESS; it points to a
        // valid device path protocol installed on `handle` (validity is required at dereference,
        // not for construction).
        let temp_device_path = match NonNull::new(temp_device_path) {
            Some(p) => p,
            None => continue,
        };

        let (remaining_path, matching_nodes) = match
            // SAFETY: temp_device_path and device_path are valid device path pointers per the
            // non-null guarantee and the caller's contract on `device_path`.
            unsafe { remaining_device_path(temp_device_path, device_path) }
        {
            Some((remaining_path, matching_nodes)) => (remaining_path, matching_nodes as isize),
            None => continue,
        };

        if matching_nodes > best_match {
            best_match = matching_nodes;
            best_device = handle;
            best_remaining_path = Some(remaining_path);
        }
    }

    match best_remaining_path {
        Some(path) => Ok((path, best_device)),
        None => Err(EfiError::NotFound),
    }
}

/// Locates a handle for a device on a device path that supports a specified protocol.
///
/// # Safety
///
/// `protocol` must be a valid pointer to an `efi::Guid`. `device_path` must be a valid pointer to
/// a `*mut Protocol` pointer that references a valid UEFI device path. `device` must be a valid
/// pointer to receive the located handle. All pointers are null checked, but validity of the
/// referenced memory is the caller's responsibility.
unsafe extern "efiapi" fn locate_device_path(
    protocol: *mut efi::Guid,
    device_path: *mut *mut efi::protocols::device_path::Protocol,
    device: *mut efi::Handle,
) -> efi::Status {
    if protocol.is_null() || device_path.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let current_device_path = {
        // SAFETY: device_path is null-checked above.
        unsafe { device_path.read_unaligned() }
    };
    // `current_device_path` was read from caller-supplied `device_path`; the caller is
    // responsible for its validity at dereference time. Here we only filter out null.
    let current_device_path = match NonNull::new(current_device_path) {
        Some(p) => p,
        None => return efi::Status::INVALID_PARAMETER,
    };

    let protocol_guid = {
        // SAFETY: protocol is null-checked above.
        unsafe { protocol.read_unaligned() }
    };
    let (best_remaining_path, best_device) = match core_locate_device_path(protocol_guid, current_device_path) {
        Err(err) => return err.into(),
        Ok((path, device)) => (path, device),
    };
    if device.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }
    // SAFETY: Caller must ensure that device_path and device are valid pointers. They are null-checked above.
    unsafe {
        device.write_unaligned(best_device);
        device_path.write_unaligned(best_remaining_path.as_ptr());
    }

    efi::Status::SUCCESS
}

pub fn init_protocol_support(st: &mut EfiSystemTable) {
    let mut bs = st.boot_services().get();

    // This bit of trickery is needed because r_efi definition of (Un)InstallMultipleProtocolInterfaces
    // is not variadic, due to rust only supporting variadic for "unsafe extern C" and not "efiapi"
    // until rust 1.91. For our purposes "efiapi" and "extern C" match, so we can get away with a
    // transmute here. Fixing it properly would require an upstream change in r_efi to pick up. There is also a bug in
    // the r_efi definition for uninstall_multiple_protocol_interfaces - per spec, the first argument is a handle, but
    // r_efi has it as *mut handle.
    // SAFETY: Transmute bridges r_efi signature mismatch for variadic interface. ABI matches for efiapi/extern C.
    bs.install_multiple_protocol_interfaces = unsafe {
        let ptr = install_multiple_protocol_interfaces as *const ();
        core::mem::transmute::<*const (), extern "efiapi" fn(*mut *mut c_void, *mut c_void, *mut c_void) -> efi::Status>(
            ptr,
        )
    };
    // SAFETY: Transmute bridges r_efi signature mismatch for variadic interface. ABI matches for efiapi/extern C.
    bs.uninstall_multiple_protocol_interfaces = unsafe {
        let ptr = uninstall_multiple_protocol_interfaces as *const ();
        core::mem::transmute::<*const (), extern "efiapi" fn(*mut c_void, *mut c_void, *mut c_void) -> efi::Status>(ptr)
    };

    bs.install_protocol_interface = install_protocol_interface;
    bs.uninstall_protocol_interface = uninstall_protocol_interface;
    bs.reinstall_protocol_interface = reinstall_protocol_interface;
    bs.register_protocol_notify = register_protocol_notify;
    bs.locate_handle = locate_handle;
    bs.handle_protocol = handle_protocol;
    bs.open_protocol = open_protocol;
    bs.close_protocol = close_protocol;
    bs.open_protocol_information = open_protocol_information;
    bs.protocols_per_handle = protocols_per_handle;
    bs.locate_handle_buffer = locate_handle_buffer;
    bs.locate_protocol = locate_protocol;
    bs.locate_device_path = locate_device_path;

    st.boot_services().set(bs);
}
