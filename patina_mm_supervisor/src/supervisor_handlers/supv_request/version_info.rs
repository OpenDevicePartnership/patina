//! `VERSION_INFO` Request Handler
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use patina::standard::efi;

use patina::management_mode::protocol::mm_supervisor_request::{
    MmSupervisorRequestHeader, MmSupervisorVersionInfo, RequestType,
};

use crate::supervisor_handlers::{PATCH_LEVEL, VERSION};

/// Handle a `VERSION_INFO` request.
///
/// Writes back the response header followed by [`MmSupervisorVersionInfo`].
pub(super) fn handle_version_info(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    let response_size = MmSupervisorRequestHeader::SIZE + MmSupervisorVersionInfo::SIZE;

    if comm_buffer.is_null() {
        log::error!("VERSION_INFO: communication buffer is null");
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::INVALID_PARAMETER;
    }

    if *comm_buffer_size < response_size {
        log::error!(
            "VERSION_INFO: buffer too small for response ({} bytes, need {})",
            *comm_buffer_size,
            response_size,
        );
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::BUFFER_TOO_SMALL;
    }

    // Write version info payload after the header
    let version_info = MmSupervisorVersionInfo {
        version: VERSION,
        patch_level: PATCH_LEVEL,
        max_supervisor_request_level: RequestType::MAX_REQUEST_TYPE,
    };

    // SAFETY: The buffer is non-null and large enough for the header and payload. The unaligned
    // write supports communication buffers without natural `MmSupervisorVersionInfo` alignment.
    unsafe {
        let payload_ptr = comm_buffer.add(MmSupervisorRequestHeader::SIZE) as *mut MmSupervisorVersionInfo;
        payload_ptr.write_unaligned(version_info);
    }

    *comm_buffer_size = response_size;
    log::info!(
        "VERSION_INFO response: version=0x{:08X}, patch=0x{:08X}, max_level={}",
        VERSION,
        PATCH_LEVEL,
        RequestType::MAX_REQUEST_TYPE,
    );

    efi::Status::SUCCESS
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    const RESPONSE_SIZE: usize = MmSupervisorRequestHeader::SIZE + MmSupervisorVersionInfo::SIZE;

    #[test]
    fn rejects_a_null_buffer() {
        let mut size = RESPONSE_SIZE;

        assert_eq!(handle_version_info(core::ptr::null_mut(), &mut size), efi::Status::INVALID_PARAMETER);
        assert_eq!(size, MmSupervisorRequestHeader::SIZE);
    }

    #[test]
    fn reports_the_required_size_for_a_short_buffer() {
        let mut buffer = [0u8; MmSupervisorRequestHeader::SIZE];
        let mut size = buffer.len();

        assert_eq!(handle_version_info(buffer.as_mut_ptr(), &mut size), efi::Status::BUFFER_TOO_SMALL);
        assert_eq!(size, MmSupervisorRequestHeader::SIZE);
    }

    #[test]
    fn writes_version_information_to_an_unaligned_buffer() {
        let mut storage = [0u8; RESPONSE_SIZE + 1];
        let buffer = storage.get_mut(1..).expect("test storage must contain an unaligned response buffer");
        let mut size = buffer.len();

        assert_eq!(handle_version_info(buffer.as_mut_ptr(), &mut size), efi::Status::SUCCESS);
        assert_eq!(size, RESPONSE_SIZE);

        let version = MmSupervisorVersionInfo::from_bytes(
            buffer
                .get(MmSupervisorRequestHeader::SIZE..RESPONSE_SIZE)
                .expect("response must contain version information"),
        )
        .expect("version information must have a valid layout");
        assert_eq!(version.version, VERSION);
        assert_eq!(version.patch_level, PATCH_LEVEL);
        assert_eq!(version.max_supervisor_request_level, RequestType::MAX_REQUEST_TYPE);
    }
}
