//! Request Target Selection for the MM Supervisor Core
//!
//! Derives the dispatch target (user vs supervisor) for an incoming request from the two
//! parallel `MmCommBufferStatus` mailboxes.
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
    /// No pending request (neither mailbox is valid).
    None,
    /// Request targets the Supervisor.
    Supervisor,
    /// Request targets the User module.
    User,
}

impl RequestTarget {
    /// Selects the dispatch target based on the two parallel status mailboxes.
    ///
    /// The user mailbox is checked first - if its `is_comm_buffer_valid` flag
    /// is set, the request belongs to the user module. Otherwise the
    /// supervisor mailbox is consulted. When neither mailbox is valid the
    /// request is treated as an asynchronous MMI, which is still dispatched
    /// through the user path so the user-core's async handler chain can run.
    pub fn select(user_status: &MmCommBufferStatus, supv_status: &MmCommBufferStatus) -> Self {
        if user_status.is_comm_buffer_valid != 0 {
            RequestTarget::User
        } else if supv_status.is_comm_buffer_valid != 0 {
            RequestTarget::Supervisor
        } else {
            // Async MMI - user-core's async dispatcher runs unconditionally
            // once we demote, so route through the user path.
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
        let mut s = MmCommBufferStatus::new();
        s.is_comm_buffer_valid = u8::from(valid);
        s
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
        // Neither mailbox valid => async MMI, dispatched through the user path.
        assert_eq!(RequestTarget::select(&status(false), &status(false)), RequestTarget::User);
    }
}
