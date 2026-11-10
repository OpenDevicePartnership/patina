//! `COMM_UPDATE` request handler.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use patina::standard::efi;

use patina::management_mode::protocol::mm_supervisor_request::MmSupervisorRequestHeader;

/// Handles a `COMM_UPDATE` request by rejecting it with `ACCESS_DENIED`.
///
/// Dynamic communication buffer updates are not supported because a runtime allocation is
/// expected to land in the PEI memory bin.
pub(super) fn handle_comm_update(_comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    *comm_buffer_size = MmSupervisorRequestHeader::SIZE;

    efi::Status::ACCESS_DENIED
}
