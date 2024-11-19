//! DXE Core Protocol
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use core::{ffi::c_void, mem::size_of};

use alloc::{slice, vec, vec::Vec};
use mu_rust_helpers::guid::guid_fmt;
use r_efi::efi;
use tpl_lock::TplMutex;
use uefi_device_path::{is_device_path_end, remaining_device_path};

use crate::{
    allocator::core_allocate_pool,
    driver_services::{core_connect_controller, core_disconnect_controller},
    events::{signal_event, EVENT_DB},
    protocol_db::{SpinLockedProtocolDb, DXE_CORE_HANDLE},
};

pub static PROTOCOL_DB: SpinLockedProtocolDb = SpinLockedProtocolDb::new();

pub fn core_install_protocol_interface(
    handle: Option<efi::Handle>,
    protocol: efi::Guid,
    interface: *mut c_void,
) -> Result<efi::Handle, efi::Status> {
    log::info!("InstallProtocolInterface: {:?} @ {:#x?}", guid_fmt!(protocol), interface);
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

extern "efiapi" fn install_protocol_interface(
    handle: *mut efi::Handle,
    protocol: *mut efi::Guid,
    interface_type: efi::InterfaceType,
    interface: *mut c_void,
) -> efi::Status {
    if handle.is_null() || protocol.is_null() || interface_type != efi::NATIVE_INTERFACE {
        return efi::Status::INVALID_PARAMETER;
    }

    let caller_handle = unsafe { *handle };
    let caller_protocol = unsafe { *protocol };

    let caller_handle = if caller_handle.is_null() { None } else { Some(caller_handle) };

    let installed_handle = match core_install_protocol_interface(caller_handle, caller_protocol, interface) {
        Err(err) => return err,
        Ok(handle) => handle,
    };

    unsafe { *handle = installed_handle };

    efi::Status::SUCCESS
}

extern "efiapi" fn uninstall_protocol_interface(
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    interface: *mut c_void,
) -> efi::Status {
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let caller_protocol = *(unsafe { protocol.as_mut().expect("previously null-checked pointer is null") });

    // Check if the handle/protocol/interface triple is legitimate
    match PROTOCOL_DB.get_interface_for_handle(handle, caller_protocol) {
        Err(err) => return err,
        Ok(found_interface) => {
            if found_interface != interface {
                return efi::Status::NOT_FOUND;
            }
        }
    }

    //attempt to close all OPEN_BY_DRIVER usages.
    let mut usage_close_status = Ok(());
    loop {
        let mut item_found = false;
        let usages = match PROTOCOL_DB.get_open_protocol_information_by_protocol(handle, caller_protocol) {
            Ok(usages) => usages,
            Err(efi::Status::NOT_FOUND) => Vec::new(),
            Err(err) => return err,
        };

        for usage in usages {
            if (usage.attributes & efi::OPEN_PROTOCOL_BY_DRIVER) != 0 {
                debug_assert!(usage.agent_handle.is_some());
                unsafe {
                    usage_close_status = core_disconnect_controller(handle, usage.agent_handle, None);
                    if usage_close_status.is_ok() {
                        item_found = true;
                    }
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
        let usages = match PROTOCOL_DB.get_open_protocol_information_by_protocol(handle, caller_protocol) {
            Ok(usages) => usages,
            Err(efi::Status::NOT_FOUND) => Vec::new(),
            Err(err) => return err,
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
                    caller_protocol,
                    usage.agent_handle,
                    usage.controller_handle,
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
        unsafe {
            let _result = core_connect_controller(handle, Vec::new(), None, true);
        }
        return efi::Status::ACCESS_DENIED;
    }

    match PROTOCOL_DB.uninstall_protocol_interface(handle, caller_protocol, interface) {
        Err(err) => err,
        Ok(()) => efi::Status::SUCCESS,
    }
}

// {2ED6CB57-3A78-4C39-9A2A-CA037841D286}
const PRIVATE_DUMMY_INTERFACE_GUID: efi::Guid =
    efi::Guid::from_fields(0x2ed6cb57, 0x3a78, 0x4c39, 0x9a, 0x2a, &[0xca, 0x03, 0x78, 0x41, 0xd2, 0x86]);

fn install_dummy_interface(handle: efi::Handle) -> Result<(), efi::Status> {
    PROTOCOL_DB
        .install_protocol_interface(Some(handle), PRIVATE_DUMMY_INTERFACE_GUID, core::ptr::null_mut())
        .map(|_| ())
}

fn uninstall_dummy_interface(handle: efi::Handle) -> Result<(), efi::Status> {
    PROTOCOL_DB.uninstall_protocol_interface(handle, PRIVATE_DUMMY_INTERFACE_GUID, core::ptr::null_mut())
}

extern "efiapi" fn reinstall_protocol_interface(
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
        return err;
    }

    // Call uninstall to close all agents that are currently consuming old_interface.
    match uninstall_protocol_interface(handle, protocol, old_interface) {
        efi::Status::SUCCESS => (),
        err => {
            let result = uninstall_dummy_interface(handle);
            debug_assert!(result.is_ok());
            return err;
        }
    }

    let protocol = *(unsafe { protocol.as_mut().expect("previously null-checked pointer is null") });

    // Call install to install the new interface and trigger any notifies
    if let Err(err) = core_install_protocol_interface(Some(handle), protocol, new_interface) {
        let result = uninstall_dummy_interface(handle);
        debug_assert!(result.is_ok());
        return err;
    }

    // Dummy interface is no longer required. Proceed if uninstall fails, but assert for debug.
    let result = uninstall_dummy_interface(handle);
    debug_assert!(result.is_ok());

    // Connect controller so agents that were forced to release old_interface can now consume new_interface. Error
    // status is ignored.
    unsafe {
        let _ = core_connect_controller(handle, Vec::new(), None, true);
    }

    efi::Status::SUCCESS
}

extern "efiapi" fn register_protocol_notify(
    protocol: *mut efi::Guid,
    event: efi::Event,
    registration: *mut *mut c_void,
) -> efi::Status {
    if protocol.is_null() || registration.is_null() || !EVENT_DB.is_valid(event) {
        return efi::Status::INVALID_PARAMETER;
    }

    match PROTOCOL_DB.register_protocol_notify(unsafe { *protocol }, event) {
        Err(err) => err,
        Ok(new_registration) => {
            unsafe { *registration = new_registration };
            efi::Status::SUCCESS
        }
    }
}

extern "efiapi" fn locate_handle(
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
                Err(efi::Status::NOT_FOUND)
            }
        }
        efi::BY_PROTOCOL => {
            if protocol.is_null() {
                return efi::Status::INVALID_PARAMETER;
            }
            PROTOCOL_DB.locate_handles(Some(unsafe { *protocol }))
        }
        _ => return efi::Status::INVALID_PARAMETER,
    };

    match search_result {
        Err(err) => err,
        Ok(mut list) => {
            if list.is_empty() {
                return efi::Status::NOT_FOUND;
            }
            if buffer_size.is_null() {
                return efi::Status::INVALID_PARAMETER;
            }

            list.shrink_to_fit();
            let input_size = unsafe { *buffer_size };
            unsafe {
                *buffer_size = list.len() * size_of::<efi::Handle>();
            }
            if input_size < list.len() * size_of::<efi::Handle>() {
                return efi::Status::BUFFER_TOO_SMALL;
            }
            if handle_buffer.is_null() {
                return efi::Status::INVALID_PARAMETER;
            }

            //copy handle list into output buffer
            unsafe { slice::from_raw_parts_mut(handle_buffer, list.len()).copy_from_slice(&list) };

            efi::Status::SUCCESS
        }
    }
}

pub extern "efiapi" fn handle_protocol(
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    interface: *mut *mut c_void,
) -> efi::Status {
    open_protocol(
        handle,
        protocol,
        interface,
        DXE_CORE_HANDLE,
        core::ptr::null_mut(),
        efi::OPEN_PROTOCOL_BY_HANDLE_PROTOCOL,
    )
}

extern "efiapi" fn open_protocol(
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

    let protocol = match unsafe { protocol.as_ref() } {
        Some(protocol) => *protocol,
        None => return efi::Status::INVALID_PARAMETER,
    };

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
            Err(efi::Status::NOT_FOUND) => Vec::new(),
            Err(err) => return err,
            Ok(usages) => usages,
        };
        if let Some(usage) = usages.iter().find(|x| {
            (x.attributes & efi::OPEN_PROTOCOL_BY_DRIVER) != 0
                && (x.attributes & efi::OPEN_PROTOCOL_EXCLUSIVE) == 0
                && x.agent_handle != agent_handle
        }) {
            unsafe {
                if core_disconnect_controller(handle, usage.agent_handle, None).is_err() {
                    return efi::Status::ACCESS_DENIED;
                }
            }
        }
    }

    match PROTOCOL_DB.add_protocol_usage(handle, protocol, agent_handle, controller_handle, attributes) {
        Err(efi::Status::UNSUPPORTED) => {
            if !interface.is_null() {
                unsafe { interface.write(core::ptr::null_mut()) };
            }
            return efi::Status::UNSUPPORTED;
        }
        Err(efi::Status::ACCESS_DENIED) => {
            return efi::Status::ACCESS_DENIED;
        }
        Err(efi::Status::ALREADY_STARTED) if (attributes & efi::OPEN_PROTOCOL_BY_DRIVER) != 0 => {
            //For already started interface is still returned.
            let desired_interface = PROTOCOL_DB
                .get_interface_for_handle(handle, protocol)
                .expect("Already Started can't happen if protocol doesn't exist.");
            if !interface.is_null() {
                unsafe { interface.write(desired_interface) };
            }
            return efi::Status::ALREADY_STARTED;
        }
        Err(efi::Status::ALREADY_STARTED) => (),
        Err(err) => return err,
        Ok(_) => (),
    };

    let desired_interface = match PROTOCOL_DB.get_interface_for_handle(handle, protocol) {
        Err(err) => return err,
        Ok(found) => found,
    };

    if attributes != efi::OPEN_PROTOCOL_TEST_PROTOCOL {
        unsafe { interface.write(desired_interface) };
    }
    efi::Status::SUCCESS
}

