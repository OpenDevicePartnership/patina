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
