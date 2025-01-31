//! DXE Core Driver Services
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::{collections::BTreeMap, vec::Vec};
use core::{ptr::NonNull, slice::from_raw_parts_mut};
use uefi_device_path::{concat_device_path_to_boxed_slice, copy_device_path_to_boxed_slice};

use r_efi::efi;

use crate::protocols::PROTOCOL_DB;

fn get_bindings_for_handles(handles: Vec<efi::Handle>) -> Vec<*mut efi::protocols::driver_binding::Protocol> {
    handles
        .iter()
        .filter_map(|x| {
            match PROTOCOL_DB.get_interface_for_handle(*x, efi::protocols::driver_binding::PROTOCOL_GUID) {
                Ok(interface) => Some(interface as *mut efi::protocols::driver_binding::Protocol),
                Err(_) => None, //ignore handles without driver bindings
            }
        })
        .collect()
}

fn get_platform_driver_override_bindings(
    controller_handle: efi::Handle,
) -> Vec<*mut efi::protocols::driver_binding::Protocol> {
    let driver_override_protocol = match PROTOCOL_DB
        .locate_protocol(efi::protocols::platform_driver_override::PROTOCOL_GUID)
    {
        Err(_) => return Vec::new(),
        Ok(protocol) => unsafe {
            (protocol as *mut efi::protocols::platform_driver_override::Protocol).as_mut().expect("bad protocol ptr")
        },
    };

    let mut driver_overrides = Vec::new();
    let mut driver_image_handle: efi::Handle = core::ptr::null_mut();
    loop {
        let status = (driver_override_protocol.get_driver)(
            driver_override_protocol,
            controller_handle,
            core::ptr::addr_of_mut!(driver_image_handle),
        );
        if status != efi::Status::SUCCESS {
            break;
        }
        driver_overrides.push(driver_image_handle);
    }

    get_bindings_for_handles(driver_overrides)
}

fn get_family_override_bindings() -> Vec<*mut efi::protocols::driver_binding::Protocol> {
    let driver_binding_handles = match PROTOCOL_DB.locate_handles(Some(efi::protocols::driver_binding::PROTOCOL_GUID)) {
        Err(_) => return Vec::new(),
        Ok(handles) => handles,
    };

    let mut driver_override_map: BTreeMap<u32, efi::Handle> = BTreeMap::new();

    // insert all the handles that have DRIVER_FAMILY_OVERRIDE_PROTOCOL on them into a sorted map
    for handle in driver_binding_handles {
        match PROTOCOL_DB.get_interface_for_handle(handle, efi::protocols::driver_family_override::PROTOCOL_GUID) {
            Ok(protocol) => {
                let driver_override_protocol = unsafe {
                    (protocol as *mut efi::protocols::driver_family_override::Protocol)
                        .as_mut()
                        .expect("bad protocol ptr")
                };
                let version = (driver_override_protocol.get_version)(driver_override_protocol);
                driver_override_map.insert(version, handle);
            }
            Err(_) => continue,
        }
    }

    //return the driver bindings for the values from the map in reverse order (highest versions first)
    get_bindings_for_handles(driver_override_map.into_values().rev().collect())
}

fn get_bus_specific_override_bindings(
    controller_handle: efi::Handle,
) -> Vec<*mut efi::protocols::driver_binding::Protocol> {
    let bus_specific_override_protocol = match PROTOCOL_DB
        .get_interface_for_handle(controller_handle, efi::protocols::bus_specific_driver_override::PROTOCOL_GUID)
    {
        Err(_) => return Vec::new(),
        Ok(protocol) => unsafe {
            (protocol as *mut efi::protocols::bus_specific_driver_override::Protocol)
                .as_mut()
                .expect("bad protocol ptr")
        },
    };

    let mut bus_overrides = Vec::new();
    let mut driver_image_handle: efi::Handle = core::ptr::null_mut();
    loop {
        let status = (bus_specific_override_protocol.get_driver)(
            bus_specific_override_protocol,
            core::ptr::addr_of_mut!(driver_image_handle),
        );
        if status != efi::Status::SUCCESS {
            break;
        }
        bus_overrides.push(driver_image_handle);
    }

    get_bindings_for_handles(bus_overrides)
}

fn get_all_driver_bindings() -> Vec<*mut efi::protocols::driver_binding::Protocol> {
    let mut driver_bindings = match PROTOCOL_DB.locate_handles(Some(efi::protocols::driver_binding::PROTOCOL_GUID)) {
        Err(_) => return Vec::new(),
        Ok(handles) if handles.is_empty() => return Vec::new(),
        Ok(handles) => get_bindings_for_handles(handles),
    };

    driver_bindings.sort_unstable_by(|a, b| unsafe { (*(*b)).version.cmp(&(*(*a)).version) });

    driver_bindings
}

