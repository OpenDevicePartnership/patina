//! Supervisor Request Dispatcher
//!
//! Handles structured requests from the non-MM environment via the
//! `MM_SUPERVISOR_REQUEST_HANDLER_GUID` protocol.
//!
//! Each request type is handled by a dedicated sub-module:
//! - [`version_info`] — supervisor version query
//! - [`fetch_policy`] — security policy retrieval
//! - [`comm_update`] — communication buffer updates
//! - [`unblock_mem`] — memory region unblocking
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

mod comm_update;
mod fetch_policy;
pub(crate) mod unblock_memory;
mod version_info;

use r_efi::efi;

use patina::management_mode::protocol::mm_supervisor_request::{
    MmSupervisorRequestHeader, REVISION, RequestType, SIGNATURE,
};

/// MM Supervisor request handler implementation.
///
/// Handles structured requests from the non-MM environment, such as:
/// - [`RequestType::UnblockMem`]: Unblock memory regions
/// - [`RequestType::FetchPolicy`]: Fetch security policy
/// - [`RequestType::VersionInfo`]: Query supervisor version information
/// - [`RequestType::CommUpdate`]: Update communication buffer configuration
///
/// The buffer is expected to contain an [`MmSupervisorRequestHeader`] at the start.
/// On return, the header's `result` field is set and any response payload follows
/// immediately after the header.
pub(crate) fn mm_supv_request_handler(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    log::info!("MmSupvRequestHandler invoked (buffer_size={})", *comm_buffer_size);

    if comm_buffer.is_null() || *comm_buffer_size < MmSupervisorRequestHeader::SIZE {
        log::error!(
            "MmSupvRequestHandler: buffer too small ({} bytes, need at least {})",
            *comm_buffer_size,
            MmSupervisorRequestHeader::SIZE,
        );
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: We verified the buffer is non-null and large enough for the header.
    let header = unsafe { &*(comm_buffer as *const MmSupervisorRequestHeader) };

    // Validate signature
    if header.signature != SIGNATURE {
        log::error!("MmSupvRequestHandler: invalid signature 0x{:08X}, expected 0x{:08X}", header.signature, SIGNATURE,);
        return efi::Status::INVALID_PARAMETER;
    }

    // Validate revision
    if header.revision > REVISION {
        log::error!("MmSupvRequestHandler: unsupported revision {}, max supported {}", header.revision, REVISION,);
        return efi::Status::UNSUPPORTED;
    }

    // Dispatch by request type
    let status = match RequestType::try_from(header.request) {
        Ok(RequestType::VersionInfo) => {
            log::info!("Processing VERSION_INFO request");
            version_info::handle_version_info(comm_buffer, comm_buffer_size)
        }
        Ok(RequestType::FetchPolicy) => {
            log::info!("Processing FETCH_POLICY request");
            fetch_policy::handle_fetch_policy(comm_buffer, comm_buffer_size)
        }
        Ok(RequestType::CommUpdate) => {
            log::info!("Processing COMM_UPDATE request");
            comm_update::handle_comm_update(comm_buffer, comm_buffer_size)
        }
        Ok(RequestType::UnblockMem) => {
            log::info!("Processing UNBLOCK_MEM request");
            unblock_memory::handle_unblock_mem(comm_buffer, comm_buffer_size)
        }
        Err(unknown) => {
            log::warn!("MmSupvRequestHandler: unsupported request type 0x{:08X}", unknown);
            *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
            efi::Status::UNSUPPORTED
        }
    };

    // Write the final status into the request header's result field.
    write_request_result(comm_buffer, status);

    // The handler's return value is only for indicating communication-level errors
    // (e.g., interrupt is being handled or not), in this case we handled the request successfully.
    efi::Status::SUCCESS
}

/// Write an [`efi::Status`] into the request header's `result` field.
///
/// The status is stored as its raw `usize` representation cast to `u64`,
/// matching the C `MM_SUPERVISOR_REQUEST_HEADER.Result` convention.
///
/// ## Safety
///
/// `comm_buffer` must point to at least `MmSupervisorRequestHeader::SIZE` bytes of writable memory.
fn write_request_result(comm_buffer: *mut u8, status: efi::Status) {
    // SAFETY: caller guarantees buffer is large enough for the header.
    unsafe {
        let header = &mut *(comm_buffer as *mut MmSupervisorRequestHeader);
        header.result = status.as_usize() as u64;
    }
}
