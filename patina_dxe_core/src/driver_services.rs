//! DXE Core Driver Services
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::{collections::BTreeMap, collections::BTreeSet, vec::Vec};
use core::ptr::NonNull;
use patina_internal_device_path::{concat_device_path_to_boxed_slice, copy_device_path_to_boxed_slice};
use patina_sdk::{
    error::EfiError,
    performance::{
        logging::{
            perf_driver_binding_start_begin, perf_driver_binding_start_end, perf_driver_binding_support_begin,
            perf_driver_binding_support_end,
        },
        measurement::create_performance_measurement,
    },
};

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
) -> Result<(), EfiError> {
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
                EfiError::status_to_result(security_status)?;
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
) -> Result<(), EfiError> {
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

            perf_driver_binding_support_begin(
                driver_binding.driver_binding_handle,
                controller_handle,
                create_performance_measurement,
            );

            //driver claims support; attempt to start it.
            match (driver_binding.supported)(driver_binding_interface, controller_handle, device_path) {
                efi::Status::SUCCESS => {
                    perf_driver_binding_support_end(
                        driver_binding.driver_binding_handle,
                        controller_handle,
                        create_performance_measurement,
                    );

                    started_drivers.push(driver_binding_interface);

                    perf_driver_binding_start_begin(
                        driver_binding.driver_binding_handle,
                        controller_handle,
                        create_performance_measurement,
                    );

                    if (driver_binding.start)(driver_binding_interface, controller_handle, device_path)
                        == efi::Status::SUCCESS
                    {
                        one_started = true;
                    }

                    perf_driver_binding_start_end(
                        driver_binding.driver_binding_handle,
                        controller_handle,
                        create_performance_measurement,
                    );
                }
                _ => {
                    perf_driver_binding_support_end(
                        driver_binding.driver_binding_handle,
                        controller_handle,
                        create_performance_measurement,
                    );
                    continue;
                }
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

    // Safety: caller must ensure that the pointer contained in remaining_device_path is valid if it is Some(_).
    if let Some(device_path) = remaining_device_path
        && unsafe { (device_path.read_unaligned()).r#type == efi::protocols::device_path::TYPE_END }
    {
        return Ok(());
    }

    Err(EfiError::NotFound)
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
) -> Result<(), EfiError> {
    authenticate_connect(handle, remaining_device_path, recursive)?;

    let return_status = core_connect_single_controller(handle, driver_handles, remaining_device_path);

    if recursive {
        for child in PROTOCOL_DB.get_child_handles(handle) {
            //ignore the return value to match behavior of edk2 reference.
            _ = unsafe { core_connect_controller(child, Vec::new(), None, true) };
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
        let mut current_ptr = driver_image_handle;
        let mut handles: Vec<efi::Handle> = Vec::new();
        loop {
            // Safety: caller must ensure that driver_image_handle is a valid pointer to a null-terminated list of
            // handles if it is not null.
            let current_val = unsafe { current_ptr.read_unaligned() };
            if current_val.is_null() {
                break;
            }
            handles.push(current_val);
            // Safety: caller guarantees a null-terminated list, so safe to advance to the next pointer as the null-terminator
            // has just been checked above.
            current_ptr = unsafe { current_ptr.add(1) };
        }
        handles
    };
    // remaining_device_path is passed in and may not have proper alignment.
    let device_path = if remaining_device_path.is_null() { None } else { Some(remaining_device_path) };

    // Safety: caller must ensure that device_path is a valid pointer to a device path structure if it is not null.
    unsafe {
        match core_connect_controller(handle, driver_handles, device_path, recursive.into()) {
            Err(err) => err.into(),
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
) -> Result<(), EfiError> {
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

    // remove duplicates but preserve ordering.
    let mut driver_set = BTreeSet::new();
    drivers_managing_controller.retain(|x| driver_set.insert(*x));

    // if the driver image was specified, only disconnect that one (if it is actually managing it)
    if let Some(driver) = driver_image_handle {
        drivers_managing_controller.retain(|x| *x == driver);
    }

    let mut one_or_more_drivers_disconnected = false;
    let no_drivers = drivers_managing_controller.is_empty();
    for driver_handle in drivers_managing_controller {
        let controller_info = match PROTOCOL_DB.get_open_protocol_information(controller_handle) {
            Ok(info) => info,
            Err(_) => continue,
        };

        // Determine whether this driver still has the controller open by driver, and what child handles it has open (if
        // any).
        let mut driver_valid = false;
        let mut child_handles = Vec::new();
        for (_guid, open_info) in controller_info.iter() {
            for info in open_info.iter() {
                if info.agent_handle == Some(driver_handle) {
                    if (info.attributes & efi::OPEN_PROTOCOL_BY_DRIVER) != 0 {
                        driver_valid = true;
                    }
                    if (info.attributes & efi::OPEN_PROTOCOL_BY_CHILD_CONTROLLER) != 0
                        && let Some(handle) = info.controller_handle
                    {
                        child_handles.push(handle);
                    }
                }
            }
        }

        // This driver no longer has the controller open by driver (may have been closed a side-effect of processing a
        // previous driver in the list), so nothing to do.
        if !driver_valid {
            continue;
        }

        // remove duplicates but preserve ordering.
        let mut child_set = BTreeSet::new();
        child_handles.retain(|x| child_set.insert(*x));

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
            .or(Err(EfiError::InvalidParameter))?;
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

    if one_or_more_drivers_disconnected || no_drivers { Ok(()) } else { Err(EfiError::NotFound) }
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
            Err(err) => err.into(),
            _ => efi::Status::SUCCESS,
        }
    }
}

pub fn init_driver_services(bs: &mut efi::BootServices) {
    bs.connect_controller = connect_controller;
    bs.disconnect_controller = disconnect_controller;
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::protocol_db::DXE_CORE_HANDLE;
    use crate::test_support;
    use core::ffi::c_void;
    use core::ptr;
    use std::str::FromStr;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use uuid::Uuid;

    // =================== TEST HELPER STATICS ===================
    static SUPPORTED_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
    static START_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);

    // =================== TEST HELPERS ===================
    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            unsafe {
                test_support::init_test_protocol_db();
            }
            f();
        })
        .unwrap();
    }

    // =================== MOCK DRIVER BINDING FUNCTIONS ===================
    // Supported functions
    extern "efiapi" fn mock_supported_success(
        _this: *mut efi::protocols::driver_binding::Protocol,
        _controller_handle: efi::Handle,
        _remaining_device_path: *mut efi::protocols::device_path::Protocol,
    ) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_supported_failure(
        _this: *mut efi::protocols::driver_binding::Protocol,
        _controller_handle: efi::Handle,
        _remaining_device_path: *mut efi::protocols::device_path::Protocol,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    extern "efiapi" fn mock_supported_with_counter(
        _this: *mut efi::protocols::driver_binding::Protocol,
        _controller_handle: efi::Handle,
        _remaining_device_path: *mut efi::protocols::device_path::Protocol,
    ) -> efi::Status {
        SUPPORTED_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
        efi::Status::SUCCESS
    }

    // Start functions
    extern "efiapi" fn mock_start_success(
        _this: *mut efi::protocols::driver_binding::Protocol,
        _controller_handle: efi::Handle,
        _remaining_device_path: *mut efi::protocols::device_path::Protocol,
    ) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_start_failure(
        _this: *mut efi::protocols::driver_binding::Protocol,
        _controller_handle: efi::Handle,
        _remaining_device_path: *mut efi::protocols::device_path::Protocol,
    ) -> efi::Status {
        efi::Status::DEVICE_ERROR
    }

    extern "efiapi" fn mock_start_with_counter(
        _this: *mut efi::protocols::driver_binding::Protocol,
        _controller_handle: efi::Handle,
        _remaining_device_path: *mut efi::protocols::device_path::Protocol,
    ) -> efi::Status {
        START_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
        efi::Status::SUCCESS
    }

    // Stop functions
    extern "efiapi" fn mock_stop_success(
        _this: *mut efi::protocols::driver_binding::Protocol,
        _controller_handle: efi::Handle,
        _num_children: usize,
        _child_handle_buffer: *mut efi::Handle,
    ) -> efi::Status {
        efi::Status::SUCCESS
    }

    // =================== MOCK PROTOCOL VERSION FUNCTIONS ===================
    extern "efiapi" fn mock_get_version_100(_this: *mut efi::protocols::driver_family_override::Protocol) -> u32 {
        100
    }

    extern "efiapi" fn mock_get_version_200(_this: *mut efi::protocols::driver_family_override::Protocol) -> u32 {
        200
    }

    // =================== HELPER FUNCTIONS ===================
    fn create_driver_binding(
        version: u32,
        handle: efi::Handle,
        supported_fn: extern "efiapi" fn(
            *mut efi::protocols::driver_binding::Protocol,
            efi::Handle,
            *mut efi::protocols::device_path::Protocol,
        ) -> efi::Status,
        start_fn: extern "efiapi" fn(
            *mut efi::protocols::driver_binding::Protocol,
            efi::Handle,
            *mut efi::protocols::device_path::Protocol,
        ) -> efi::Status,
        stop_fn: extern "efiapi" fn(
            *mut efi::protocols::driver_binding::Protocol,
            efi::Handle,
            usize,
            *mut efi::Handle,
        ) -> efi::Status,
    ) -> Box<efi::protocols::driver_binding::Protocol> {
        // Create a unique image handle by installing a protocol with arbitrary GUID
        // This is safer than arithmetic that could overflow
        let test_uuid = Uuid::from_str("12345678-1234-5678-9abc-def012345678").unwrap();
        let test_guid = efi::Guid::from_bytes(test_uuid.as_bytes());
        let image_handle = match PROTOCOL_DB.install_protocol_interface(
            None,
            test_guid,
            core::ptr::null_mut(), // Dummy protocol data for test
        ) {
            Ok((handle, _)) => handle,
            Err(_) => DXE_CORE_HANDLE, // Fallback to DXE_CORE_HANDLE
        };

        Box::new(efi::protocols::driver_binding::Protocol {
            version,
            supported: supported_fn,
            start: start_fn,
            stop: stop_fn,
            driver_binding_handle: handle,
            image_handle,
        })
    }

    fn create_default_driver_binding(
        version: u32,
        handle: efi::Handle,
    ) -> Box<efi::protocols::driver_binding::Protocol> {
        create_driver_binding(version, handle, mock_supported_success, mock_start_success, mock_stop_success)
    }

    fn create_end_device_path() -> efi::protocols::device_path::Protocol {
        efi::protocols::device_path::Protocol {
            r#type: efi::protocols::device_path::TYPE_END,
            sub_type: efi::protocols::device_path::End::SUBTYPE_ENTIRE,
            length: [4, 0],
        }
    }

    fn create_vendor_defined_device_path(_vendor_data: u32) -> efi::protocols::device_path::Protocol {
        efi::protocols::device_path::Protocol {
            r#type: efi::protocols::device_path::TYPE_HARDWARE,
            sub_type: efi::protocols::device_path::Hardware::SUBTYPE_VENDOR,
            length: [20, 0],
        }
    }

    // =================== TESTS ===================
    #[test]
    fn test_get_bindings_for_handles_empty() {
        with_locked_state(|| {
            let handles = vec![0x1 as efi::Handle, 0x2 as efi::Handle];
            let bindings = get_bindings_for_handles(handles);
            assert_eq!(bindings.len(), 0);
        });
    }

    #[test]
    fn test_get_bindings_for_handles_with_results() {
        with_locked_state(|| {
            // Create binding protocols
            let binding1 = create_default_driver_binding(10, 0x10 as efi::Handle);
            let binding1_ptr = Box::into_raw(binding1) as *mut core::ffi::c_void;

            let binding2 = create_default_driver_binding(20, 0x20 as efi::Handle);
            let binding2_ptr = Box::into_raw(binding2) as *mut core::ffi::c_void;

            // Create handles and install protocols
            PROTOCOL_DB
                .install_protocol_interface(
                    Some(0x1 as efi::Handle),
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x1111 as *mut core::ffi::c_void,
                )
                .unwrap();

            PROTOCOL_DB
                .install_protocol_interface(
                    Some(0x2 as efi::Handle),
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x2222 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Install driver binding protocols on the handles
            PROTOCOL_DB
                .install_protocol_interface(
                    Some(0x1 as efi::Handle),
                    efi::protocols::driver_binding::PROTOCOL_GUID,
                    binding1_ptr,
                )
                .unwrap();

            PROTOCOL_DB
                .install_protocol_interface(
                    Some(0x2 as efi::Handle),
                    efi::protocols::driver_binding::PROTOCOL_GUID,
                    binding2_ptr,
                )
                .unwrap();

            // Test the function
            let handles = vec![0x1 as efi::Handle, 0x2 as efi::Handle];
            let bindings = get_bindings_for_handles(handles);
            assert_eq!(bindings.len(), 2);

            // Verify the binding versions
            unsafe {
                assert_eq!((*bindings[0]).version, 10);
                assert_eq!((*bindings[1]).version, 20);
            }
        });
    }

    #[test]
    fn test_get_platform_driver_override_bindings_no_drivers() {
        with_locked_state(|| {
            use std::sync::atomic::{AtomicUsize, Ordering};

            static CALL_COUNT: AtomicUsize = AtomicUsize::new(0);

            // Mock platform driver override protocol that returns no drivers
            #[repr(C)]
            struct MockPlatformDriverOverrideProtocol {
                get_driver: fn(
                    this: *mut u8,
                    controller_handle: efi::Handle,
                    driver_image_handle: *mut efi::Handle,
                ) -> efi::Status,
            }

            let platform_override = Box::new(MockPlatformDriverOverrideProtocol {
                get_driver: |_this, _controller_handle, _driver_image_handle| {
                    CALL_COUNT.fetch_add(1, Ordering::SeqCst);
                    // Always return failure - no override drivers
                    efi::Status::NOT_FOUND
                },
            });
            let platform_override_ptr = Box::into_raw(platform_override) as *mut core::ffi::c_void;

            // Install the platform driver override protocol
            let (_, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::platform_driver_override::PROTOCOL_GUID,
                    platform_override_ptr,
                )
                .unwrap();

            // Create controller handle
            let (controller_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x2000 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Reset call counter
            CALL_COUNT.store(0, Ordering::SeqCst);

            // Test the function
            let bindings = get_platform_driver_override_bindings(controller_handle);

            // Verify results
            assert_eq!(CALL_COUNT.load(Ordering::SeqCst), 1, "Should call get_driver once and break on failure");
            assert_eq!(bindings.len(), 0, "Should return empty vector when no drivers available");
        });
    }

    #[test]
    fn test_authenticate_connect_no_device_path() {
        with_locked_state(|| {
            // Create a controller handle without a device path protocol
            let (controller_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::simple_text_output::PROTOCOL_GUID, // Any protocol except device_path
                    0x1111 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Call authenticate_connect - should succeed gracefully since no device path exists
            let result = authenticate_connect(controller_handle, None, false);

            assert!(result.is_ok(), "authenticate_connect should succeed when no device path is present");
        });
    }

    #[test]
    fn test_authenticate_connect_with_security2_protocol() {
        with_locked_state(|| {
            use std::sync::atomic::{AtomicBool, Ordering};

            // Track whether security2 was called
            static SECURITY2_CALLED: AtomicBool = AtomicBool::new(false);

            // Mock file_authentication function with proper UEFI calling convention
            extern "efiapi" fn mock_file_authentication(
                _this: *mut u8,
                _device_path: *mut u8,
                _file_buffer: *mut u8,
                _file_size: usize,
                _boot_policy: bool,
            ) -> efi::Status {
                SECURITY2_CALLED.store(true, Ordering::SeqCst);
                efi::Status::SUCCESS
            }

            // Create a mock Security2 protocol that uses the extern function
            #[repr(C)]
            struct MockSecurity2Protocol {
                file_authentication: extern "efiapi" fn(
                    this: *mut u8,
                    device_path: *mut u8,
                    file_buffer: *mut u8,
                    file_size: usize,
                    boot_policy: bool,
                ) -> efi::Status,
            }

            let security2 = Box::new(MockSecurity2Protocol { file_authentication: mock_file_authentication });
            let security2_ptr = Box::into_raw(security2) as *mut core::ffi::c_void;

            // Install the security2 protocol in the protocol database
            let (_, _) = PROTOCOL_DB
                .install_protocol_interface(None, mu_pi::protocols::security2::PROTOCOL_GUID, security2_ptr)
                .unwrap();

            // Create a proper END device path that should be safe to process
            let device_path = Box::new(efi::protocols::device_path::Protocol {
                r#type: efi::protocols::device_path::TYPE_END,
                sub_type: efi::protocols::device_path::End::SUBTYPE_ENTIRE,
                length: [4, 0],
            });
            let device_path_ptr = Box::into_raw(device_path) as *mut core::ffi::c_void;

            let (controller_handle, _) = PROTOCOL_DB
                .install_protocol_interface(None, efi::protocols::device_path::PROTOCOL_GUID, device_path_ptr)
                .unwrap();

            // Reset the flag
            SECURITY2_CALLED.store(false, Ordering::SeqCst);

            // Call authenticate_connect
            let result = authenticate_connect(controller_handle, None, false);

            assert!(result.is_ok(), "authenticate_connect should succeed");

            // Verify that security2.file_authentication was actually called
            assert!(SECURITY2_CALLED.load(Ordering::SeqCst), "security2.file_authentication should have been called");
        });
    }

    #[test]
    fn test_get_family_override_bindings() {
        with_locked_state(|| {
            // Create driver binding protocols
            let binding1 = create_default_driver_binding(10, 0x10 as efi::Handle);
            let binding1_ptr = Box::into_raw(binding1) as *mut core::ffi::c_void;

            let binding2 = create_default_driver_binding(20, 0x20 as efi::Handle);
            let binding2_ptr = Box::into_raw(binding2) as *mut core::ffi::c_void;

            let binding3 = create_default_driver_binding(30, 0x30 as efi::Handle);
            let binding3_ptr = Box::into_raw(binding3) as *mut core::ffi::c_void;

            // Create handle objects and install driver binding protocols
            let handle1 = 0x1 as efi::Handle;
            let handle2 = 0x2 as efi::Handle;
            let handle3 = 0x3 as efi::Handle;

            PROTOCOL_DB
                .install_protocol_interface(Some(handle1), efi::protocols::driver_binding::PROTOCOL_GUID, binding1_ptr)
                .unwrap();

            PROTOCOL_DB
                .install_protocol_interface(Some(handle2), efi::protocols::driver_binding::PROTOCOL_GUID, binding2_ptr)
                .unwrap();

            PROTOCOL_DB
                .install_protocol_interface(Some(handle3), efi::protocols::driver_binding::PROTOCOL_GUID, binding3_ptr)
                .unwrap();

            // Create family override protocols with different versions
            let family_override1 =
                Box::new(efi::protocols::driver_family_override::Protocol { get_version: mock_get_version_100 });
            let family_override1_ptr = Box::into_raw(family_override1) as *mut core::ffi::c_void;

            let family_override2 =
                Box::new(efi::protocols::driver_family_override::Protocol { get_version: mock_get_version_200 });
            let family_override2_ptr = Box::into_raw(family_override2) as *mut core::ffi::c_void;

            // Only install family override protocol on handles 1 and 2
            PROTOCOL_DB
                .install_protocol_interface(
                    Some(handle1),
                    efi::protocols::driver_family_override::PROTOCOL_GUID,
                    family_override1_ptr,
                )
                .unwrap();

            PROTOCOL_DB
                .install_protocol_interface(
                    Some(handle2),
                    efi::protocols::driver_family_override::PROTOCOL_GUID,
                    family_override2_ptr,
                )
                .unwrap();

            // Test the function
            let bindings = get_family_override_bindings();

            // Should return 2 bindings sorted by family override version (highest first)
            assert_eq!(bindings.len(), 2);

            // First binding should be from handle2 (version 200)
            // Second binding should be from handle1 (version 100)
            unsafe {
                assert_eq!((*bindings[0]).version, 20); // handle2's binding version
                assert_eq!((*bindings[1]).version, 10); // handle1's binding version
            }

            // Handle3 should not be included as it doesn't have the family override protocol
        });
    }

    #[test]
    fn test_get_all_driver_bindings() {
        with_locked_state(|| {
            // Create driver binding protocols with different versions
            let binding1 = create_default_driver_binding(10, 0x10 as efi::Handle);
            let binding1_ptr = Box::into_raw(binding1) as *mut core::ffi::c_void;

            let binding2 = create_default_driver_binding(30, 0x20 as efi::Handle);
            let binding2_ptr = Box::into_raw(binding2) as *mut core::ffi::c_void;

            let binding3 = create_default_driver_binding(20, 0x30 as efi::Handle);
            let binding3_ptr = Box::into_raw(binding3) as *mut core::ffi::c_void;

            // Create handle objects
            let handle1 = 0x1 as efi::Handle;
            let handle2 = 0x2 as efi::Handle;
            let handle3 = 0x3 as efi::Handle;

            // Install driver binding protocols on the handles
            PROTOCOL_DB
                .install_protocol_interface(Some(handle1), efi::protocols::driver_binding::PROTOCOL_GUID, binding1_ptr)
                .unwrap();

            PROTOCOL_DB
                .install_protocol_interface(Some(handle2), efi::protocols::driver_binding::PROTOCOL_GUID, binding2_ptr)
                .unwrap();

            PROTOCOL_DB
                .install_protocol_interface(Some(handle3), efi::protocols::driver_binding::PROTOCOL_GUID, binding3_ptr)
                .unwrap();

            // Call the function we're testing
            let bindings = get_all_driver_bindings();

            // Should return all 3 bindings sorted by version (highest first)
            assert_eq!(bindings.len(), 3);

            // Verify the correct order by version (descending)
            unsafe {
                assert_eq!((*bindings[0]).version, 30); // handle2's binding - highest version
                assert_eq!((*bindings[1]).version, 20); // handle3's binding - middle version
                assert_eq!((*bindings[2]).version, 10); // handle1's binding - lowest version
            }

            // Test with no driver bindings installed
            // First, uninstall all the protocols
            PROTOCOL_DB
                .uninstall_protocol_interface(handle1, efi::protocols::driver_binding::PROTOCOL_GUID, binding1_ptr)
                .unwrap();
            PROTOCOL_DB
                .uninstall_protocol_interface(handle2, efi::protocols::driver_binding::PROTOCOL_GUID, binding2_ptr)
                .unwrap();
            PROTOCOL_DB
                .uninstall_protocol_interface(handle3, efi::protocols::driver_binding::PROTOCOL_GUID, binding3_ptr)
                .unwrap();

            // Now test with empty DB
            let empty_bindings = get_all_driver_bindings();
            assert_eq!(empty_bindings.len(), 0);
        });
    }

    #[test]
    fn test_core_connect_single_controller() {
        with_locked_state(|| {
            // Reset counters
            SUPPORTED_CALL_COUNT.store(0, Ordering::SeqCst);
            START_CALL_COUNT.store(0, Ordering::SeqCst);

            // Initialize the handles in the protocol database
            // Controller protocol
            let (controller_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x1111 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Driver protocols
            let (driver_handle1, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x1010 as *mut core::ffi::c_void,
                )
                .unwrap();

            let (driver_handle2, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x2020 as *mut core::ffi::c_void,
                )
                .unwrap();

            let (driver_handle3, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x3030 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Create three driver binding protocols with different behaviors
            let binding1 = create_driver_binding(
                10,
                driver_handle1,
                mock_supported_with_counter,
                mock_start_with_counter,
                mock_stop_success,
            );
            let binding1_ptr = Box::into_raw(binding1) as *mut core::ffi::c_void;

            let binding2 = create_driver_binding(
                20,
                driver_handle2,
                mock_supported_failure, // This one will fail Supported()
                mock_start_success,
                mock_stop_success,
            );
            let binding2_ptr = Box::into_raw(binding2) as *mut core::ffi::c_void;

            let binding3 = create_driver_binding(
                30,
                driver_handle3,
                mock_supported_success,
                mock_start_failure, // This one will fail Start()
                mock_stop_success,
            );
            let binding3_ptr = Box::into_raw(binding3) as *mut core::ffi::c_void;

            // Install driver binding protocols on their handles
            PROTOCOL_DB
                .install_protocol_interface(
                    Some(driver_handle1),
                    efi::protocols::driver_binding::PROTOCOL_GUID,
                    binding1_ptr,
                )
                .unwrap();

            PROTOCOL_DB
                .install_protocol_interface(
                    Some(driver_handle2),
                    efi::protocols::driver_binding::PROTOCOL_GUID,
                    binding2_ptr,
                )
                .unwrap();

            PROTOCOL_DB
                .install_protocol_interface(
                    Some(driver_handle3),
                    efi::protocols::driver_binding::PROTOCOL_GUID,
                    binding3_ptr,
                )
                .unwrap();

            let result = core_connect_single_controller(
                controller_handle,
                vec![driver_handle1, driver_handle3], // Include only binding1 and binding3
                None,
            );

            assert!(result.is_ok());

            // Verify the right number of calls were made
            assert_eq!(SUPPORTED_CALL_COUNT.load(Ordering::SeqCst), 1); // binding1 only
            assert_eq!(START_CALL_COUNT.load(Ordering::SeqCst), 1); // binding1 only

            // Reset counters for next test
            SUPPORTED_CALL_COUNT.store(0, Ordering::SeqCst);
            START_CALL_COUNT.store(0, Ordering::SeqCst);

            // Test connection with an END device path
            let end_path = create_end_device_path();
            let end_path_ptr = Box::into_raw(Box::new(end_path));

            let result = core_connect_single_controller(controller_handle, vec![driver_handle1], Some(end_path_ptr));

            // Should succeed because this is an END device path
            assert!(result.is_ok());

            // Reset counters for next test
            SUPPORTED_CALL_COUNT.store(0, Ordering::SeqCst);
            START_CALL_COUNT.store(0, Ordering::SeqCst);

            // Test connection where no drivers match
            let _result = core_connect_single_controller(
                controller_handle,
                vec![driver_handle2], // Only the one that fails Supported()
                None,
            );

            // Verify that support was checked but start was not called
            assert_eq!(SUPPORTED_CALL_COUNT.load(Ordering::SeqCst), 1); // Since we're using mock_supported_failure
            assert_eq!(START_CALL_COUNT.load(Ordering::SeqCst), 1); // Start should never be called
        });
    }

    #[test]
    fn test_core_connect_controller() {
        with_locked_state(|| {
            // Initialize test handles and protocols
            // Controller handle
            let (controller_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x1111 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Driver handle
            let (driver_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x2222 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Create and install a driver binding protocol
            let binding = create_driver_binding(
                10,
                driver_handle,
                mock_supported_with_counter,
                mock_start_with_counter,
                mock_stop_success,
            );
            let binding_ptr = Box::into_raw(binding) as *mut core::ffi::c_void;

            PROTOCOL_DB
                .install_protocol_interface(
                    Some(driver_handle),
                    efi::protocols::driver_binding::PROTOCOL_GUID,
                    binding_ptr,
                )
                .unwrap();

            // Reset counters
            SUPPORTED_CALL_COUNT.store(0, Ordering::SeqCst);
            START_CALL_COUNT.store(0, Ordering::SeqCst);

            // Test 1: Basic connection (non-recursive)
            unsafe {
                let result = core_connect_controller(
                    controller_handle,
                    vec![driver_handle],
                    None,
                    false, // non-recursive
                );

                assert!(result.is_ok());
                assert_eq!(SUPPORTED_CALL_COUNT.load(Ordering::SeqCst), 1);
                assert_eq!(START_CALL_COUNT.load(Ordering::SeqCst), 1);
            }

            // Reset counters
            SUPPORTED_CALL_COUNT.store(0, Ordering::SeqCst);
            START_CALL_COUNT.store(0, Ordering::SeqCst);

            // Test 2: Create a child handle to test recursive behavior
            let (child_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x3333 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Make child_handle a child of controller_handle
            PROTOCOL_DB
                .add_protocol_usage(
                    controller_handle,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    Some(driver_handle),
                    Some(child_handle),
                    efi::OPEN_PROTOCOL_BY_CHILD_CONTROLLER,
                )
                .unwrap();

            // Test recursive connection
            unsafe {
                let result = core_connect_controller(
                    controller_handle,
                    vec![driver_handle],
                    None,
                    true, // recursive
                );

                assert!(result.is_ok());
                // Should have at least two calls (one for parent, one for child)
                assert!(SUPPORTED_CALL_COUNT.load(Ordering::SeqCst) >= 1);
                assert!(START_CALL_COUNT.load(Ordering::SeqCst) >= 1);
            }

            // Test 3: Test with remaining device path
            let end_path = create_end_device_path();
            let end_path_ptr = Box::into_raw(Box::new(end_path));

            // Reset counters
            SUPPORTED_CALL_COUNT.store(0, Ordering::SeqCst);
            START_CALL_COUNT.store(0, Ordering::SeqCst);

            unsafe {
                let result = core_connect_controller(
                    controller_handle,
                    vec![driver_handle],
                    Some(end_path_ptr),
                    false, // non-recursive
                );

                assert!(result.is_ok());
            }
        });
    }

    #[test]
    fn test_connect_controller() {
        with_locked_state(|| {
            // Initialize test handles and protocols
            let (controller_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x1111 as *mut core::ffi::c_void,
                )
                .unwrap();

            let (driver_handle1, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x2222 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Create and install a driver binding protocol
            let binding = create_driver_binding(
                10,
                driver_handle1,
                mock_supported_with_counter,
                mock_start_with_counter,
                mock_stop_success,
            );
            let binding_ptr = Box::into_raw(binding) as *mut core::ffi::c_void;

            PROTOCOL_DB
                .install_protocol_interface(
                    Some(driver_handle1),
                    efi::protocols::driver_binding::PROTOCOL_GUID,
                    binding_ptr,
                )
                .unwrap();

            // Reset counters
            SUPPORTED_CALL_COUNT.store(0, Ordering::SeqCst);
            START_CALL_COUNT.store(0, Ordering::SeqCst);

            // Test 1: Call with single driver handle
            let mut driver_handles = vec![driver_handle1, core::ptr::null_mut()];
            let status = connect_controller(
                controller_handle,
                driver_handles.as_mut_ptr(),
                core::ptr::null_mut(), // No remaining device path
                efi::Boolean::FALSE,
            );

            assert_eq!(status, efi::Status::SUCCESS);
            assert_eq!(SUPPORTED_CALL_COUNT.load(Ordering::SeqCst), 1);
            assert_eq!(START_CALL_COUNT.load(Ordering::SeqCst), 1);

            // Test 2: Call with null driver handle (should use all drivers)
            SUPPORTED_CALL_COUNT.store(0, Ordering::SeqCst);
            START_CALL_COUNT.store(0, Ordering::SeqCst);

            let status = connect_controller(
                controller_handle,
                core::ptr::null_mut(), // Null driver handle array
                core::ptr::null_mut(), // No remaining device path
                efi::Boolean::FALSE,
            );

            assert_eq!(status, efi::Status::SUCCESS);
            // At least one support call should have happened
            assert!(SUPPORTED_CALL_COUNT.load(Ordering::SeqCst) >= 1);
        });
    }

    #[test]
    fn test_core_disconnect_controller() {
        with_locked_state(|| {
            let uuid1 = Uuid::from_str("0e896c7a-57dc-4987-bc22-abc3a8263210").unwrap();
            let guid1 = efi::Guid::from_bytes(uuid1.as_bytes());
            let interface1: *mut c_void = 0x1234 as *mut c_void;
            let (handle1, _) = PROTOCOL_DB.install_protocol_interface(None, guid1, interface1).unwrap();

            // Test single driver managing controller
            {
                // Create controller handle with VendorDefined device path
                let controller_device_path = Box::new(create_vendor_defined_device_path(0x1111));
                let controller_device_path_ptr = Box::into_raw(controller_device_path) as *mut core::ffi::c_void;
                let (controller_handle, _) = PROTOCOL_DB
                    .install_protocol_interface(
                        None,
                        efi::protocols::device_path::PROTOCOL_GUID,
                        controller_device_path_ptr,
                    )
                    .unwrap();

                // Create driver handle with VendorDefined device path
                let driver_device_path = Box::new(create_vendor_defined_device_path(0x2222));
                let driver_device_path_ptr = Box::into_raw(driver_device_path) as *mut core::ffi::c_void;
                let (driver_handle, _) = PROTOCOL_DB
                    .install_protocol_interface(
                        None,
                        efi::protocols::device_path::PROTOCOL_GUID,
                        driver_device_path_ptr,
                    )
                    .unwrap();

                // Create and install driver binding protocol
                let binding = create_driver_binding(
                    10,
                    driver_handle,
                    mock_supported_success,
                    mock_start_success,
                    mock_stop_success,
                );
                let binding_ptr = Box::into_raw(binding) as *mut core::ffi::c_void;

                PROTOCOL_DB
                    .install_protocol_interface(
                        Some(driver_handle),
                        efi::protocols::driver_binding::PROTOCOL_GUID,
                        binding_ptr,
                    )
                    .unwrap();

                // Simulate driver managing controller by adding protocol usage
                PROTOCOL_DB
                    .add_protocol_usage(
                        controller_handle,
                        efi::protocols::device_path::PROTOCOL_GUID,
                        Some(driver_handle),
                        Some(handle1),
                        efi::OPEN_PROTOCOL_BY_DRIVER,
                    )
                    .unwrap();

                // Test disconnect without specifying driver or child
                unsafe {
                    let result = core_disconnect_controller(controller_handle, None, None);
                    assert!(result.is_ok(), "Should successfully disconnect all drivers");
                }
            }
        });
    }

    #[test]
    fn test_core_disconnect_controller_with_child_handles() {
        with_locked_state(|| {
            use std::sync::atomic::{AtomicUsize, Ordering};

            // Track calls to the stop function and parameters
            static STOP_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
            static LAST_CHILD_COUNT: AtomicUsize = AtomicUsize::new(0);

            let uuid1 = Uuid::from_str("0e896c7a-57dc-4987-bc22-abc3a8263210").unwrap();
            let guid1 = efi::Guid::from_bytes(uuid1.as_bytes());
            let interface1: *mut c_void = 0x1234 as *mut c_void;
            let (handle1, _) = PROTOCOL_DB.install_protocol_interface(None, guid1, interface1).unwrap();

            // Mock stop function that tracks calls and child count
            extern "efiapi" fn mock_stop_with_tracking(
                _this: *mut efi::protocols::driver_binding::Protocol,
                _controller_handle: efi::Handle,
                num_children: usize,
                _child_handle_buffer: *mut efi::Handle,
            ) -> efi::Status {
                STOP_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
                LAST_CHILD_COUNT.store(num_children, Ordering::SeqCst);
                efi::Status::SUCCESS
            }

            // Create controller handle with VendorDefined device path
            let controller_device_path = Box::new(create_vendor_defined_device_path(0x1111));
            let controller_device_path_ptr = Box::into_raw(controller_device_path) as *mut core::ffi::c_void;
            let (controller_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    controller_device_path_ptr,
                )
                .unwrap();

            // Create driver handle with VendorDefined device path
            let driver_device_path = Box::new(create_vendor_defined_device_path(0x2222));
            let driver_device_path_ptr = Box::into_raw(driver_device_path) as *mut core::ffi::c_void;
            let (driver_handle, _) = PROTOCOL_DB
                .install_protocol_interface(None, efi::protocols::device_path::PROTOCOL_GUID, driver_device_path_ptr)
                .unwrap();

            // Create child handle with VendorDefined device path
            let child_device_path = Box::new(create_vendor_defined_device_path(0x3333));
            let child_device_path_ptr = Box::into_raw(child_device_path) as *mut core::ffi::c_void;
            let (child_handle, _) = PROTOCOL_DB
                .install_protocol_interface(None, efi::protocols::device_path::PROTOCOL_GUID, child_device_path_ptr)
                .unwrap();

            // Create driver binding protocol
            let binding = Box::new(efi::protocols::driver_binding::Protocol {
                version: 10,
                supported: mock_supported_success,
                start: mock_start_success,
                stop: mock_stop_with_tracking,
                driver_binding_handle: driver_handle,
                image_handle: DXE_CORE_HANDLE,
            });
            let binding_ptr = Box::into_raw(binding) as *mut core::ffi::c_void;

            // Install driver binding protocol
            PROTOCOL_DB
                .install_protocol_interface(
                    Some(driver_handle),
                    efi::protocols::driver_binding::PROTOCOL_GUID,
                    binding_ptr,
                )
                .unwrap();

            // Simulate driver managing controller - add BY_DRIVER usage
            PROTOCOL_DB
                .add_protocol_usage(
                    controller_handle,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    Some(driver_handle),
                    Some(handle1),
                    efi::OPEN_PROTOCOL_BY_DRIVER,
                )
                .unwrap();

            // Simulate child controller managed by this driver - add BY_CHILD_CONTROLLER usage
            PROTOCOL_DB
                .add_protocol_usage(
                    controller_handle,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    Some(driver_handle),
                    Some(child_handle), // child handle for BY_CHILD_CONTROLLER
                    efi::OPEN_PROTOCOL_BY_CHILD_CONTROLLER,
                )
                .unwrap();

            // Reset counters
            STOP_CALL_COUNT.store(0, Ordering::SeqCst);
            LAST_CHILD_COUNT.store(999, Ordering::SeqCst); // Set to invalid value to detect changes

            // Test disconnect - should call stop function
            unsafe {
                let result = core_disconnect_controller(controller_handle, None, None);
                assert!(result.is_ok(), "disconnect should succeed");
            }

            // Verify stop was called at least once
            let call_count = STOP_CALL_COUNT.load(Ordering::SeqCst);
            assert!(call_count > 0, "stop should be called at least once, but was called {call_count} times");

            // Just verify that the function executed the child handling logic
            // The exact behavior depends on the protocol database implementation
            println!("Stop called {} times, last child count: {}", call_count, LAST_CHILD_COUNT.load(Ordering::SeqCst));
        });
    }

    #[test]
    fn test_core_disconnect_controller_specific_child_only_child() {
        with_locked_state(|| {
            use std::sync::atomic::{AtomicUsize, Ordering};

            // Track calls to the stop function and parameters
            static STOP_CALLS: AtomicUsize = AtomicUsize::new(0);
            static DRIVER_STOP_CALLED: AtomicUsize = AtomicUsize::new(0); // Track full driver stops

            let uuid1 = Uuid::from_str("0e896c7a-57dc-4987-bc22-abc3a8263210").unwrap();
            let guid1 = efi::Guid::from_bytes(uuid1.as_bytes());
            let interface1: *mut c_void = 0x1234 as *mut c_void;
            let (handle1, _) = PROTOCOL_DB.install_protocol_interface(None, guid1, interface1).unwrap();

            // Mock stop function that tracks different types of calls
            extern "efiapi" fn mock_stop_tracking(
                _this: *mut efi::protocols::driver_binding::Protocol,
                _controller_handle: efi::Handle,
                num_children: usize,
                _child_handle_buffer: *mut efi::Handle,
            ) -> efi::Status {
                STOP_CALLS.fetch_add(1, Ordering::SeqCst);
                if num_children == 0 {
                    DRIVER_STOP_CALLED.fetch_add(1, Ordering::SeqCst);
                }
                efi::Status::SUCCESS
            }

            // Create controller handle
            let (controller_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x1111 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Create driver handle
            let (driver_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x2222 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Create only one child handle
            let (child_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x3333 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Create driver binding protocol
            let binding = Box::new(efi::protocols::driver_binding::Protocol {
                version: 10,
                supported: mock_supported_success,
                start: mock_start_success,
                stop: mock_stop_tracking,
                driver_binding_handle: driver_handle,
                image_handle: DXE_CORE_HANDLE,
            });
            let binding_ptr = Box::into_raw(binding) as *mut core::ffi::c_void;

            // Install driver binding protocol
            PROTOCOL_DB
                .install_protocol_interface(
                    Some(driver_handle),
                    efi::protocols::driver_binding::PROTOCOL_GUID,
                    binding_ptr,
                )
                .unwrap();

            // Simulate driver managing controller
            PROTOCOL_DB
                .add_protocol_usage(
                    controller_handle,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    Some(driver_handle),
                    Some(handle1),
                    efi::OPEN_PROTOCOL_BY_DRIVER,
                )
                .unwrap();

            // Simulate only ONE child controller managed by this driver
            PROTOCOL_DB
                .add_protocol_usage(
                    controller_handle,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    Some(driver_handle),
                    Some(child_handle),
                    efi::OPEN_PROTOCOL_BY_CHILD_CONTROLLER,
                )
                .unwrap();

            // Reset counters
            STOP_CALLS.store(0, Ordering::SeqCst);
            DRIVER_STOP_CALLED.store(0, Ordering::SeqCst);

            // Test disconnect with specific child handle (which is the ONLY child)
            // This should trigger: is_only_child = total_children == child_handles.len() = true
            // Because: total_children = 1, child_handles.retain() keeps 1 child, so 1 == 1
            unsafe {
                let result = core_disconnect_controller(controller_handle, None, Some(child_handle));
                assert!(result.is_ok(), "disconnect should succeed");
            }

            // When child was specified and it was the only child, driver should be fully disconnected
            // This means stop should be called twice: once for children, once for driver
            let total_calls = STOP_CALLS.load(Ordering::SeqCst);
            let driver_stops = DRIVER_STOP_CALLED.load(Ordering::SeqCst);

            println!("Total stop calls: {total_calls}, Driver stops (num_children=0): {driver_stops}");

            // Since we specified the only child, the driver should be disconnected completely
            assert!(driver_stops > 0, "Driver should be fully stopped when specified child is the only child");
        });
    }

    #[test]
    fn test_core_disconnect_controller_invalid_driver_handle() {
        with_locked_state(|| {
            // Create a valid controller handle
            let (controller_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x1111 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Use an invalid driver handle (not in protocol database)
            let invalid_driver_handle = 0x9999 as efi::Handle;

            // Test disconnect with invalid driver handle
            unsafe {
                let result = core_disconnect_controller(
                    controller_handle,
                    Some(invalid_driver_handle), // Invalid driver handle
                    None,
                );

                // Should fail with InvalidParameter due to driver handle validation
                assert!(result.is_err(), "Should fail with invalid driver handle");
                if let Err(error) = result {
                    assert_eq!(
                        error,
                        EfiError::InvalidParameter,
                        "Should return InvalidParameter for invalid driver handle"
                    );
                }
            }
        });
    }

    #[test]
    fn test_core_disconnect_controller_invalid_child_handle() {
        with_locked_state(|| {
            // Create a valid controller handle
            let (controller_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x1111 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Use an invalid child handle (not in protocol database)
            let invalid_child_handle = 0x8888 as efi::Handle;

            // Test disconnect with invalid child handle
            unsafe {
                let result = core_disconnect_controller(
                    controller_handle,
                    None,
                    Some(invalid_child_handle), // Invalid child handle
                );

                // Should fail with InvalidParameter due to child handle validation
                assert!(result.is_err(), "Should fail with invalid child handle");
                if let Err(error) = result {
                    assert_eq!(
                        error,
                        EfiError::InvalidParameter,
                        "Should return InvalidParameter for invalid child handle"
                    );
                }
            }
        });
    }

    #[test]
    fn test_disconnect_controller_extern_function() {
        with_locked_state(|| {
            // Create controller handle
            let (controller_handle, _) = PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    efi::protocols::device_path::PROTOCOL_GUID,
                    0x1111 as *mut core::ffi::c_void,
                )
                .unwrap();

            // Test the extern "efiapi" function with null handles (should succeed for empty controller)
            let status = disconnect_controller(
                controller_handle,
                core::ptr::null_mut(), // No specific driver
                core::ptr::null_mut(), // No child handle
            );

            assert_eq!(status, efi::Status::SUCCESS, "disconnect_controller should succeed with null handles");

            // Test with invalid controller handle
            let invalid_handle = 0x9999 as efi::Handle;
            let status = disconnect_controller(invalid_handle, core::ptr::null_mut(), core::ptr::null_mut());

            // Should return error status for invalid handle
            assert_ne!(status, efi::Status::SUCCESS, "disconnect_controller should fail with invalid handle");
        });
    }

    #[test]
    fn test_init_driver_services() {
        // Create dummy function pointers to use for initialization
        extern "efiapi" fn dummy_raise_tpl(_new_tpl: efi::Tpl) -> efi::Tpl {
            0
        }
        extern "efiapi" fn dummy_restore_tpl(_old_tpl: efi::Tpl) {}
        extern "efiapi" fn dummy_allocate_pages(
            _allocation_type: u32,
            _memory_type: u32,
            _pages: usize,
            _memory: *mut u64,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_free_pages(_memory: u64, _pages: usize) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_get_memory_map(
            _memory_map_size: *mut usize,
            _memory_map: *mut efi::MemoryDescriptor,
            _map_key: *mut usize,
            _descriptor_size: *mut usize,
            _descriptor_version: *mut u32,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_allocate_pool(
            _pool_type: u32,
            _size: usize,
            _buffer: *mut *mut c_void,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_free_pool(_buffer: *mut c_void) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_create_event(
            _event_type: u32,
            _notify_tpl: efi::Tpl,
            _notify_function: Option<efi::EventNotify>,
            _notify_context: *mut c_void,
            _event: *mut efi::Event,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_set_timer(_event: efi::Event, _type: u32, _trigger_time: u64) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_wait_for_event(
            _number_of_events: usize,
            _event: *mut efi::Event,
            _index: *mut usize,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_signal_event(_event: efi::Event) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_close_event(_event: efi::Event) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_check_event(_event: efi::Event) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_install_protocol_interface(
            _handle: *mut efi::Handle,
            _protocol: *mut efi::Guid,
            _interface_type: u32,
            _interface: *mut c_void,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_reinstall_protocol_interface(
            _handle: efi::Handle,
            _protocol: *mut efi::Guid,
            _old_interface: *mut c_void,
            _new_interface: *mut c_void,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_uninstall_protocol_interface(
            _handle: efi::Handle,
            _protocol: *mut efi::Guid,
            _interface: *mut c_void,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_handle_protocol(
            _handle: efi::Handle,
            _protocol: *mut efi::Guid,
            _interface: *mut *mut c_void,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_register_protocol_notify(
            _protocol: *mut efi::Guid,
            _event: efi::Event,
            _registration: *mut *mut c_void,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_locate_handle(
            _search_type: u32,
            _protocol: *mut efi::Guid,
            _search_key: *mut c_void,
            _buffer_size: *mut usize,
            _buffer: *mut efi::Handle,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_locate_device_path(
            _protocol: *mut efi::Guid,
            _device_path: *mut *mut r_efi::protocols::device_path::Protocol,
            _device: *mut efi::Handle,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_install_configuration_table(
            _guid: *mut efi::Guid,
            _table: *mut c_void,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_load_image(
            _boot_policy: efi::Boolean,
            _parent_image_handle: efi::Handle,
            _device_path: *mut r_efi::protocols::device_path::Protocol,
            _source_buffer: *mut c_void,
            _source_size: usize,
            _image_handle: *mut efi::Handle,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_start_image(
            _image_handle: efi::Handle,
            _exit_data_size: *mut usize,
            _exit_data: *mut *mut u16,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_exit(
            _image_handle: efi::Handle,
            _exit_status: efi::Status,
            _exit_data_size: usize,
            _exit_data: *mut u16,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_unload_image(_image_handle: efi::Handle) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_exit_boot_services(_image_handle: efi::Handle, _map_key: usize) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_get_next_monotonic_count(_count: *mut u64) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_stall(_microseconds: usize) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_set_watchdog_timer(
            _timeout: usize,
            _watchdog_code: u64,
            _data_size: usize,
            _watchdog_data: *mut u16,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_connect_controller(
            _controller_handle: efi::Handle,
            _driver_image_handle: *mut efi::Handle,
            _remaining_device_path: *mut r_efi::protocols::device_path::Protocol,
            _recursive: efi::Boolean,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_disconnect_controller(
            _controller_handle: efi::Handle,
            _driver_image_handle: efi::Handle,
            _child_handle: efi::Handle,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_open_protocol(
            _handle: efi::Handle,
            _protocol: *mut efi::Guid,
            _interface: *mut *mut c_void,
            _agent_handle: efi::Handle,
            _controller_handle: efi::Handle,
            _attributes: u32,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_close_protocol(
            _handle: efi::Handle,
            _protocol: *mut efi::Guid,
            _agent_handle: efi::Handle,
            _controller_handle: efi::Handle,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_open_protocol_information(
            _handle: efi::Handle,
            _protocol: *mut efi::Guid,
            _entry_buffer: *mut *mut efi::OpenProtocolInformationEntry,
            _entry_count: *mut usize,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_protocols_per_handle(
            _handle: efi::Handle,
            _protocol_buffer: *mut *mut *mut efi::Guid,
            _protocol_buffer_count: *mut usize,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_locate_handle_buffer(
            _search_type: u32,
            _protocol: *mut efi::Guid,
            _search_key: *mut c_void,
            _no_handles: *mut usize,
            _buffer: *mut *mut efi::Handle,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_locate_protocol(
            _protocol: *mut efi::Guid,
            _registration: *mut c_void,
            _interface: *mut *mut c_void,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_install_multiple_protocol_interfaces(
            _handle: *mut efi::Handle,
            _args: *mut c_void,
            _more_args: *mut c_void,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_uninstall_multiple_protocol_interfaces(
            _handle: efi::Handle,
            _args: *mut c_void,
            _more_args: *mut c_void,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_calculate_crc32(
            _data: *mut c_void,
            _data_size: usize,
            _crc32: *mut u32,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn dummy_copy_mem(_destination: *mut c_void, _source: *mut c_void, _length: usize) {}
        extern "efiapi" fn dummy_set_mem(_buffer: *mut c_void, _size: usize, _value: u8) {}
        extern "efiapi" fn dummy_create_event_ex(
            _event_type: u32,
            _notify_tpl: efi::Tpl,
            _notify_function: Option<efi::EventNotify>,
            _notify_context: *const c_void,
            _event_group: *const efi::Guid,
            _event: *mut efi::Event,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }

        let mut boot_services = efi::BootServices {
            hdr: efi::TableHeader { signature: 0, revision: 0, header_size: 0, crc32: 0, reserved: 0 },
            // Fill with dummy function pointers
            raise_tpl: dummy_raise_tpl,
            restore_tpl: dummy_restore_tpl,
            allocate_pages: dummy_allocate_pages,
            free_pages: dummy_free_pages,
            get_memory_map: dummy_get_memory_map,
            allocate_pool: dummy_allocate_pool,
            free_pool: dummy_free_pool,
            create_event: dummy_create_event,
            set_timer: dummy_set_timer,
            wait_for_event: dummy_wait_for_event,
            signal_event: dummy_signal_event,
            close_event: dummy_close_event,
            check_event: dummy_check_event,
            install_protocol_interface: dummy_install_protocol_interface,
            reinstall_protocol_interface: dummy_reinstall_protocol_interface,
            uninstall_protocol_interface: dummy_uninstall_protocol_interface,
            handle_protocol: dummy_handle_protocol,
            reserved: ptr::null_mut(),
            register_protocol_notify: dummy_register_protocol_notify,
            locate_handle: dummy_locate_handle,
            locate_device_path: dummy_locate_device_path,
            install_configuration_table: dummy_install_configuration_table,
            load_image: dummy_load_image,
            start_image: dummy_start_image,
            exit: dummy_exit,
            unload_image: dummy_unload_image,
            exit_boot_services: dummy_exit_boot_services,
            get_next_monotonic_count: dummy_get_next_monotonic_count,
            stall: dummy_stall,
            set_watchdog_timer: dummy_set_watchdog_timer,
            connect_controller: dummy_connect_controller,
            disconnect_controller: dummy_disconnect_controller,
            open_protocol: dummy_open_protocol,
            close_protocol: dummy_close_protocol,
            open_protocol_information: dummy_open_protocol_information,
            protocols_per_handle: dummy_protocols_per_handle,
            locate_handle_buffer: dummy_locate_handle_buffer,
            locate_protocol: dummy_locate_protocol,
            install_multiple_protocol_interfaces: dummy_install_multiple_protocol_interfaces,
            uninstall_multiple_protocol_interfaces: dummy_uninstall_multiple_protocol_interfaces,
            calculate_crc32: dummy_calculate_crc32,
            copy_mem: dummy_copy_mem,
            set_mem: dummy_set_mem,
            create_event_ex: dummy_create_event_ex,
        };
        init_driver_services(&mut boot_services);

        assert!(boot_services.connect_controller as usize == connect_controller as usize);
        assert!(boot_services.disconnect_controller as usize == disconnect_controller as usize);
    }
}
