/// Returns a device path for the file specified by the given fv_handle and filename GUID.
pub fn device_path_bytes_for_fv_file(fv_handle: efi::Handle, file_name: efi::Guid) -> Result<Box<[u8]>, efi::Status> {
    let fv_device_path = PROTOCOL_DB.get_interface_for_handle(fv_handle, efi::protocols::device_path::PROTOCOL_GUID)?;
    let file_node = &FvPiWgDevicePath::new_file(file_name);
    concat_device_path_to_boxed_slice(
        fv_device_path as *mut _ as *const efi::protocols::device_path::Protocol,
        file_node as *const _ as *const efi::protocols::device_path::Protocol,
    )
}
