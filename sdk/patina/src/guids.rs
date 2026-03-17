//! Patina GUID Definitions
//!
//! GUIDs that are used for common and generic events between drivers but are not defined in a formal
//! specification.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// Cache Attribute Change Event Group GUID
///
/// The GUID for an event group signaled when the cache attributes for a memory region are changed. The event group
/// is intended for architectures, such as x86, that require cache attribute changes to be propagated to all APs.
///
/// (`B8E477C7-26A9-4B9A-A7C9-5F8F1F3D9C7B`)
pub const CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP: crate::BinaryGuid =
    crate::BinaryGuid::from_string("B8E477C7-26A9-4B9A-A7C9-5F8F1F3D9C7B");

/// DXE Core Module GUID
///
/// The FFS file GUID for the DXE Core module. Interfaces that depend upon a module GUID such as the Memory Allocation
/// Module HOB and status codes that are produced by the DXE Core module will use this GUID.
///
/// Platforms that integrate the DXE Core module into their firmware volumes should use this GUID to identify the
/// DXE Core FFS file.
///
/// (`23C9322F-2AF2-476A-BC4C-26BC88266C71`)
/// ```
/// # use patina::guids::DXE_CORE;
/// # assert_eq!("23C9322F-2AF2-476A-BC4C-26BC88266C71", format!("{}", DXE_CORE));
/// ```
pub const DXE_CORE: crate::BinaryGuid = crate::BinaryGuid::from_string("23C9322F-2AF2-476A-BC4C-26BC88266C71");

/// Exit Boot Services Failed GUID
///
/// The GUID for an event group signaled when ExitBootServices() fails. For example, the ExitBootServices()
/// implementation may find that the memory map key provided does not match the current memory map key and return
/// an error code. This event group will be signaled in that case just before returning to the caller.
///
/// (`4F6C5507-232F-4787-B95E-72F862490CB1`)
/// ```
/// # use patina::guids::EBS_FAILED;
/// # assert_eq!("4F6C5507-232F-4787-B95E-72F862490CB1", format!("{}", EBS_FAILED));
/// ```
pub const EBS_FAILED: crate::BinaryGuid = crate::BinaryGuid::from_string("4F6C5507-232F-4787-B95E-72F862490CB1");

/// EDKII FPDT (Firmware Performance Data Table) extender firmware performance.
///
/// Use in HOB list to mark a hob as performance reports.
/// Report status code guide for FBPT address.
/// Configuration table guid for the FBPT address.
///
/// (`3B387BFD-7ABC-4CF2-A0CA-B6A16C1B1B25`)
/// ```
/// # use patina::guids::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE;
/// # assert_eq!("3B387BFD-7ABC-4CF2-A0CA-B6A16C1B1B25", format!("{}", EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE));
/// ```
pub const EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE: crate::BinaryGuid =
    crate::BinaryGuid::from_string("3B387BFD-7ABC-4CF2-A0CA-B6A16C1B1B25");

/// Exit Boot Services event group GUID.
///
/// The GUID for the event group signaled when `ExitBootServices()` is called.
///
/// In MM, this is forwarded as an MMI to allow MM drivers to perform cleanup.
///
/// Defined in UEFI/PI as `gEfiEventExitBootServicesGuid`.
///
/// (`27ABF055-B1B8-4C26-8048-748F37BAA2DF`)
/// ```
/// # use patina::{Guid, guids::EVENT_EXIT_BOOT_SERVICES};
/// # assert_eq!("27ABF055-B1B8-4C26-8048-748F37BAA2DF", format!("{:?}", Guid::from_ref(&EVENT_EXIT_BOOT_SERVICES)));
/// ```
pub const EVENT_EXIT_BOOT_SERVICES: crate::BinaryGuid =
    crate::BinaryGuid::from_string("27ABF055-B1B8-4C26-8048-748F37BAA2DF");

