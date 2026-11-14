//! Request Target Selection for the MM Supervisor Core
//!
//! Derives the dispatch target (user or supervisor) for an incoming request
//! from the two parallel `MmCommBufferStatus` mailboxes.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::management_mode::MmCommBufferStatus;

/// Request target derived from the user and supervisor `MmCommBufferStatus`
/// mailboxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestTarget {
    /// Nothing to dispatch: the communication buffers or both status mailboxes are not set up yet.
    /// [`RequestTarget::select`] never returns this; a request with neither mailbox valid is an
    /// asynchronous MMI routed to [`RequestTarget::User`].
    None,
    /// The request is for the supervisor.
    Supervisor,
    /// The request is for the user core.
    User,
}

impl RequestTarget {
    /// Selects the dispatch target from the two status mailboxes.
    ///
    /// The user mailbox wins if its `is_comm_buffer_valid` flag is set. Otherwise the
    /// supervisor mailbox is checked. When neither is valid the request is an asynchronous MMI,
    /// which still goes through the user path so the user core's async handlers run.
    pub fn select(user_status: &MmCommBufferStatus, supv_status: &MmCommBufferStatus) -> Self {
        if user_status.is_comm_buffer_valid != 0 {
            RequestTarget::User
        } else if supv_status.is_comm_buffer_valid != 0 {
            RequestTarget::Supervisor
        } else {
            // The user core runs its async dispatcher on every demotion.
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