extern "efiapi" fn close_protocol(
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

    match PROTOCOL_DB.remove_protocol_usage(handle, unsafe { *protocol }, Some(agent_handle), controller_handle) {
        Err(err) => err,
        Ok(_) => efi::Status::SUCCESS,
    }
}

extern "efiapi" fn open_protocol_information(
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    entry_buffer: *mut *mut efi::OpenProtocolInformationEntry,
    entry_count: *mut usize,
) -> efi::Status {
    if protocol.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let mut open_info: Vec<efi::OpenProtocolInformationEntry> =
        match PROTOCOL_DB.get_open_protocol_information_by_protocol(handle, unsafe { *protocol }) {
            Err(err) => return err,
            Ok(info) => info.into_iter().map(efi::OpenProtocolInformationEntry::from).collect(),
        };

    open_info.shrink_to_fit();

    let buffer_size = open_info.len() * size_of::<efi::OpenProtocolInformationEntry>();
    //caller is supposed to free the entry buffer using FreePool, so we need to allocate it using allocate pool.
    match core_allocate_pool(efi::BOOT_SERVICES_DATA, buffer_size) {
        Err(err) => err,
        Ok(allocation) => unsafe {
            entry_buffer.write(allocation as *mut efi::OpenProtocolInformationEntry);
            *entry_count = open_info.len();
            slice::from_raw_parts_mut(*entry_buffer, open_info.len()).copy_from_slice(&open_info);
            efi::Status::SUCCESS
        },
    }
}

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
        let protocol: *mut efi::Guid = args.arg();
        if protocol.is_null() {
            break;
        }
        let interface: *mut c_void = args.arg();
        if *protocol == efi::protocols::device_path::PROTOCOL_GUID {
            if let Ok((remaining_path, handle)) = core_locate_device_path(
                efi::protocols::device_path::PROTOCOL_GUID,
                interface as *const efi::protocols::device_path::Protocol,
            ) {
                if PROTOCOL_DB.validate_handle(handle).is_ok() && is_device_path_end(remaining_path) {
                    return efi::Status::ALREADY_STARTED;
                }
            }
        }

        interfaces_to_install.push((protocol, interface));
    }

    let mut interfaces_to_uninstall_on_error = Vec::new();
    for (protocol, interface) in interfaces_to_install {
        match install_protocol_interface(handle, protocol, efi::NATIVE_INTERFACE, interface) {
            efi::Status::SUCCESS => interfaces_to_uninstall_on_error.push((protocol, interface)),
            err => {
                //on error, attempt to uninstall all the previously installed interfaces. best-effort, errors are ignored.
                for (protocol, interface) in interfaces_to_uninstall_on_error {
                    let _ = uninstall_protocol_interface(*handle, protocol, interface);
                }
                return err;
            }
        }
    }

    efi::Status::SUCCESS
}

