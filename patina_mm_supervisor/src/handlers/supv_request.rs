//! Supervisor request dispatcher.
//!
//! Handles structured requests the non-MM environment sends with
//! `MM_SUPERVISOR_REQUEST_HANDLER_GUID`.
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

/// Handles MM Supervisor requests from the non-MM environment:
/// - [`RequestType::UnblockMem`]: unblock memory regions
/// - [`RequestType::FetchPolicy`]: fetch the security policy
/// - [`RequestType::VersionInfo`]: query supervisor version information
/// - [`RequestType::CommUpdate`]: update the communication buffer configuration
///
/// The buffer starts with an [`MmSupervisorRequestHeader`]. The request status is written to
/// the header `result` field, and any response payload follows the header. The return value
/// reports only communication-level errors: a malformed header is rejected, and any request that
/// reaches dispatch returns `SUCCESS`.
pub(crate) fn mm_supv_request_handler(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    if comm_buffer.is_null() || *comm_buffer_size < MmSupervisorRequestHeader::SIZE {
        log::error!(
            "MmSupvRequestHandler: buffer too small ({} bytes, need at least {})",
            *comm_buffer_size,
            MmSupervisorRequestHeader::SIZE,
        );
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: the caller provides a readable communication buffer, and the checks above show it
    // holds a complete header. `read_unaligned` accepts buffers that are not aligned for
    // `MmSupervisorRequestHeader`.
    let header = unsafe { comm_buffer.cast::<MmSupervisorRequestHeader>().read_unaligned() };

    if header.signature != SIGNATURE {
        log::error!("MmSupvRequestHandler: invalid signature 0x{:08X}, expected 0x{:08X}", header.signature, SIGNATURE);
        return efi::Status::INVALID_PARAMETER;
    }

    if header.revision > REVISION {
        log::error!("MmSupvRequestHandler: unsupported revision {}, max supported {}", header.revision, REVISION);
        return efi::Status::UNSUPPORTED;
    }

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

    write_request_result(comm_buffer, status);

    // The return value reports communication-level errors only; the request status is in the
    // header.
    efi::Status::SUCCESS
}

/// Writes `status` into the request header `result` field, as its raw value widened to `u64`
/// like the C `MM_SUPERVISOR_REQUEST_HEADER.Result`.
///
/// `comm_buffer` must point to at least `MmSupervisorRequestHeader::SIZE` writable bytes.
fn write_request_result(comm_buffer: *mut u8, status: efi::Status) {
    // SAFETY: the dispatcher checked that the buffer holds a writable header. Unaligned reads and
    // writes avoid an alignment requirement on the communication buffer.
    unsafe {
        let mut header = comm_buffer.cast::<MmSupervisorRequestHeader>().read_unaligned();
        header.result = status.as_usize() as u64;
        comm_buffer.cast::<MmSupervisorRequestHeader>().write_unaligned(header);
    }
}
