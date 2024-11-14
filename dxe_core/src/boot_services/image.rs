extern crate alloc;

use alloc::{boxed::Box, string::String};
use core::{slice::from_raw_parts, ffi::c_void};
use corosensei::{Coroutine, CoroutineResult, ScopedCoroutine, Yielder};
use r_efi::efi;
use uefi_device_path::{copy_device_path_to_boxed_slice, device_path_node_count};

use crate::{boot_services::{with_protocol_db, BootServices}, image::{remove_image_memory_protections, core_load_pe_image, get_buffer_by_file_path, empty_image_info, DxeCoreGlobalImageData, ImageStack, ENTRY_POINT_STACK_SIZE}, protocol_db::ProtocolDb};

// Loads the image specified by the device_path (not yet supported) or
// source_buffer argument. See EFI_BOOT_SERVICES::LoadImage() API definition
// in UEFI spec for usage details.
// * boot_policy - indicates whether the image is being loaded by the boot
//                 manager from the specified device path. ignored if
//                 source_buffer is not null.
// * parent_image_handle - the caller's image handle.
// * device_path - the file path from which the image is loaded.
// * source_buffer - if not null, pointer to the memory location containing the
//                   image to be loaded.
//  * source_size - size in bytes of source_buffer. ignored if source_buffer is
//                  null.
//  * image_handle - pointer to the returned image handle that is created on
//                   successful image load.
pub fn load_image(
    private_image_data: &tpl_lock::TplMutex<DxeCoreGlobalImageData>,
    boot_policy: efi::Boolean,
    parent_image_handle: efi::Handle,
    device_path: *mut efi::protocols::device_path::Protocol,
    source_buffer: *mut c_void,
    source_size: usize,
    image_handle: *mut efi::Handle,
) -> efi::Status {
    if image_handle.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let image = if source_buffer.is_null() {
        None
    } else {
        if source_size == 0 {
            return efi::Status::LOAD_ERROR;
        }
        Some(unsafe { from_raw_parts(source_buffer as *const u8, source_size) })
    };

    match core_load_image(private_image_data, boot_policy.into(), parent_image_handle, device_path, image) {
        Err(err) => return err,
        Ok(handle) => unsafe { image_handle.write(handle) },
    }

    efi::Status::SUCCESS
}

