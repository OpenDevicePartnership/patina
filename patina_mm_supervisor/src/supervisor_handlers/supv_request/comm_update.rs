//! COMM_UPDATE Request Handler
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use r_efi::efi;

use patina::management_mode::protocol::mm_supervisor_request::MmSupervisorRequestHeader;

/// Handle a COMM_UPDATE request.
///
/// Updates the communication buffer address for future SMI entries.
pub(super) fn handle_comm_update(_comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    log::info!("COMM_UPDATE request");

    // We do not support dynamic communication buffer updates in this implementation, because
    // we expect the runtime allocation will fall into PEI memory bin.
    *comm_buffer_size = MmSupervisorRequestHeader::SIZE;

    efi::Status::ACCESS_DENIED
}