/// End of dxe event group GUID.
///
/// (`02CE967A-DD7E-4FFC-9EE7-810CF0470880`)
/// ```
/// # use patina::guids::EVENT_GROUP_END_OF_DXE;
/// # assert_eq!("02CE967A-DD7E-4FFC-9EE7-810CF0470880", format!("{}", EVENT_GROUP_END_OF_DXE));
/// ```
pub const EVENT_GROUP_END_OF_DXE: crate::BinaryGuid =
    crate::BinaryGuid::from_string("02CE967A-DD7E-4FFC-9EE7-810CF0470880");

/// Ready to Boot event group GUID.
///
/// The GUID for the event group signaled when the platform is ready to boot.
///
/// In MM, this is forwarded as an MMI to allow MM drivers to perform final setup.
///
/// Defined in UEFI/PI as `gEfiEventReadyToBootGuid`.
///
/// (`7CE88FB3-4BD7-4679-87A8-A8D8DEE50D2B`)
/// ```
/// # use patina::{Guid, guids::EVENT_READY_TO_BOOT};
/// # assert_eq!("7CE88FB3-4BD7-4679-87A8-A8D8DEE50D2B", format!("{:?}", Guid::from_ref(&EVENT_READY_TO_BOOT)));
/// ```
pub const EVENT_READY_TO_BOOT: crate::BinaryGuid =
    crate::BinaryGuid::from_string("7CE88FB3-4BD7-4679-87A8-A8D8DEE50D2B");

/// Hardware Interrupt protocol GUID.
/// This protocol provides a means of registering and unregistering interrupt handlers for AARCH64 systems.
///
/// (`2890B3EA-053D-1643-AD0C-D64808DA3FF1`)
/// ```
/// # use patina::guids::HARDWARE_INTERRUPT_PROTOCOL;
/// # assert_eq!("2890B3EA-053D-1643-AD0C-D64808DA3FF1", format!("{}", HARDWARE_INTERRUPT_PROTOCOL));
/// ```
pub const HARDWARE_INTERRUPT_PROTOCOL: crate::BinaryGuid =
    crate::BinaryGuid::from_string("2890B3EA-053D-1643-AD0C-D64808DA3FF1");

/// Hardware Interrupt v2 protocol GUID.
/// This protocol provides a means of registering and unregistering interrupt handlers for AARCH64 systems.
/// This protocol extends the Hardware Interrupt Protocol to support interrupt type query.
///
/// (`32898322-2DA1-474A-BAAA-F3F7CF569470`)
/// ```
/// # use patina::guids::HARDWARE_INTERRUPT_PROTOCOL_V2;
/// # assert_eq!("32898322-2DA1-474A-BAAA-F3F7CF569470", format!("{}", HARDWARE_INTERRUPT_PROTOCOL_V2));
/// ```
pub const HARDWARE_INTERRUPT_PROTOCOL_V2: crate::BinaryGuid =
    crate::BinaryGuid::from_string("32898322-2DA1-474A-BAAA-F3F7CF569470");

/// Memory Type Info GUID
///
/// The memory type information HOB and variable can be used to store information
/// for each memory type in Variable or HOB.
///
/// The Memory Type Information GUID can also be optionally used as the Owner
/// field of a Resource Descriptor HOB to provide the preferred memory range
/// for the memory types described in the Memory Type Information GUID HOB.
///
/// (`4C19049F-4137-4DD3-9C10-8B97A83FFDFA`)
/// ```
/// # use patina::guids::MEMORY_TYPE_INFORMATION;
/// # assert_eq!("4C19049F-4137-4DD3-9C10-8B97A83FFDFA", format!("{}", MEMORY_TYPE_INFORMATION));
/// ```
pub const MEMORY_TYPE_INFORMATION: crate::BinaryGuid =
    crate::BinaryGuid::from_string("4C19049F-4137-4DD3-9C10-8B97A83FFDFA");