unsafe extern "C" fn uninstall_multiple_protocol_interfaces(handle: efi::Handle, mut args: ...) -> efi::Status {
    // See note in install_multiple_protocol_interfaces.
    let tpl_mutex = TplMutex::new(efi::TPL_NOTIFY, (), "atomic_protocol_uninstall");
    let _tpl_guard = tpl_mutex.lock();

    if handle.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let mut interfaces_to_uninstall = Vec::new();
    loop {
        let protocol: *mut efi::Guid = args.arg();
        if protocol.is_null() {
            break;
        }
        let interface: *mut c_void = args.arg();
        interfaces_to_uninstall.push((protocol, interface));
    }

    let mut interfaces_to_reinstall_on_error = Vec::new();
    for (protocol, interface) in interfaces_to_uninstall {
        match uninstall_protocol_interface(handle, protocol, interface) {
            efi::Status::SUCCESS => interfaces_to_reinstall_on_error.push((protocol, interface)),
            _err => {
                //on error, attempt to re-install all the previously uninstall interfaces. best-effort, errors are ignored.
                for (protocol, interface) in interfaces_to_reinstall_on_error {
                    let protocol = *(unsafe { protocol.as_mut().expect("previously null-checked pointer is null.") });
                    let _ = core_install_protocol_interface(Some(handle), protocol, interface);
                }
                return efi::Status::INVALID_PARAMETER;
            }
        }
    }

    efi::Status::SUCCESS
}

