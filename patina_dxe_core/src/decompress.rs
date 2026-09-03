//! A module for core UEFI decompression functionality.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::ffi::c_void;

use alloc::boxed::Box;
use patina::{
    component::{Storage, component},
    error::EfiError,
    log_debug_assert,
    standard::efi::{self, protocols::decompress},
    uefi::{
        boot_services::BootServices,
        decompress::{DecompressionAlgorithm, decompress_into_with_algo},
    },
};

/// Component to install the UEFI Decompress Protocol.
#[derive(Default)]
pub(crate) struct DecompressProtocolInstaller;

#[component]
impl DecompressProtocolInstaller {
    fn entry_point(self, storage: &mut Storage) -> patina::error::Result<()> {
        let protocol = Box::new(decompress::Protocol { get_info, decompress });

        match storage.boot_services().install_protocol_interface(None, protocol) {
            Ok(_) => Ok(()),
            Err(err) => EfiError::status_to_result(err),
        }
    }
}

unsafe extern "efiapi" fn get_info(
    _: *mut decompress::Protocol,
    src: *mut c_void,
    src_size: u32,
    dst_size: *mut u32,
    scratch_size: *mut u32,
) -> efi::Status {
    if src.is_null() | dst_size.is_null() | scratch_size.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    if src_size < 8 {
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: The data the pointer points to is at least 8 bytes long, as checked above.
    let compressed_size = unsafe { src.cast::<u32>().read_unaligned() };

    if (src_size < compressed_size + 8) || compressed_size.checked_add(8).is_none() {
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: The pointers are not null, as checked above.
    //         The data the pointer points to is at least 8 bytes long, as checked above.
    unsafe { dst_size.write_volatile(src.cast::<u32>().add(1).read_unaligned()) };

    // We do not need any scratch space for the rust implementation.
    // SAFETY: The pointer is not null, as checked above.
    unsafe { scratch_size.cast::<u32>().write_volatile(0) };

    efi::Status::SUCCESS
}

/// FFI interface to decompress data and return it.
unsafe extern "efiapi" fn decompress(
    _: *mut decompress::Protocol,
    source_buffer: *mut c_void,
    source_size: u32,
    destination_buffer: *mut c_void,
    destination_size: u32,
    _scratch_buffer: *mut c_void,
    _scratch_size: u32,
) -> efi::Status {
    if source_buffer.is_null() || destination_buffer.is_null() {
        log_debug_assert!("DecompressProtocol::decompress called with null pointer");
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: source_buffer and destination_buffer pointers are validated as non-null.
    // Sizes are provided by caller and trusted to match the buffer allocations.
    let src = unsafe { core::slice::from_raw_parts(source_buffer as *const u8, source_size as usize) };
    // SAFETY: destination_buffer is validated as non-null and mutable access is exclusive.
    let dst = unsafe { core::slice::from_raw_parts_mut(destination_buffer as *mut u8, destination_size as usize) };

    match decompress_into_with_algo(src, dst, DecompressionAlgorithm::UefiDecompress) {
        Ok(()) => efi::Status::SUCCESS,
        Err(_) => efi::Status::INVALID_PARAMETER,
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use core::ptr;

    // Builds a minimal 16-byte compressed header: [compressed_size: u32][orig_size: u32] followed by padding.
    fn compressed_header(compressed_size: u32, orig_size: u32) -> [u8; 16] {
        let mut buf = [0u8; 16];
        buf[0..4].copy_from_slice(&compressed_size.to_le_bytes());
        buf[4..8].copy_from_slice(&orig_size.to_le_bytes());
        buf
    }

    #[test]
    fn test_decompress_get_info_rejects_invalid_parameters() {
        let mut src = compressed_header(8, 100);
        let mut dst_size = 0u32;
        let mut scratch_size = 0u32;

        // SAFETY: src is valid, dst_size is valid, scratch_size is valid.
        let status =
            unsafe { get_info(ptr::null_mut(), ptr::null_mut(), 16, &raw mut dst_size, &raw mut scratch_size) };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);

        // SAFETY: src is valid, dst_size is null under test, scratch_size is valid.
        let status = unsafe {
            get_info(ptr::null_mut(), src.as_mut_ptr() as *mut c_void, 16, ptr::null_mut(), &raw mut scratch_size)
        };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);

        // SAFETY: src and dst_size are valid, scratch_size is null under test.
        let status = unsafe {
            get_info(ptr::null_mut(), src.as_mut_ptr() as *mut c_void, 16, &raw mut dst_size, ptr::null_mut())
        };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);

        let mut small = [0u8; 4];
        // SAFETY: all pointers reference valid stack values.
        let status = unsafe {
            get_info(ptr::null_mut(), small.as_mut_ptr() as *mut c_void, 4, &raw mut dst_size, &raw mut scratch_size)
        };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);

        let mut undersized = compressed_header(100, 50);
        // SAFETY: all pointers reference valid stack values.
        let status = unsafe {
            get_info(
                ptr::null_mut(),
                undersized.as_mut_ptr() as *mut c_void,
                16,
                &raw mut dst_size,
                &raw mut scratch_size,
            )
        };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_decompress_get_info_returns_sizes_on_valid_input() {
        let mut src = compressed_header(8, 100);
        let mut dst_size = 0xFFFF_FFFFu32;
        let mut scratch_size = 0xFFFF_FFFFu32;

        // SAFETY: all pointers reference valid stack values and src_size matches the buffer.
        let status = unsafe {
            get_info(ptr::null_mut(), src.as_mut_ptr() as *mut c_void, 16, &raw mut dst_size, &raw mut scratch_size)
        };
        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(dst_size, 100);
        assert_eq!(scratch_size, 0);
    }

    #[test]
    fn test_decompress_decompress_rejects_invalid_parameters() {
        let mut src = compressed_header(8, 0);
        let mut dst = [0u8; 8];

        // SAFETY: source_buffer is null under test.
        let status = unsafe {
            decompress(ptr::null_mut(), ptr::null_mut(), 16, dst.as_mut_ptr() as *mut c_void, 8, ptr::null_mut(), 0)
        };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);

        // SAFETY: destination_buffer is null under test.
        let status = unsafe {
            decompress(ptr::null_mut(), src.as_mut_ptr() as *mut c_void, 16, ptr::null_mut(), 8, ptr::null_mut(), 0)
        };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);

        let mut malformed = [0u8; 4];
        // SAFETY: both buffers are valid; the source is intentionally malformed.
        let status = unsafe {
            decompress(
                ptr::null_mut(),
                malformed.as_mut_ptr() as *mut c_void,
                4,
                dst.as_mut_ptr() as *mut c_void,
                8,
                ptr::null_mut(),
                0,
            )
        };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_decompress_decompress_succeeds_with_zero_original_size() {
        let mut src = compressed_header(8, 0);
        let mut dst = [0u8; 8];

        // SAFETY: both buffers are valid; destination_size of 0 yields an empty destination slice.
        let status = unsafe {
            decompress(
                ptr::null_mut(),
                src.as_mut_ptr() as *mut c_void,
                16,
                dst.as_mut_ptr() as *mut c_void,
                0,
                ptr::null_mut(),
                0,
            )
        };
        assert_eq!(status, efi::Status::SUCCESS);
    }
}
