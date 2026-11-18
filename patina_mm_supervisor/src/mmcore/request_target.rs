//! Request Target Selection for the MM Supervisor Core
//!
//! Picks the dispatch target, user or supervisor, from the two `MmCommBufferStatus` mailboxes.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::management_mode::MmCommBufferStatus;

/// Request target derived from the user and supervisor `MmCommBufferStatus` mailboxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestTarget {
    /// Nothing to dispatch because the communication buffers are not set up yet. Never returned
    /// by [`RequestTarget::select`].
    None,
    /// The request is for the supervisor.
    Supervisor,
    /// The request is for the user core.
    User,
}

impl RequestTarget {
    /// Selects the dispatch target from the two status mailboxes.
    ///
    /// A valid user mailbox wins, then a valid supervisor mailbox. With neither valid the MMI is
    /// asynchronous and still goes to the user core, which runs its async handlers on every
    /// demotion.
    pub fn select(user_status: &MmCommBufferStatus, supv_status: &MmCommBufferStatus) -> Self {
        if user_status.is_comm_buffer_valid != 0 {
            RequestTarget::User
        } else if supv_status.is_comm_buffer_valid != 0 {
            RequestTarget::Supervisor
        } else {
            RequestTarget::User
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    /// Builds a status mailbox with the given validity flag.
    fn status(valid: bool) -> MmCommBufferStatus {
        let mut buffer_status = MmCommBufferStatus::new();
        buffer_status.is_comm_buffer_valid = u8::from(valid);
        buffer_status
    }

    #[test]
    fn test_request_target_user_when_user_valid() {
        assert_eq!(RequestTarget::select(&status(true), &status(false)), RequestTarget::User);
    }

    #[test]
    fn test_request_target_supervisor_when_only_supervisor_valid() {
        assert_eq!(RequestTarget::select(&status(false), &status(true)), RequestTarget::Supervisor);
    }

    #[test]
    fn test_request_target_user_takes_priority_when_both_valid() {
        // The user mailbox is consulted first, so it wins even when both are valid.
        assert_eq!(RequestTarget::select(&status(true), &status(true)), RequestTarget::User);
    }

    #[test]
    fn test_request_target_async_routes_to_user_when_none_valid() {
        // Neither mailbox is valid, so the async MMI goes through the user path.
        assert_eq!(RequestTarget::select(&status(false), &status(false)), RequestTarget::User);
    }
}