/// MM Dispatch Event GUID.
///
/// An MMI handler is registered with this GUID to trigger driver dispatch.
///
/// When the supervisor sends an MMI with this GUID, the core attempts to
/// dispatch any previously-discovered-but-not-yet-dispatched drivers.
///
/// Defined in StandaloneMmPkg as `gEventMmDispatchGuid`.
///
/// (`7E6EFFFA-69B4-4C1B-A4C7-AFF9C9244FEE`)
/// ```
/// # use patina::{Guid, guids::MM_DISPATCH_EVENT};
/// # assert_eq!("7E6EFFFA-69B4-4C1B-A4C7-AFF9C9244FEE", format!("{:?}", Guid::from_ref(&MM_DISPATCH_EVENT)));
/// ```
pub const MM_DISPATCH_EVENT: crate::BinaryGuid = crate::BinaryGuid::from_string("7E6EFFFA-69B4-4C1B-A4C7-AFF9C9244FEE");

/// DXE MM Ready To Lock Protocol GUID.
///
/// This protocol GUID is used to signal that the DXE phase is ready to lock
/// down MM. When an MMI with this GUID is received, the MM core begins the
/// ready-to-lock sequence.
///
/// Defined in PI as `gEfiDxeMmReadyToLockProtocolGuid`.
///
/// (`60FF8964-E906-41D0-AFED-F241E974E08E`)
/// ```
/// # use patina::{Guid, guids::MM_DXE_READY_TO_LOCK_PROTOCOL};
/// # assert_eq!("60FF8964-E906-41D0-AFED-F241E974E08E", format!("{:?}", Guid::from_ref(&MM_DXE_READY_TO_LOCK_PROTOCOL)));
/// ```
pub const MM_DXE_READY_TO_LOCK_PROTOCOL: crate::BinaryGuid =
    crate::BinaryGuid::from_string("60FF8964-E906-41D0-AFED-F241E974E08E");

/// MM End of DXE Protocol GUID.
///
/// This protocol is installed in the MM handle database when an End-of-DXE MMI
/// is received. MM drivers can register a protocol notification for this GUID
/// to perform actions that must happen after all DXE drivers have been dispatched
/// but before 3rd-party OpROMs execute.
///
/// Defined in PI as `gEfiMmEndOfDxeProtocolGuid`.
///
/// (`24E70042-D5C5-4260-8C39-0AD3AA32E93D`)
/// ```
/// # use patina::{Guid, guids::MM_END_OF_DXE_PROTOCOL};
/// # assert_eq!("24E70042-D5C5-4260-8C39-0AD3AA32E93D", format!("{:?}", Guid::from_ref(&MM_END_OF_DXE_PROTOCOL)));
/// ```
pub const MM_END_OF_DXE_PROTOCOL: crate::BinaryGuid =
    crate::BinaryGuid::from_string("24E70042-D5C5-4260-8C39-0AD3AA32E93D");

/// MM End of PEI Protocol GUID.
///
/// This protocol is installed in the MM handle database when an End-of-PEI MMI
/// is received. It signals that the PEI phase has completed.
///
/// Defined in PI as `gEfiMmEndOfPeiProtocol`.
///
/// (`F33E1BF3-980B-4BFB-A29A-B29C86453732`)
/// ```
/// # use patina::{Guid, guids::MM_END_OF_PEI_PROTOCOL};
/// # assert_eq!("F33E1BF3-980B-4BFB-A29A-B29C86453732", format!("{:?}", Guid::from_ref(&MM_END_OF_PEI_PROTOCOL)));
/// ```
pub const MM_END_OF_PEI_PROTOCOL: crate::BinaryGuid =
    crate::BinaryGuid::from_string("F33E1BF3-980B-4BFB-A29A-B29C86453732");