// authenticate a connect call through the security2 arch protocol
fn authenticate_connect(
    controller_handle: efi::Handle,
    remaining_device_path: Option<*mut efi::protocols::device_path::Protocol>,
    recursive: bool,
) -> Result<(), efi::Status> {
    if let Ok(device_path) =
        PROTOCOL_DB.get_interface_for_handle(controller_handle, efi::protocols::device_path::PROTOCOL_GUID)
    {
        let device_path = device_path as *mut efi::protocols::device_path::Protocol;
        if let Ok(security2_ptr) = PROTOCOL_DB.locate_protocol(mu_pi::protocols::security2::PROTOCOL_GUID) {
            let file_path = {
                if !recursive {
                    if let Some(remaining_path) = remaining_device_path {
                        concat_device_path_to_boxed_slice(device_path, remaining_path)
                    } else {
                        copy_device_path_to_boxed_slice(device_path)
                    }
                } else {
                    copy_device_path_to_boxed_slice(device_path)
                }
            };

            if let Ok(mut file_path) = file_path {
                let security2 = unsafe {
                    (security2_ptr as *mut mu_pi::protocols::security2::Protocol)
                        .as_ref()
                        .expect("security2 should not be null")
                };
                let security_status = (security2.file_authentication)(
                    security2_ptr as *mut _,
                    file_path.as_mut_ptr() as *mut _,
                    core::ptr::null_mut(),
                    0,
                    false,
                );
                if security_status != efi::Status::SUCCESS {
                    return Err(security_status);
                }
            }
        }
    }
    //if there is no device path on the controller handle,
    //or if there is no security2 protocol instance,
    //or any of the device paths are malformed,
    //then above will fall through to here, and no authentication is performed.
    Ok(())
}

fn core_connect_single_controller(
    controller_handle: efi::Handle,
    driver_handles: Vec<efi::Handle>,
    remaining_device_path: Option<*mut efi::protocols::device_path::Protocol>,
) -> Result<(), efi::Status> {
    PROTOCOL_DB.validate_handle(controller_handle)?;

    //The following sources for driver instances are considered per UEFI Spec 2.10 section 7.3.12:
    //1. Context Override
    let mut driver_candidates = Vec::new();
    driver_candidates.extend(get_bindings_for_handles(driver_handles));

    //2. Platform Driver Override
    let mut platform_override_drivers = get_platform_driver_override_bindings(controller_handle);
    platform_override_drivers.retain(|x| !driver_candidates.contains(x));
    driver_candidates.append(&mut platform_override_drivers);

    //3. Driver Family Override Search
    let mut family_override_drivers = get_family_override_bindings();
    family_override_drivers.retain(|x| !driver_candidates.contains(x));
    driver_candidates.append(&mut family_override_drivers);

    //4. Bus Specific Driver Override
    let mut bus_override_drivers = get_bus_specific_override_bindings(controller_handle);
    bus_override_drivers.retain(|x| !driver_candidates.contains(x));
    driver_candidates.append(&mut bus_override_drivers);

    //5. Driver Binding Search
    let mut driver_bindings = get_all_driver_bindings();
    driver_bindings.retain(|x| !driver_candidates.contains(x));
    driver_candidates.append(&mut driver_bindings);

    //loop until no more drivers can be started on handle.
    let mut one_started = false;
    loop {
        let mut started_drivers = Vec::new();
        for driver_binding_interface in driver_candidates.clone() {
            let driver_binding = unsafe { &mut *(driver_binding_interface) };
            let device_path = remaining_device_path.or(Some(core::ptr::null_mut())).expect("must be some");
            match (driver_binding.supported)(driver_binding_interface, controller_handle, device_path) {
                efi::Status::SUCCESS => {
                    //driver claims support; attempt to start it.
                    started_drivers.push(driver_binding_interface);
                    if (driver_binding.start)(driver_binding_interface, controller_handle, device_path)
                        == efi::Status::SUCCESS
                    {
                        one_started = true;
                    }
                }
                _ => continue,
            }
        }
        if started_drivers.is_empty() {
            break;
        }
        driver_candidates.retain(|x| !started_drivers.contains(x));
    }

    if one_started {
        return Ok(());
    }

    if let Some(device_path) = remaining_device_path {
        if unsafe { (*device_path).r#type == efi::protocols::device_path::TYPE_END } {
            return Ok(());
        }
    }

    Err(efi::Status::NOT_FOUND)
}

