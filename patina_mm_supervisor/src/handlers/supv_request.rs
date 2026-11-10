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
/// The buffer starts with an [`MmSupervisorRequestHeader`], followed by any request or response
/// payload. The request status goes in the header `result` field. The return value only reports
/// a malformed header; any request that reaches dispatch returns `SUCCESS`.
///
/// ## Safety
///
/// `comm_buffer` must be null or point to `*comm_buffer_size` bytes that are readable and
/// writable for the duration of the call.
pub(crate) unsafe fn mm_supv_request_handler(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    if comm_buffer.is_null() || *comm_buffer_size < MmSupervisorRequestHeader::SIZE {
        log::error!(
            "MmSupvRequestHandler: buffer too small ({} bytes, need at least {})",
            *comm_buffer_size,
            MmSupervisorRequestHeader::SIZE,
        );
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: the buffer is non-null and holds a complete header (checked above), and
    // `read_unaligned` has no alignment requirement.
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
            // SAFETY: this handler's own contract covers `comm_buffer` for `*comm_buffer_size` bytes.
            unsafe { version_info::handle_version_info(comm_buffer, comm_buffer_size) }
        }
        Ok(RequestType::FetchPolicy) => {
            log::debug!("Processing FETCH_POLICY request");
            // SAFETY: this handler's own contract covers `comm_buffer` for `*comm_buffer_size` bytes.
            unsafe { fetch_policy::handle_fetch_policy(comm_buffer, comm_buffer_size) }
        }
        Ok(RequestType::CommUpdate) => {
            log::debug!("Processing COMM_UPDATE request");
            comm_update::handle_comm_update(comm_buffer, comm_buffer_size)
        }
        Ok(RequestType::UnblockMem) => {
            log::debug!("Processing UNBLOCK_MEM request");
            // SAFETY: this handler's own contract covers `comm_buffer` for `*comm_buffer_size` bytes.
            unsafe { unblock_memory::handle_unblock_mem(comm_buffer, comm_buffer_size) }
        }
        Err(unknown) => {
            log::warn!("MmSupvRequestHandler: unsupported request type 0x{unknown:08X}");
            *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
            efi::Status::UNSUPPORTED
        }
    };

    // SAFETY: the buffer holds a complete header (checked above) and is writable under this
    // handler's own contract.
    unsafe { write_request_result(comm_buffer, status) };

    efi::Status::SUCCESS
}

/// Writes `status` into the request header `result` field as a `u64`.
///
/// ## Safety
///
/// `comm_buffer` must point to at least `MmSupervisorRequestHeader::SIZE` readable and writable
/// bytes.
unsafe fn write_request_result(comm_buffer: *mut u8, status: efi::Status) {
    // SAFETY: the caller guarantees a readable and writable header, and unaligned accesses have
    // no alignment requirement.
    unsafe {
        let mut header = comm_buffer.cast::<MmSupervisorRequestHeader>().read_unaligned();
        header.result = status.as_usize() as u64;
        comm_buffer.cast::<MmSupervisorRequestHeader>().write_unaligned(header);
    }
}