/// Loads the image specified by the device path (not yet supported) or slice.
/// * parent_image_handle - the handle of the image that is loading this one.
/// * device_path - optional device path describing where to load the image from.
/// * image - optional slice containing the image data.
///
/// One of `device_path` or `image` must be specified.
/// returns the image handle of the freshly loaded image.
pub fn core_load_image(
    private_image_data: &tpl_lock::TplMutex<DxeCoreGlobalImageData>,
    boot_policy: bool,
    parent_image_handle: efi::Handle,
    device_path: *mut efi::protocols::device_path::Protocol,
    image: Option<&[u8]>,
) -> Result<efi::Handle, efi::Status> {
    if image.is_none() && device_path.is_null() {
        log::error!("failed to load image: image is none or device path is null.");
        return Err(efi::Status::INVALID_PARAMETER);
    }

    with_protocol_db!(|db| db
        .validate_handle(parent_image_handle)
        .inspect_err(|err| log::error!("failed to load image: invalid handle: {:#x?}", err))
    )?;

    with_protocol_db!(|db| db
        .get_interface_for_handle(parent_image_handle, efi::protocols::loaded_image::PROTOCOL_GUID)
        .inspect_err(|err| log::error!("failed to load image: failed to get loaded image interface: {:#x?}", err))
        .map_err(|_| efi::Status::INVALID_PARAMETER)
    )?;

    let image_to_load = match image {
        Some(image) => image.to_vec(),
        None => get_buffer_by_file_path(boot_policy, device_path)?,
    };

    //TODO: image authentication not implemented yet.

    // load the image.
    let mut image_info = empty_image_info();
    image_info.system_table = private_image_data.lock().system_table;
    image_info.parent_handle = parent_image_handle;

    if !device_path.is_null() {
        if let Ok((_, handle)) = BootServices::core_locate_device_path(efi::protocols::device_path::PROTOCOL_GUID, device_path) {
            image_info.device_handle = handle;
        }

        // extract file path here and set in image_info
        let (_, device_path_size) = device_path_node_count(device_path)?;
        let file_path_size: usize =
            device_path_size.saturating_sub(core::mem::size_of::<efi::protocols::device_path::Protocol>());
        let file_path = unsafe { (device_path as *const u8).add(file_path_size) };
        image_info.file_path = file_path as *mut efi::protocols::device_path::Protocol;
    }

    let mut private_info = core_load_pe_image(image_to_load.as_ref(), image_info)
        .inspect_err(|err| log::error!("failed to load image: core_load_pe_image failed: {:#x?}", err))?;

    let image_info_ptr = private_info.image_info.as_ref() as *const efi::protocols::loaded_image::Protocol;
    let image_info_ptr = image_info_ptr as *mut c_void;

    log::info!(
        "Loaded driver at {:#x?} EntryPoint={:#x?} {:}",
        private_info.image_info.image_base,
        private_info.entry_point as usize,
        private_info.pe_info.filename.as_ref().unwrap_or(&String::from("<no PDB>"))
    );

    // install the loaded_image protocol for this freshly loaded image on a new
    // handle.
    let handle = BootServices::core_install_protocol_interface(None, efi::protocols::loaded_image::PROTOCOL_GUID, image_info_ptr)
        .inspect_err(|err| log::error!("failed to load image: install loaded image protocol failed: {:#x?}", err))?;

    // install the loaded_image device path protocol for the new image. If input device path is not null, then make a
    // permanent copy on the heap.
    let loaded_image_device_path = if device_path.is_null() {
        core::ptr::null_mut()
    } else {
        // make copy and convert to raw pointer to avoid drop at end of function.
        Box::into_raw(copy_device_path_to_boxed_slice(device_path)?) as *mut u8
    };

    BootServices::core_install_protocol_interface(
        Some(handle),
        efi::protocols::loaded_image_device_path::PROTOCOL_GUID,
        loaded_image_device_path as *mut c_void,
    )
    .inspect_err(|err| log::error!("failed to load image: install device path failed: {:#x?}", err))?;

    if let Some(res_section) = private_info.hii_resource_section {
        BootServices::core_install_protocol_interface(
            Some(handle),
            efi::protocols::hii_package_list::PROTOCOL_GUID,
            res_section as *mut c_void,
        )
        .inspect_err(|err| log::error!("failed to load image: install HII package list failed: {:#x?}", err))?;
    }

    // Store the interface pointers for unload to use when uninstalling these protocol interfaces.
    private_info.image_info_ptr = image_info_ptr;
    private_info.image_device_path_ptr = device_path as *mut c_void;

    // save the private image data for this image in the private image data map.
    private_image_data.lock().private_image_data.insert(handle, private_info);

    // return the new handle.
    Ok(handle)
}

// Transfers control to the entry point of an image that was loaded by
// load_image. See EFI_BOOT_SERVICES::StartImage() API definition in UEFI spec
// for usage details.
// * image_handle - handle of the image to be started.
// * exit_data_size - pointer to receive the size, in bytes, of exit_data.
//                    if exit_data is null, this is parameter is ignored.
// * exit_data - pointer to receive a data buffer with exit data, if any.
pub fn start_image(
    private_image_data: &tpl_lock::TplMutex<DxeCoreGlobalImageData>,
    image_handle: efi::Handle,
    exit_data_size: *mut usize,
    exit_data: *mut *mut efi::Char16,
) -> efi::Status {
    let status = core_start_image(private_image_data, image_handle);

    // retrieve any exit data that was provided by the entry point.
    if !exit_data_size.is_null() && !exit_data.is_null() {
        let private_data = private_image_data.lock();
        if let Some(image_data) = private_data.private_image_data.get(&image_handle) {
            if let Some(image_exit_data) = image_data.exit_data {
                unsafe {
                    exit_data_size.write(image_exit_data.0);
                    exit_data.write(image_exit_data.1);
                }
            }
        }
    }

    let image_type = private_image_data.lock().private_image_data.get(&image_handle).map(|x| x.pe_info.image_type);

    if status.is_err() || image_type == Some(crate::image::EFI_IMAGE_SUBSYSTEM_EFI_APPLICATION) {
        let _result = core_unload_image(private_image_data, image_handle, true);
    }

    match status {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => err,
    }
}