/// MM Ready To Lock Protocol GUID.
///
/// This protocol is installed in the MM handle database when the ready-to-lock
/// handler runs. MM drivers can register a protocol notification for this GUID
/// to be informed that MMRAM is about to be locked.
///
/// Defined in PI as `gEfiMmReadyToLockProtocolGuid`.
///
/// (`47B7FA8C-F4BD-4AF6-8200-333086F0D2C8`)
/// ```
/// # use patina::{Guid, guids::MM_READY_TO_LOCK_PROTOCOL};
/// # assert_eq!("47B7FA8C-F4BD-4AF6-8200-333086F0D2C8", format!("{:?}", Guid::from_ref(&MM_READY_TO_LOCK_PROTOCOL)));
/// ```
pub const MM_READY_TO_LOCK_PROTOCOL: crate::BinaryGuid =
    crate::BinaryGuid::from_string("47B7FA8C-F4BD-4AF6-8200-333086F0D2C8");

/// Performance Protocol GUID.
///
/// This protocol provides a means of adding performace record to the Firmware Basic Boot Performance Table (FBPT).
///
/// (`76B6BDFA-2ACD-4462-9E3F-CB58C969D937`)
/// ```
/// # use patina::guids::PERFORMANCE_PROTOCOL;
/// # assert_eq!("76B6BDFA-2ACD-4462-9E3F-CB58C969D937", format!("{}", PERFORMANCE_PROTOCOL));
/// ```
pub const PERFORMANCE_PROTOCOL: crate::BinaryGuid =
    crate::BinaryGuid::from_string("76B6BDFA-2ACD-4462-9E3F-CB58C969D937");

/// EFI SMM Communication Protocol GUID as defined in the PI 1.2 specification.
///
/// This protocol provides a means of communicating between drivers outside of SMM and SMI
/// handlers inside of SMM.
///
/// (`C68ED8E2-9DC6-4CBD-9D94-DB65ACC5C332`)
/// ```
/// # use patina::guids::SMM_COMMUNICATION_PROTOCOL;
/// # assert_eq!("C68ED8E2-9DC6-4CBD-9D94-DB65ACC5C332", format!("{}", SMM_COMMUNICATION_PROTOCOL));
/// ```
pub const SMM_COMMUNICATION_PROTOCOL: crate::BinaryGuid =
    crate::BinaryGuid::from_string("C68ED8E2-9DC6-4CBD-9D94-DB65ACC5C332");

/// Zero GUID
///
/// All-zero GUID, used as a marker or placeholder.
///
/// (`00000000-0000-0000-0000-000000000000`)
/// ```
/// # use patina::guids::ZERO;
/// # assert_eq!("00000000-0000-0000-0000-000000000000", format!("{}", ZERO));
/// ```
pub const ZERO: crate::BinaryGuid = crate::BinaryGuid::from_string("00000000-0000-0000-0000-000000000000");

/// EFI_HOB_MEMORY_ALLOC_STACK_GUID
///
///  Describes the memory stack that is produced by the HOB producer phase and upon which all post
///  memory-installed executable content in the HOB producer phase is executing.
///
/// (`4ED4BF27-4092-42E9-807D-527B1D00C9BD`)
/// ```
/// # use patina::guids::HOB_MEMORY_ALLOC_STACK;
/// # assert_eq!("4ED4BF27-4092-42E9-807D-527B1D00C9BD", format!("{}", HOB_MEMORY_ALLOC_STACK));
/// ```
pub const HOB_MEMORY_ALLOC_STACK: crate::BinaryGuid =
    crate::BinaryGuid::from_string("4ED4BF27-4092-42E9-807D-527B1D00C9BD");

/// EFI HOB List GUID
///
/// The GUID used to identify the HOB list when it is installed as a configuration table entry
/// in the EFI System Table or the MM System Table. Drivers can locate the HOB list by searching
/// the configuration table for this GUID.
///
/// Defined in the PI Specification as `gEfiHobListGuid`.
///
/// (`7739F24C-93D7-11D4-9A3A-0090273FC14D`)
/// ```
/// # use patina::{Guid, guids::HOB_LIST};
/// # assert_eq!("7739F24C-93D7-11D4-9A3A-0090273FC14D", format!("{:?}", Guid::from_ref(&HOB_LIST)));
/// ```
pub const HOB_LIST: crate::BinaryGuid = crate::BinaryGuid::from_string("7739F24C-93D7-11D4-9A3A-0090273FC14D");