/// Connects a controller to drivers
///
/// This function matches the behavior of EFI_BOOT_SERVICES.ConnectController() API in the UEFI spec 2.10 section
/// 7.3.12. Refer to the UEFI spec description for details on input parameters, behavior, and error return codes.
///
/// # Safety
/// This routine cannot hold the protocol db lock while executing DriverBinding->Supported()/Start() since
/// they need to access protocol db services. That means this routine can't guarantee that driver bindings remain
/// valid for the duration of its execution. For example, if a driver were be unloaded in a timer callback after
/// returning true from Supported() before Start() is called, then the driver binding instance would be uninstalled or
/// invalid and Start() would be an invalid function pointer when invoked. In general, the spec implicitly assumes
/// that driver binding instances that are valid at the start of he call to ConnectController() must remain valid for
/// the duration of the ConnectController() call. If this is not true, then behavior is undefined. This function is
/// marked unsafe for this reason.
///
/// ## Example
///
/// ```ignore
/// let result = core_connect_controller(controller_handle, Vec::new(), None, false);
/// ```
///
pub unsafe fn core_connect_controller(
    handle: efi::Handle,
    driver_handles: Vec<efi::Handle>,
    remaining_device_path: Option<*mut efi::protocols::device_path::Protocol>,
    recursive: bool,
) -> Result<(), efi::Status> {
    authenticate_connect(handle, remaining_device_path, recursive)?;

    let return_status = core_connect_single_controller(handle, driver_handles, remaining_device_path);

    if recursive {
        for child in PROTOCOL_DB.get_child_handles(handle) {
            //ignore the return value to match behavior of edk2 reference.
            _ = core_connect_controller(child, Vec::new(), None, true);
        }
    }

    return_status
}

extern "efiapi" fn connect_controller(
    handle: efi::Handle,
    driver_image_handle: *mut efi::Handle,
    remaining_device_path: *mut efi::protocols::device_path::Protocol,
    recursive: efi::Boolean,
) -> efi::Status {
    let driver_handles = if driver_image_handle.is_null() {
        Vec::new()
    } else {
        let mut count = 0;
        let mut current_ptr = driver_image_handle;
        loop {
            let current_val = unsafe { *current_ptr };
            if current_val.is_null() {
                break;
            }
            count += 1;
            current_ptr = unsafe { current_ptr.add(1) };
        }
        let slice = unsafe { from_raw_parts_mut(driver_image_handle, count) };
        slice.to_vec().clone()
    };

    let device_path = NonNull::new(remaining_device_path).map(|x| x.as_ptr());
    unsafe {
        match core_connect_controller(handle, driver_handles, device_path, recursive.into()) {
            Err(err) => err,
            _ => efi::Status::SUCCESS,
        }
    }
}

