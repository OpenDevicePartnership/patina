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

/// Runtime Arch Protocol GUID.
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
    /// List head for registered runtime images.
    pub image_head: list_entry::Entry,
    /// List head for registered runtime events.
    pub event_head: list_entry::Entry,
    /// Size of each memory descriptor.
    pub memory_descriptor_size: usize,
    /// Version of the memory descriptor.
    pub memory_descriptor_version: u32,
    /// Total size of the memory map.
    pub memory_map_size: usize,
    /// Physical address of the memory map.
    pub memory_map_physical: *mut efi::MemoryDescriptor,
    /// Virtual address of the memory map.
    pub memory_map_virtual: *mut efi::MemoryDescriptor,
    /// Whether virtual addressing mode is active.
    pub virtual_mode: AtomicBool,
    /// Whether system is at runtime.
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
    /// Base address of the image.
    pub image_base: *mut c_void,
    /// Size of the image.
    pub image_size: u64,
    /// Relocation data for the image.
    pub relocation_data: *mut c_void,
    /// Handle associated with the image.
    pub handle: efi::Handle,
    /// Link entry in the image list.
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
    /// Type of the runtime event.
    pub event_type: u32,
    /// Task priority level for the event.
    pub notify_tpl: efi::Tpl,
    /// Notification function for the event.
    pub notify_function: efi::EventNotify,
    /// Context data for the event notification.
    pub context: *mut c_void,
    /// Event handle.
    pub event: efi::Event,
    /// Link entry in the image list.
    pub link: list_entry::Entry,
}
