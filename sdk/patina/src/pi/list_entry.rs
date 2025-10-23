//! Linked List Entry
//!
//! Defined in the PI Specification as an EFI Linked List entry (EfiListEntry). See Related Definitions for the
//! Runtime Architectural Protocol.
//!
//! Represents a doubly linked list where with forward and back links.
//!
//! See <https://uefi.org/specs/PI/1.8A/V2_DXE_Architectural_Protocols.html#efi-runtime-arch-protocol>.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#[repr(C)]
#[derive(Debug)]
/// Doubly-linked list entry.
pub struct Entry {
    /// Forward link pointer to the next entry in the list.
    pub forward_link: *mut Entry,
    /// Backward link pointer to the previous entry in the list.
    pub back_link: *mut Entry,
}
