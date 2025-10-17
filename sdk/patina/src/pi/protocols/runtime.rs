//! Runtime Architectural Protocol
//!
//! Contains the UEFI runtime services that are callable only in physical mode.
//!
//! See <https://uefi.org/specs/PI/1.8A/V2_DXE_Architectural_Protocols.html#runtime-architectural-protocol>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{ffi::c_void, sync::atomic::AtomicBool};

use crate::pi::list_entry;
use r_efi::efi;

pub const PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0xb7dfb4e1, 0x052f, 0x449f, 0x87, 0xbe, &[0x98, 0x18, 0xfc, 0x91, 0xb7, 0x33]);

/// Allows the runtime functionality of the DXE Foundation to be contained
/// in a separate driver. It also provides hooks for the DXE Foundation to
/// export information that is needed at runtime. As such, this protocol allows
/// services to the DXE Foundation to manage runtime drivers and events.
/// This protocol also implies that the runtime services required to transition
/// to virtual mode, SetVirtualAddressMap() and ConvertPointer(), have been
/// registered into the UEFI Runtime Table in the UEFI System Table. This protocol
/// must be produced by a runtime DXE driver and may only be consumed by the DXE Foundation.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.8.1
#[repr(C)]
#[derive(Debug)]
pub struct Protocol {
    pub image_head: list_entry::Entry,
    pub event_head: list_entry::Entry,
    pub memory_descriptor_size: usize,
    pub memory_descriptor_version: u32,
    pub memory_map_size: usize,
    pub memory_map_physical: *mut efi::MemoryDescriptor,
    pub memory_map_virtual: *mut efi::MemoryDescriptor,
    pub virtual_mode: AtomicBool,
    pub at_runtime: AtomicBool,
}

/// Related definition for runtime architectural protocol as the entry type
/// for the image list.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.8.1
#[repr(C)]
#[derive(Debug)]
pub struct ImageEntry {
    pub image_base: *mut c_void,
    pub image_size: u64,
    pub relocation_data: *mut c_void,
    pub handle: efi::Handle,
    pub link: list_entry::Entry,
}

/// Related definition for runtime architectural protocol as the entry type
/// for the event list.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.8.1
#[repr(C)]
#[derive(Debug)]
pub struct EventEntry {
    pub event_type: u32,
    pub notify_tpl: efi::Tpl,
    pub notify_function: efi::EventNotify,
    pub context: *mut c_void,
    pub event: efi::Event,
    pub link: list_entry::Entry,
}
