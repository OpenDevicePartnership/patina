//! Platform Initialization (PI) Specification event group GUIDs.
//!
//! Event group GUIDs defined by the Platform Initialization (PI) Specification. Event *types* and
//! the UEFI-defined event groups live in [`crate::uefi::event`].
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::BinaryGuid;

/// End of DXE event group GUID.
///
/// The GUID for the event group signaled at the end of the DXE phase, before BDS.
///
/// Defined in the PI Specification as `gEfiEndOfDxeEventGroupGuid`.
///
/// (`02CE967A-DD7E-4FFC-9EE7-810CF0470880`)
/// ```
/// # use patina::pi::event::END_OF_DXE_EVENT_GROUP_GUID;
/// # assert_eq!("02CE967A-DD7E-4FFC-9EE7-810CF0470880", format!("{}", END_OF_DXE_EVENT_GROUP_GUID));
/// ```
pub const END_OF_DXE_EVENT_GROUP_GUID: BinaryGuid = BinaryGuid::from_string("02CE967A-DD7E-4FFC-9EE7-810CF0470880");
