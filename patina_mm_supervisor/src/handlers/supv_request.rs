//! Supervisor Request Dispatcher
//!
//! Handles structured requests from the non-MM environment via the
//! `MM_SUPERVISOR_REQUEST_HANDLER_GUID` protocol.
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

use patina::standard::efi;

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
    if comm_buffer.is_null() || *comm_buffer_size < MmSupervisorRequestHeader::SIZE {
        log::error!(
            "MmSupvRequestHandler: buffer too small ({} bytes, need at least {})",
            *comm_buffer_size,
            MmSupervisorRequestHeader::SIZE,
        );
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: The caller provides a readable communication buffer and the checks above establish
    // that it contains a complete header. `read_unaligned` also accepts firmware buffers that are
    // not naturally aligned for `MmSupervisorRequestHeader`.
    let header = unsafe { comm_buffer.cast::<MmSupervisorRequestHeader>().read_unaligned() };

    // Validate signature
    if header.signature != SIGNATURE {
        log::error!("MmSupvRequestHandler: invalid signature 0x{:08X}, expected 0x{:08X}", header.signature, SIGNATURE);
        return efi::Status::INVALID_PARAMETER;
    }

    // Validate revision
    if header.revision > REVISION {
        log::error!("MmSupvRequestHandler: unsupported revision {}, max supported {}", header.revision, REVISION);
        return efi::Status::UNSUPPORTED;
    }

    // Dispatch by request type
    let status = match RequestType::try_from(header.request) {
        Ok(RequestType::VersionInfo) => {
            log::debug!("Processing VERSION_INFO request");
            version_info::handle_version_info(comm_buffer, comm_buffer_size)
        }
        Ok(RequestType::FetchPolicy) => {
            log::debug!("Processing FETCH_POLICY request");
            fetch_policy::handle_fetch_policy(comm_buffer, comm_buffer_size)
        }
        Ok(RequestType::CommUpdate) => {
            log::debug!("Processing COMM_UPDATE request");
            comm_update::handle_comm_update(comm_buffer, comm_buffer_size)
        }
        Ok(RequestType::UnblockMem) => {
            log::debug!("Processing UNBLOCK_MEM request");
            unblock_memory::handle_unblock_mem(comm_buffer, comm_buffer_size)
        }
        Err(unknown) => {
            log::warn!("MmSupvRequestHandler: unsupported request type 0x{unknown:08X}");
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
    // SAFETY: The dispatcher validated that the buffer contains a writable header. Unaligned
    // reads and writes avoid imposing an alignment requirement on the communication buffer.
    unsafe {
        let mut header = comm_buffer.cast::<MmSupervisorRequestHeader>().read_unaligned();
        header.result = status.as_usize() as u64;
        comm_buffer.cast::<MmSupervisorRequestHeader>().write_unaligned(header);
    }
}