/// Disconnects drivers from a controller.
///
/// This function matches the behavior of EFI_BOOT_SERVICES.DisconnectController() API in the UEFI spec 2.10 section
/// 7.3.13. Refer to the UEFI spec description for details on input parameters, behavior, and error return codes.
///
/// # Safety
/// This routine cannot hold the protocol db lock while executing DriverBinding->Supported()/Start() since
/// they need to access protocol db services. That means this routine can't guarantee that driver bindings remain
/// valid for the duration of its execution. For example, if a driver were be unloaded in a timer callback after
/// returning true from Supported() before Start() is called, then the driver binding instance would be uninstalled or
/// invalid and Start() would be an invalid function pointer when invoked. In general, the spec implicitly assumes
/// that driver binding instances that are valid at the start of he call to ConnectController() must remain valid for
/// the duration of the ConnectController() call. If this is not true, then behavior is undefined. This function is
/// marked unsafe for this reason.
///
/// ## Example
///
/// ```ignore
/// let result = core_disconnect_controller(controller_handle, None, None);
/// ```
///
pub unsafe fn core_disconnect_controller(
    controller_handle: efi::Handle,
    driver_image_handle: Option<efi::Handle>,
    child_handle: Option<efi::Handle>,
) -> Result<(), efi::Status> {
    PROTOCOL_DB.validate_handle(controller_handle)?;

    if let Some(handle) = driver_image_handle {
        PROTOCOL_DB.validate_handle(handle)?;
    }

    if let Some(handle) = child_handle {
        PROTOCOL_DB.validate_handle(handle)?;
    }

    // determine which driver_handles should be stopped.
    let mut drivers_managing_controller = {
        match PROTOCOL_DB.get_open_protocol_information(controller_handle) {
            Ok(info) => info
                .iter()
                .flat_map(|(_guid, open_info)| {
                    open_info.iter().filter_map(|x| {
                        if (x.attributes & efi::OPEN_PROTOCOL_BY_DRIVER) != 0 {
                            Some(x.agent_handle.expect("BY_DRIVER usage must have an agent handle"))
                        } else {
                            None
                        }
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    };

    drivers_managing_controller.sort_unstable();
    drivers_managing_controller.dedup();

    // if the driver image was specified, only disconnect that one (if it is actually managing it)
    if let Some(driver) = driver_image_handle {
        drivers_managing_controller.retain(|x| *x == driver);
    }

    let mut one_or_more_drivers_disconnected = false;
    let no_drivers = drivers_managing_controller.is_empty();
    for driver_handle in drivers_managing_controller {
        //determine which child handles should be stopped.
        let mut child_handles: Vec<_> = match PROTOCOL_DB.get_open_protocol_information(controller_handle) {
            Ok(info) => info
                .iter()
                .flat_map(|(_guid, open_info)| {
                    open_info.iter().filter_map(|x| {
                        if (x.agent_handle == Some(driver_handle))
                            && ((x.attributes & efi::OPEN_PROTOCOL_BY_CHILD_CONTROLLER) != 0)
                        {
                            Some(x.controller_handle.expect("controller handle required when open by child controller"))
                        } else {
                            None
                        }
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        child_handles.sort_unstable();
        child_handles.dedup();

        let total_children = child_handles.len();
        let mut is_only_child = false;
        if let Some(handle) = child_handle {
            //if the child was specified, but was the only child, then the driver should be disconnected.
            //if the child was specified, but other children were present, then the driver should not be disconnected.
            child_handles.retain(|x| x == &handle);
            is_only_child = total_children == child_handles.len();
        }

        //resolve the handle to the driver_binding.
        //N.B. Corner case: a driver could install a driver-binding instance; then be asked to manage a controller (and
        //thus, become an agent_handle in the open protocol information), and then something uninstalls the driver binding
        //from the agent_handle. This would mean that the agent_handle now no longer supports the driver binding but is
        //marked in the protocol database as managing the controller. This code just returns INVALID_PARAMETER in this case
        //(which effectively makes the controller "un-disconnect-able" since all subsequent disconnects will also fail for
        //the same reason). This matches the reference C implementation. As an enhancement, the core could track driver
        //bindings that are actively managing controllers and return an ACCESS_DENIED status if something attempts to
        //uninstall a binding that is in use.
        let driver_binding_interface = PROTOCOL_DB
            .get_interface_for_handle(driver_handle, efi::protocols::driver_binding::PROTOCOL_GUID)
            .or(Err(efi::Status::INVALID_PARAMETER))?;
        let driver_binding_interface = driver_binding_interface as *mut efi::protocols::driver_binding::Protocol;
        let driver_binding = unsafe { &mut *(driver_binding_interface) };

        let mut status = efi::Status::SUCCESS;
        if !child_handles.is_empty() {
            //disconnect the child controller(s).
            status = (driver_binding.stop)(
                driver_binding_interface,
                controller_handle,
                child_handles.len(),
                child_handles.as_mut_ptr(),
            );
        }
        if status == efi::Status::SUCCESS && (child_handle.is_none() || is_only_child) {
            status = (driver_binding.stop)(driver_binding_interface, controller_handle, 0, core::ptr::null_mut());
        }
        if status == efi::Status::SUCCESS {
            one_or_more_drivers_disconnected = true;
        }
    }

    if one_or_more_drivers_disconnected || no_drivers {
        Ok(())
    } else {
        Err(efi::Status::NOT_FOUND)
    }
}

extern "efiapi" fn disconnect_controller(
    controller_handle: efi::Handle,
    driver_image_handle: efi::Handle,
    child_handle: efi::Handle,
) -> efi::Status {
    let driver_image_handle = NonNull::new(driver_image_handle).map(|x| x.as_ptr());
    let child_handle = NonNull::new(child_handle).map(|x| x.as_ptr());
    unsafe {
        match core_disconnect_controller(controller_handle, driver_image_handle, child_handle) {
            Err(err) => err,
            _ => efi::Status::SUCCESS,
        }
    }
}

pub fn init_driver_services(bs: &mut efi::BootServices) {
    bs.connect_controller = connect_controller;
    bs.disconnect_controller = disconnect_controller;
}
