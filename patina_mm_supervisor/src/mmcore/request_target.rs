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