extern "efiapi" fn protocols_per_handle(
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
        Err(err) => return err,
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
        Err(err) => err,
        Ok(allocation) => unsafe {
            protocol_buffer.write(allocation as *mut *mut efi::Guid);
            protocol_buffer_count.write(protocol_list.len());

            let guid_buffer = (*protocol_buffer as usize + ptr_buffer_size) as *mut efi::Guid;
            let guids = slice::from_raw_parts_mut(guid_buffer, protocol_list.len());
            guids.copy_from_slice(&protocol_list);

            let guid_ptrs: Vec<*mut efi::Guid> = guids.iter_mut().map(|x| x as *mut efi::Guid).collect();
            slice::from_raw_parts_mut(*protocol_buffer, protocol_list.len()).copy_from_slice(&guid_ptrs);
            efi::Status::SUCCESS
        },
    }
}

extern "efiapi" fn locate_handle_buffer(
    search_type: efi::LocateSearchType,
    protocol: *mut efi::Guid,
    search_key: *mut c_void,
    no_handles: *mut usize,
    buffer: *mut *mut efi::Handle,
) -> efi::Status {
    if no_handles.is_null() || buffer.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    //EDK2 C reference code unconditionally sets no_handles and buffer to default values regardless of success or failure
    //of the function, and some callers expect this behavior (and don't check return status before using no_handles).
    unsafe {
        no_handles.write(0);
        buffer.write(core::ptr::null_mut());
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
                Err(efi::Status::NOT_FOUND)
            }
        }
        efi::BY_PROTOCOL => {
            if protocol.is_null() {
                return efi::Status::INVALID_PARAMETER;
            }
            unsafe { PROTOCOL_DB.locate_handles(Some(*protocol)) }
        }
        _ => return efi::Status::INVALID_PARAMETER,
    };
    let handles = match handles {
        Err(err) => return err,
        Ok(handles) => handles,
    };

    if handles.is_empty() {
        efi::Status::NOT_FOUND
    } else {
        //caller is supposed to free the handle buffer using free pool, so we need to allocate it using allocate pool.
        let buffer_size = handles.len() * size_of::<efi::Handle>();
        match core_allocate_pool(efi::BOOT_SERVICES_DATA, buffer_size) {
            Err(err) => err,
            Ok(allocation) => unsafe {
                buffer.write(allocation as *mut efi::Handle);
                no_handles.write(handles.len());
                slice::from_raw_parts_mut(*buffer, handles.len()).copy_from_slice(&handles);
                efi::Status::SUCCESS
            },
        }
    }
}