pub fn core_start_image(private_image_data: &tpl_lock::TplMutex<DxeCoreGlobalImageData>, image_handle: efi::Handle) -> Result<(), efi::Status> {
    // TODO WE NEED TO GET RID OF THIS.
    with_protocol_db!(|db| db.validate_handle(image_handle))?;

    if let Some(private_data) = private_image_data.lock().private_image_data.get_mut(&image_handle) {
        if private_data.started {
            Err(efi::Status::INVALID_PARAMETER)?;
        }
    } else {
        Err(efi::Status::INVALID_PARAMETER)?;
    }

    // allocate a buffer for the entry point stack.
    let stack = ImageStack::new(ENTRY_POINT_STACK_SIZE)?;

    // define a co-routine that wraps the entry point execution. this doesn't
    // run until the coroutine.resume() call below.
    // TODO: JAVA this is ScopedCoroutine not Coroutine
    let mut coroutine = ScopedCoroutine::with_stack(stack, move |yielder, image_handle| {
        let mut private_data = private_image_data.lock();
        // mark the image as started and grab a copy of the private info.
        let status;
        if let Some(private_info) = private_data.private_image_data.get_mut(&image_handle) {
            private_info.started = true;
            let entry_point = private_info.entry_point;

            // save a pointer to the yielder so that exit() can use it.
            private_data.image_start_contexts.push(yielder as *const Yielder<_, _>);

            // get a copy of the system table pointer to pass to the entry point.
            let system_table = private_data.system_table;
            drop(private_data);
            // invoke the entry point. Code on the other side of this pointer is
            // FFI, which is inherently unsafe, but it's not  "technically" unsafe
            // from a rust standpoint since r_efi doesn't define the ImageEntryPoint
            // pointer type as "pointer to unsafe function"
            status = entry_point(image_handle, system_table);

            //safety note: any variables with "Drop" routines that need to run
            //need to be explicitly dropped before calling exit(). Since exit()
            //effectively "longjmp"s back to StartImage(), rust automatic
            //drops will not be triggered.
            exit(private_image_data, image_handle, status, 0, core::ptr::null_mut());
        } else {
            status = efi::Status::NOT_FOUND;
        }
        status
    });

    // Save the handle of the previously running image and update the currently
    // running image to the one we are about to invoke. In the event of nested
    // calls to StartImage(), the chain of previously running images will
    // be preserved on the stack of the various StartImage() instances.
    let mut private_data = private_image_data.lock();
    let previous_image = private_data.current_running_image;
    private_data.current_running_image = Some(image_handle);
    drop(private_data);

    // switch stacks and execute the above defined coroutine to start the image.
    let status = match coroutine.resume(image_handle) {
        CoroutineResult::Yield(status) => status,
        // Note: `CoroutineResult::Return` is unexpected, since it would imply
        // that exit() failed. TODO: should panic here?
        CoroutineResult::Return(status) => status,
    };

    log::info!("start_image entrypoint exit with status: {:#x?}", status);

    // because we used exit() to return from the coroutine (as opposed to
    // returning naturally from it), the coroutine is marked as suspended rather
    // than complete. We need to forcibly mark the coroutine done; otherwise it
    // will try to use unwind to clean up the co-routine stack (i.e. "drop" any
    // live objects). This unwind support requires std and will panic if
    // executed.
    unsafe { coroutine.force_reset() };

    private_image_data.lock().current_running_image = previous_image;
    match status {
        efi::Status::SUCCESS => Ok(()),
        err => Err(err),
    }
}

pub fn unload_image(private_image_data: &tpl_lock::TplMutex<DxeCoreGlobalImageData>, image_handle: efi::Handle) -> efi::Status {
    match core_unload_image(private_image_data, image_handle, false) {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => err,
    }
}

