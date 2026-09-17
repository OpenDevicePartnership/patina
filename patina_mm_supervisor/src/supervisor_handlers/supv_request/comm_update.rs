//! `COMM_UPDATE` Request Handler
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use patina::standard::efi;

use patina::management_mode::protocol::mm_supervisor_request::MmSupervisorRequestHeader;

/// Handle a `COMM_UPDATE` request.
///
/// Updates the communication buffer address for future SMI entries.
pub(super) fn handle_comm_update(_comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    log::info!("COMM_UPDATE request");

    // We do not support dynamic communication buffer updates in this implementation, because
    // we expect the runtime allocation will fall into PEI memory bin.
    *comm_buffer_size = MmSupervisorRequestHeader::SIZE;

    efi::Status::ACCESS_DENIED
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn rejects_dynamic_updates_and_returns_only_the_response_header() {
        let mut size = usize::MAX;

        assert_eq!(handle_comm_update(core::ptr::null_mut(), &mut size), efi::Status::ACCESS_DENIED);
        assert_eq!(size, MmSupervisorRequestHeader::SIZE);
    }
}
