//! Platform Initialization (PI) Specification GUID Constants
//!
//! GUID constants defined by the Platform Initialization (PI) Specification that do not have a
//! more specific home. Event group GUIDs live in [`crate::pi::event`], protocol GUIDs live with
//! their protocol in [`crate::pi::protocol`], and HOB payload GUIDs live with their HOB
//! definition in [`crate::pi::hob`].
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::BinaryGuid;

/// EFI HOB List configuration-table GUID.
///
/// The GUID used to identify the HOB list when it is installed as a configuration table entry
/// in the EFI System Table or the MM System Table. Drivers can locate the HOB list by searching
/// the configuration table for this GUID.
///
/// Defined in the PI Specification as `gEfiHobListGuid`.
///
/// (`7739F24C-93D7-11D4-9A3A-0090273FC14D`)
/// ```
/// # use patina::{Guid, pi::guid::HOB_LIST_TABLE_GUID};
/// # assert_eq!("7739F24C-93D7-11D4-9A3A-0090273FC14D", format!("{:?}", Guid::from_ref(&HOB_LIST_TABLE_GUID)));
/// ```
pub const HOB_LIST_TABLE_GUID: BinaryGuid = BinaryGuid::from_string("7739F24C-93D7-11D4-9A3A-0090273FC14D");

/// `EFI_HOB_MEMORY_ALLOC_STACK_GUID`
///
/// Describes the memory stack that is produced by the HOB producer phase and upon which all post
/// memory-installed executable content in the HOB producer phase is executing.
///
/// Defined in the PI Specification as `gEfiHobMemoryAllocStackGuid`.
///
/// (`4ED4BF27-4092-42E9-807D-527B1D00C9BD`)
/// ```
/// # use patina::pi::guid::MEMORY_ALLOC_STACK_HOB_GUID;
/// # assert_eq!("4ED4BF27-4092-42E9-807D-527B1D00C9BD", format!("{}", MEMORY_ALLOC_STACK_HOB_GUID));
/// ```
pub const MEMORY_ALLOC_STACK_HOB_GUID: BinaryGuid = BinaryGuid::from_string("4ED4BF27-4092-42E9-807D-527B1D00C9BD");
