//! Global State for the MM Supervisor Core
//!
//! The supervisor's module-level globals, split by who reads them:
//!
//! - [`init`] - the flags and entry-point handles used while bringing cores online
//! - [`security`] - the state every syscall and request handler validates against
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
