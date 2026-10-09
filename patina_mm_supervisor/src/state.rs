//! Global State for the MM Supervisor Core
//!
//! The supervisor's module-level globals, split by who reads them:
//!
//! - [`init`] - the flags and entry-point handles used while bringing cores online
//! - [`security`] - the state every syscall and request handler validates against
//! - [`save_state`] - the save state regions, one for each CPU, and the gated reads Ring 3
//!   makes of them
//!
//! The two accessors are re-exported here, so callers name them as `state::init_state` and
//! `state::security_state` without reaching for the submodule.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub(crate) mod init;
pub(crate) mod save_state;
pub(crate) mod security;

pub(crate) use init::{InitState, init_state};
pub(crate) use security::security_state;
