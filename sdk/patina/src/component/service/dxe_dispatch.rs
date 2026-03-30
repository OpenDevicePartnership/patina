//! DXE Dispatch Service Definition.
//!
//! This module contains the [`DxeDispatch`] trait for services that expose
//! DXE driver dispatch capability. See [`DxeDispatch`] for the primary interface.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

use crate::error::Result;

/// Service interface for DXE driver dispatch.
///
/// Provides access to the PI dispatcher for components that need to trigger
/// additional driver dispatch passes beyond the core's built-in dispatch loop
/// (e.g., to interleave controller connection with driver dispatch during boot).
///
/// Note: The DXE core already runs a PI dispatch loop automatically. This
/// service is only needed when a component must explicitly trigger a dispatch
/// pass at a specific point in its execution.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait DxeDispatch {
    /// Performs a single DXE driver dispatch pass.
    ///
    /// Returns `true` if any drivers were dispatched, `false` if no drivers were dispatched.
    fn dispatch(&self) -> Result<bool>;
}
