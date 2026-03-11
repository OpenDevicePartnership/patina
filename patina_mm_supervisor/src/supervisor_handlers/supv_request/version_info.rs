//! VERSION_INFO Request Handler
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use r_efi::efi;

use patina::management_mode::protocol::mm_supervisor_request::{
    MmSupervisorRequestHeader, MmSupervisorVersionInfo, RequestType,
};

use crate::supervisor_handlers::{PATCH_LEVEL, VERSION};

/// Handle a VERSION_INFO request.
///
/// Writes back the response header followed by [`MmSupervisorVersionInfo`].
pub(super) fn handle_version_info(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    let response_size = MmSupervisorRequestHeader::SIZE + MmSupervisorVersionInfo::SIZE;

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

    // SAFETY: We verified the buffer is large enough for header + version info.
    unsafe {
        let payload_ptr = comm_buffer.add(MmSupervisorRequestHeader::SIZE) as *mut MmSupervisorVersionInfo;
        core::ptr::write(payload_ptr, version_info);
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
