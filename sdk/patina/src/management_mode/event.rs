//! Management Mode (MM) event group GUIDs.
//!
//! Event group GUIDs used to coordinate the DXE and MM environments.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::BinaryGuid;

/// MM Dispatch event group GUID.
///
/// An MMI handler is registered with this GUID to trigger driver dispatch.
///
/// When the supervisor sends an MMI with this GUID, the core attempts to
/// dispatch any previously-discovered-but-not-yet-dispatched drivers.
///
/// Defined in `StandaloneMmPkg` as `gEventMmDispatchGuid`.
///
/// (`7E6EFFFA-69B4-4C1B-A4C7-AFF9C9244FEE`)
/// ```
/// # use patina::{Guid, management_mode::event::MM_DISPATCH_EVENT_GROUP_GUID};
/// # assert_eq!(
/// #     "7E6EFFFA-69B4-4C1B-A4C7-AFF9C9244FEE",
/// #     format!("{:?}", Guid::from_ref(&MM_DISPATCH_EVENT_GROUP_GUID))
/// # );
/// ```
pub const MM_DISPATCH_EVENT_GROUP_GUID: BinaryGuid = BinaryGuid::from_string("7E6EFFFA-69B4-4C1B-A4C7-AFF9C9244FEE");