extern "efiapi" fn locate_protocol(
    protocol: *mut efi::Guid,
    registration: *mut c_void,
    interface: *mut *mut c_void,
) -> efi::Status {
    if protocol.is_null() || interface.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    if !registration.is_null() {
        if let Some(handle) = PROTOCOL_DB.next_handle_for_registration(registration) {
            let i_face = PROTOCOL_DB
                .get_interface_for_handle(handle, unsafe { *protocol })
                .expect("Protocol should exist on handle if it is returned for registration key.");
            unsafe { interface.write(i_face) };
        } else {
            return efi::Status::NOT_FOUND;
        }
    } else {
        match PROTOCOL_DB.locate_protocol(unsafe { *protocol }) {
            Err(err) => {
                unsafe { interface.write(core::ptr::null_mut()) };
                return err;
            }
            Ok(i_face) => unsafe { interface.write(i_face) },
        }
    }
    efi::Status::SUCCESS
}

pub fn core_locate_device_path(
    protocol: efi::Guid,
    device_path: *const r_efi::protocols::device_path::Protocol,
) -> Result<(*mut r_efi::protocols::device_path::Protocol, efi::Handle), efi::Status> {
    let device_path_protocol_guid = &r_efi::protocols::device_path::PROTOCOL_GUID as *const _ as *mut efi::Guid;

    let mut best_device: efi::Handle = core::ptr::null_mut();
    let mut best_match: isize = -1;
    let mut best_remaining_path: *const r_efi::protocols::device_path::Protocol = core::ptr::null_mut();

    let handles = match PROTOCOL_DB.locate_handles(Some(protocol)) {
        Err(err) => return Err(err),
        Ok(handles) => handles,
    };

    for handle in handles {
        let mut temp_device_path: *mut r_efi::protocols::device_path::Protocol = core::ptr::null_mut();
        let temp_device_path_ptr: *mut *mut c_void = &mut temp_device_path as *mut _ as *mut *mut c_void;
        let status = handle_protocol(handle, device_path_protocol_guid, temp_device_path_ptr);
        if status != efi::Status::SUCCESS {
            continue;
        }

        let (remaining_path, matching_nodes) = match remaining_device_path(temp_device_path, device_path) {
            Some((remaining_path, matching_nodes)) => (remaining_path, matching_nodes as isize),
            None => continue,
        };

        if matching_nodes > best_match {
            best_match = matching_nodes;
            best_device = handle;
            best_remaining_path = remaining_path;
        }
    }

    if best_match == -1 {
        return Err(efi::Status::NOT_FOUND);
    }

    Ok((best_remaining_path as *mut r_efi::protocols::device_path::Protocol, best_device))
}

extern "efiapi" fn locate_device_path(
    protocol: *mut efi::Guid,
    device_path: *mut *mut r_efi::protocols::device_path::Protocol,
    device: *mut efi::Handle,
) -> efi::Status {
    if protocol.is_null() || device_path.is_null() || unsafe { *device_path }.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let (best_remaining_path, best_device) =
        match core_locate_device_path(unsafe { *protocol }, unsafe { *device_path }) {
            Err(err) => return err,
            Ok((path, device)) => (path, device),
        };
    if device.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }
    unsafe {
        device.write(best_device);
        device_path.write(best_remaining_path);
    }

    efi::Status::SUCCESS
}

pub fn init_protocol_support(bs: &mut efi::BootServices) {
    //make sure that well-known handles exist.
    PROTOCOL_DB.init_protocol_db();

    //This bit of trickery is needed because r_efi definition of (Un)InstallMultipleProtocolInterfaces
    //is not variadic, due to rust only supporting variadic for "unsafe extern C" and not "efiapi"
    //until very recently. For x86_64 "efiapi" and "extern C" match, so we can get away with a
    //transmute here. Fixing it for other architectures more generally would require an upstream
    //change in r_efi to pick up. There is also a bug in r_efi definition for
    //uninstall_multiple_program_interfaces - per spec, the first argument is a handle, but
    //r_efi has it as *mut handle.
    bs.install_multiple_protocol_interfaces = unsafe {
        let ptr = install_multiple_protocol_interfaces as *const ();
        core::mem::transmute::<*const (), extern "efiapi" fn(*mut *mut c_void, *mut c_void, *mut c_void) -> efi::Status>(
            ptr,
        )
    };
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
}
