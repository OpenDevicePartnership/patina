//! Global State for the MM Supervisor Core
//!
//! - [`init`] - flags and entry-point handles used while bringing cores online
//! - [`security`] - state the syscall and request handlers validate against
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub(crate) mod init;
pub(crate) mod security;

pub(crate) use init::{InitState, init_state};
pub(crate) use security::security_state;
