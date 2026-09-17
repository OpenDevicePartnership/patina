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
    log::info!("MmSupvRequestHandler invoked (buffer_size={})", *comm_buffer_size);

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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::supervisor_handlers::{PATCH_LEVEL, VERSION};
    use patina::management_mode::protocol::mm_supervisor_request::MmSupervisorVersionInfo;
    use zerocopy::IntoBytes;

    const VERSION_RESPONSE_SIZE: usize = MmSupervisorRequestHeader::SIZE + MmSupervisorVersionInfo::SIZE;

    fn request_header(signature: u32, revision: u32, request: u32) -> MmSupervisorRequestHeader {
        MmSupervisorRequestHeader { signature, revision, request, reserved: 0, result: 0 }
    }

    fn request_buffer<const N: usize>(header: MmSupervisorRequestHeader) -> [u8; N] {
        let mut buffer = [0; N];
        buffer
            .get_mut(..MmSupervisorRequestHeader::SIZE)
            .expect("test request buffer must fit the request header")
            .copy_from_slice(header.as_bytes());
        buffer
    }

    fn response_header(buffer: &[u8]) -> MmSupervisorRequestHeader {
        MmSupervisorRequestHeader::from_bytes(
            buffer.get(..MmSupervisorRequestHeader::SIZE).expect("response buffer must contain the request header"),
        )
        .expect("response header must have a valid layout")
    }

    #[test]
    fn rejects_a_null_buffer() {
        let mut size = MmSupervisorRequestHeader::SIZE;

        assert_eq!(mm_supv_request_handler(core::ptr::null_mut(), &mut size), efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn rejects_a_buffer_smaller_than_the_request_header() {
        let mut buffer = [0; MmSupervisorRequestHeader::SIZE];
        let mut size = MmSupervisorRequestHeader::SIZE - 1;

        assert_eq!(mm_supv_request_handler(buffer.as_mut_ptr(), &mut size), efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn rejects_an_invalid_signature() {
        let header = request_header(0, REVISION, RequestType::VersionInfo.into());
        let mut buffer = request_buffer::<VERSION_RESPONSE_SIZE>(header);
        let mut size = VERSION_RESPONSE_SIZE;

        assert_eq!(mm_supv_request_handler(buffer.as_mut_ptr(), &mut size), efi::Status::INVALID_PARAMETER);
        assert_eq!(size, VERSION_RESPONSE_SIZE);
        assert_eq!(response_header(&buffer).result, 0);
    }

    #[test]
    fn rejects_an_unsupported_revision() {
        let header = request_header(SIGNATURE, REVISION + 1, RequestType::VersionInfo.into());
        let mut buffer = request_buffer::<VERSION_RESPONSE_SIZE>(header);
        let mut size = VERSION_RESPONSE_SIZE;

        assert_eq!(mm_supv_request_handler(buffer.as_mut_ptr(), &mut size), efi::Status::UNSUPPORTED);
        assert_eq!(size, VERSION_RESPONSE_SIZE);
        assert_eq!(response_header(&buffer).result, 0);
    }

    #[test]
    fn reports_an_unknown_request_in_the_response_header() {
        let header = request_header(SIGNATURE, REVISION, u32::MAX);
        let mut buffer = request_buffer::<VERSION_RESPONSE_SIZE>(header);
        let mut size = VERSION_RESPONSE_SIZE;

        assert_eq!(mm_supv_request_handler(buffer.as_mut_ptr(), &mut size), efi::Status::SUCCESS);
        assert_eq!(size, MmSupervisorRequestHeader::SIZE);
        assert_eq!(response_header(&buffer).result, efi::Status::UNSUPPORTED.as_usize() as u64);
    }

    #[test]
    fn returns_version_information() {
        let header = request_header(SIGNATURE, REVISION, RequestType::VersionInfo.into());
        let mut buffer = request_buffer::<VERSION_RESPONSE_SIZE>(header);
        let mut size = VERSION_RESPONSE_SIZE;

        assert_eq!(mm_supv_request_handler(buffer.as_mut_ptr(), &mut size), efi::Status::SUCCESS);
        assert_eq!(size, VERSION_RESPONSE_SIZE);
        assert_eq!(response_header(&buffer).result, efi::Status::SUCCESS.as_usize() as u64);

        let version_info = MmSupervisorVersionInfo::from_bytes(
            buffer
                .get(MmSupervisorRequestHeader::SIZE..VERSION_RESPONSE_SIZE)
                .expect("version response must contain its payload"),
        )
        .expect("version response payload must have a valid layout");
        assert_eq!(version_info.version, VERSION);
        assert_eq!(version_info.patch_level, PATCH_LEVEL);
        assert_eq!(version_info.max_supervisor_request_level, RequestType::MAX_REQUEST_TYPE);
    }

    #[test]
    fn reports_comm_update_as_access_denied_in_the_response_header() {
        let header = request_header(SIGNATURE, REVISION, RequestType::CommUpdate.into());
        let mut buffer = request_buffer::<VERSION_RESPONSE_SIZE>(header);
        let mut size = VERSION_RESPONSE_SIZE;

        assert_eq!(mm_supv_request_handler(buffer.as_mut_ptr(), &mut size), efi::Status::SUCCESS);
        assert_eq!(size, MmSupervisorRequestHeader::SIZE);
        assert_eq!(response_header(&buffer).result, efi::Status::ACCESS_DENIED.as_usize() as u64);
    }

    #[test]
    fn reports_a_short_version_response_buffer_in_the_response_header() {
        let header = request_header(SIGNATURE, REVISION, RequestType::VersionInfo.into());
        let mut buffer = request_buffer::<{ MmSupervisorRequestHeader::SIZE }>(header);
        let mut size = buffer.len();

        assert_eq!(mm_supv_request_handler(buffer.as_mut_ptr(), &mut size), efi::Status::SUCCESS);
        assert_eq!(size, MmSupervisorRequestHeader::SIZE);
        assert_eq!(response_header(&buffer).result, efi::Status::BUFFER_TOO_SMALL.as_usize() as u64);
    }

    #[test]
    fn accepts_an_unaligned_communication_buffer() {
        let header = request_header(SIGNATURE, 0, RequestType::VersionInfo.into());
        let mut storage = [0u8; VERSION_RESPONSE_SIZE + 1];
        let buffer = storage.get_mut(1..).expect("test storage must provide an intentionally unaligned buffer");
        buffer
            .get_mut(..MmSupervisorRequestHeader::SIZE)
            .expect("test request buffer must fit the request header")
            .copy_from_slice(header.as_bytes());
        let mut size = buffer.len();

        assert_eq!(mm_supv_request_handler(buffer.as_mut_ptr(), &mut size), efi::Status::SUCCESS);
        assert_eq!(size, VERSION_RESPONSE_SIZE);
        assert_eq!(response_header(buffer).result, efi::Status::SUCCESS.as_usize() as u64);
    }
}