pub fn core_unload_image(private_image_data: &tpl_lock::TplMutex<DxeCoreGlobalImageData>, image_handle: efi::Handle, force_unload: bool) -> Result<(), efi::Status> {
    with_protocol_db!(|db| db.validate_handle(image_handle))?;
    let image_data = private_image_data.lock();
    let private_data = image_data.private_image_data.get(&image_handle).ok_or(efi::Status::INVALID_PARAMETER)?;
    let unload_function = private_data.image_info.unload;
    let started = private_data.started;
    drop(image_data); // release the image lock while unload logic executes as this function may be re-entrant.

    // if the image has been started, request that it unload, and don't unload it if
    // the unload function doesn't exist or returns an error.
    if started {
        if let Some(function) = unload_function {
            //Safety: this is unsafe (even though rust doesn't think so) because we are calling
            //into the "unload" function pointer that the image itself set. r_efi doesn't mark
            //the unload function type as unsafe - so rust reports an "unused_unsafe" since it
            //doesn't know it's unsafe. We suppress the warning and mark it unsafe anyway as a
            //warning to the future.
            #[allow(unused_unsafe)]
            unsafe {
                let status = (function)(image_handle);
                if status != efi::Status::SUCCESS {
                    Err(status)?;
                }
            }
        } else if !force_unload {
            Err(efi::Status::UNSUPPORTED)?;
        }
    }
    let handles = with_protocol_db!(|db|db.locate_handles(None).unwrap_or_default());

    // close any protocols opened by this image.
    for handle in handles {
        let protocols = match with_protocol_db!(|db|db.get_protocols_on_handle(handle)) {
            Err(_) => continue,
            Ok(protocols) => protocols,
        };
        for protocol in protocols {
            let open_infos = match with_protocol_db!(|db|db.get_open_protocol_information_by_protocol(handle, protocol)) {
                Err(_) => continue,
                Ok(open_infos) => open_infos,
            };
            for open_info in open_infos {
                if Some(image_handle) == open_info.agent_handle {
                    let _result = with_protocol_db!(|db|db.remove_protocol_usage(
                        handle,
                        protocol,
                        open_info.agent_handle,
                        open_info.controller_handle,
                    ));
                }
            }
        }
    }

    // remove the private data for this image from the private_image_data map.
    // it will get dropped when it goes out of scope at the end of the function and the pages allocated for it
    // and the image_info box along with it.
    let private_image_data = private_image_data.lock().private_image_data.remove(&image_handle).unwrap();
    // remove the image and device path protocols from the image handle.
    let _ = with_protocol_db!(|db|db.uninstall_protocol_interface(
        image_handle,
        efi::protocols::loaded_image::PROTOCOL_GUID,
        private_image_data.image_info_ptr,
    ));

    let _ = with_protocol_db!(|db|db.uninstall_protocol_interface(
        image_handle,
        efi::protocols::loaded_image_device_path::PROTOCOL_GUID,
        private_image_data.image_device_path_ptr,
    ));

    // we have to remove the memory protections from the image sections before freeing the image buffer, because
    // core_free_pages expects the memory being freed to be in a single continuous memory descriptor, which is not
    // true when we've changed the attributes per section
    remove_image_memory_protections(&private_image_data.pe_info, &private_image_data);

    Ok(())
}

// Terminates a loaded EFI image and returns control to boot services.
// See EFI_BOOT_SERVICES::Exit() API definition in UEFI spec for usage details.
// * image_handle - the handle of the currently running image.
// * exit_status - the exit status for the image.
// * exit_data_size - the size of the exit_data buffer, if exit_data is not
//                    null.
// * exit_data - optional buffer of data provided by the caller.
pub fn exit(
    private_image_data: &tpl_lock::TplMutex<DxeCoreGlobalImageData>,
    image_handle: efi::Handle,
    status: efi::Status,
    exit_data_size: usize,
    exit_data: *mut efi::Char16,
) -> efi::Status {
    let started = match private_image_data.lock().private_image_data.get(&image_handle) {
        Some(image_data) => image_data.started,
        None => return efi::Status::INVALID_PARAMETER,
    };

    // if not started, just unload the image.
    if !started {
        return match core_unload_image(private_image_data, image_handle, true) {
            Ok(()) => efi::Status::SUCCESS,
            Err(_err) => efi::Status::INVALID_PARAMETER,
        };
    }

    // image has been started - check the currently running image.
    let mut private_data = private_image_data.lock();
    if Some(image_handle) != private_data.current_running_image {
        return efi::Status::INVALID_PARAMETER;
    }

    // save the exit data, if present, into the private_image_data for this
    // image for start_image to retrieve and return.
    if (exit_data_size != 0) && !exit_data.is_null() {
        if let Some(image_data) = private_data.private_image_data.get_mut(&image_handle) {
            image_data.exit_data = Some((exit_data_size, exit_data));
        }
    }

    // retrieve the yielder that was saved in the start_image entry point
    // coroutine wrapper.
    // safety note: this assumes that the top of the image_start_contexts stack
    // is the currently running image.
    if let Some(yielder) = private_data.image_start_contexts.pop() {
        let yielder = unsafe { &*yielder };
        drop(private_data);

        // safety note: any variables with "Drop" routines that need to run
        // need to be explicitly dropped before calling suspend(). Since suspend()
        // effectively "longjmp"s back to StartImage(), rust automatic
        // drops will not be triggered.

        // transfer control back to start_image by calling the suspend function on
        // yielder. This will switch stacks back to the start_image that invoked
        // the entry point coroutine.
        yielder.suspend(status);
    }

    //should never reach here, but rust doesn't know that.
    efi::Status::ACCESS_DENIED
}