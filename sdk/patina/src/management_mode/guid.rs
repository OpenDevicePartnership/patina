//! Management Mode (MM) GUID Constants
//!
//! GUID constants for MM/SMM protocols used to coordinate the DXE and MM environments. Event
//! group GUIDs live in [`crate::management_mode::event`].
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::BinaryGuid;

/// DXE MM Ready To Lock protocol GUID.
///
/// This protocol GUID is used to signal that the DXE phase is ready to lock
/// down MM. When an MMI with this GUID is received, the MM core begins the
/// ready-to-lock sequence.
///
/// Defined in PI as `gEfiDxeMmReadyToLockProtocolGuid`.
///
/// (`60FF8964-E906-41D0-AFED-F241E974E08E`)
/// ```
/// # use patina::{Guid, management_mode::guid::MM_DXE_READY_TO_LOCK_PROTOCOL_GUID};
/// # assert_eq!(
/// #     "60FF8964-E906-41D0-AFED-F241E974E08E",
/// #     format!("{:?}", Guid::from_ref(&MM_DXE_READY_TO_LOCK_PROTOCOL_GUID))
/// # );
/// ```
pub const MM_DXE_READY_TO_LOCK_PROTOCOL_GUID: BinaryGuid =
    BinaryGuid::from_string("60FF8964-E906-41D0-AFED-F241E974E08E");

/// MM End of DXE protocol GUID.
///
/// This protocol is installed in the MM handle database when an End-of-DXE MMI
/// is received. MM drivers can register a protocol notification for this GUID
/// to perform actions that must happen after all DXE drivers have been dispatched
/// but before 3rd-party `OpROMs` execute.
///
/// Defined in PI as `gEfiMmEndOfDxeProtocolGuid`.
///
/// (`24E70042-D5C5-4260-8C39-0AD3AA32E93D`)
/// ```
/// # use patina::{Guid, management_mode::guid::MM_END_OF_DXE_PROTOCOL_GUID};
/// # assert_eq!(
/// #     "24E70042-D5C5-4260-8C39-0AD3AA32E93D",
/// #     format!("{:?}", Guid::from_ref(&MM_END_OF_DXE_PROTOCOL_GUID))
/// # );
/// ```
pub const MM_END_OF_DXE_PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("24E70042-D5C5-4260-8C39-0AD3AA32E93D");

/// MM End of PEI protocol GUID.
///
/// This protocol is installed in the MM handle database when an End-of-PEI MMI
/// is received. It signals that the PEI phase has completed.
///
/// Defined in PI as `gEfiMmEndOfPeiProtocol`.
///
/// (`F33E1BF3-980B-4BFB-A29A-B29C86453732`)
/// ```
/// # use patina::{Guid, management_mode::guid::MM_END_OF_PEI_PROTOCOL_GUID};
/// # assert_eq!(
/// #     "F33E1BF3-980B-4BFB-A29A-B29C86453732",
/// #     format!("{:?}", Guid::from_ref(&MM_END_OF_PEI_PROTOCOL_GUID))
/// # );
/// ```
pub const MM_END_OF_PEI_PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("F33E1BF3-980B-4BFB-A29A-B29C86453732");

/// MM Ready To Lock protocol GUID.
///
/// This protocol is installed in the MM handle database when the ready-to-lock
/// handler runs. MM drivers can register a protocol notification for this GUID
/// to be informed that MMRAM is about to be locked.
///
/// Defined in PI as `gEfiMmReadyToLockProtocolGuid`.
///
/// (`47B7FA8C-F4BD-4AF6-8200-333086F0D2C8`)
/// ```
/// # use patina::{Guid, management_mode::guid::MM_READY_TO_LOCK_PROTOCOL_GUID};
/// # assert_eq!(
/// #     "47B7FA8C-F4BD-4AF6-8200-333086F0D2C8",
/// #     format!("{:?}", Guid::from_ref(&MM_READY_TO_LOCK_PROTOCOL_GUID))
/// # );
/// ```
pub const MM_READY_TO_LOCK_PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("47B7FA8C-F4BD-4AF6-8200-333086F0D2C8");

/// EFI SMM Communication protocol GUID as defined in the PI 1.2 specification.
///
/// This protocol provides a means of communicating between drivers outside of SMM and SMI
/// handlers inside of SMM.
///
/// (`C68ED8E2-9DC6-4CBD-9D94-DB65ACC5C332`)
/// ```
/// # use patina::management_mode::guid::SMM_COMMUNICATION_PROTOCOL_GUID;
/// # assert_eq!(
/// #     "C68ED8E2-9DC6-4CBD-9D94-DB65ACC5C332",
/// #     format!("{}", SMM_COMMUNICATION_PROTOCOL_GUID)
/// # );
/// ```
pub const SMM_COMMUNICATION_PROTOCOL_GUID: BinaryGuid = crate::pi::protocol::communication::PROTOCOL_GUID;
