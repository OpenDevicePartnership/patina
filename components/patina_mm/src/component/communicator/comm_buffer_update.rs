//! MM Communication Buffer Update Module
//!
//! This module isolates functionality for updating MM communication buffers via protocol notification.
//! The buffer update feature is opt-in via configuration and provides a mechanism for firmware to
//! dynamically update communication buffer addresses at runtime.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::{
    component::communicator::MmCommunicator,
    config::CommunicateBuffer,
    protocol::mm_comm_buffer_update::{self, MmCommBufferUpdateProtocol},
};
use patina::{
    base::UEFI_PAGE_SIZE,
    boot_services::{BootServices, StandardBootServices, event::EventType, tpl::Tpl},
};
use zerocopy::FromBytes;

use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

extern crate alloc;
use alloc::boxed::Box;

/// Context for the MM Comm Buffer Update Protocol notify callback
///
/// This context is shared between the protocol callback and the communicate() method.
/// When a protocol callback triggers, it stores the pending buffer update atomically.
/// The next communicate() call will apply the pending update.
#[repr(C)]
pub(super) struct ProtocolNotifyContext {
    pub(super) boot_services: StandardBootServices,
    pub(super) updatable_buffer_id: u8,
    pub(super) communicator: *const MmCommunicator,
    /// Pending buffer update - set by protocol callback, consumed by communicate()
    pub(super) pending_buffer: AtomicPtr<CommunicateBuffer>,
    /// Flag indicating if a buffer update is pending
    pub(super) has_pending_update: AtomicBool,
}

/// Register protocol notify callback for MM Communication Buffer Updates
///
/// This function sets up the protocol notification that will be triggered when
/// the MM Communication Buffer Update Protocol is installed.
///
/// # Parameters
/// - `boot_services`: Boot services for creating events and registering protocol notify
/// - `updatable_buffer_id`: The buffer ID that should be updated when protocol is installed
/// - `communicator`: Pointer to the MmCommunicator instance
///
/// # Returns
/// - `Ok(&'static ProtocolNotifyContext)`: Context that should be stored for later use
/// - `Err(patina::error::Error)`: If event creation or protocol notify registration fails
///
/// # Safety
/// - The communicator pointer must remain valid for the lifetime of the context
/// - The returned context is leaked and will live for a static lifetime
pub(super) fn register_buffer_update_notify(
    boot_services: StandardBootServices,
    updatable_buffer_id: u8,
    communicator: *const MmCommunicator,
) -> patina::error::Result<&'static ProtocolNotifyContext> {
    log::trace!(target: "mm_comm", "Setting up protocol notify callback for buffer ID {}", updatable_buffer_id);

    let context = Box::leak(Box::new(ProtocolNotifyContext {
        boot_services: boot_services.clone(),
        updatable_buffer_id,
        communicator,
        pending_buffer: AtomicPtr::new(core::ptr::null_mut()),
        has_pending_update: AtomicBool::new(false),
    }));

    let event = context.boot_services.create_event(
        EventType::NOTIFY_SIGNAL,
        Tpl::CALLBACK,
        Some(protocol_notify_callback),
        context,
    )?;

    log::trace!(target: "mm_comm", "Registering protocol notify - callback may fire synchronously");
    context.boot_services.register_protocol_notify(mm_comm_buffer_update::GUID.as_efi_guid(), event)?;
    log::debug!(
        target: "mm_comm",
        "Registered protocol notify on {} with updatable_buffer_id={}",
        mm_comm_buffer_update::GUID,
        updatable_buffer_id
    );

    Ok(context)
}

/// Apply any pending buffer update if available
///
/// This function checks if a pending buffer update is available (set by the protocol callback)
/// and applies it if needed. It should be called from communicate() before processing
/// the communication request.
///
/// # Parameters
/// - `context`: The protocol notify context containing pending buffer information
/// - `comm_buffers`: Mutable reference to the vector of communication buffers
///
/// # Returns
/// - `true` if a buffer update was applied
/// - `false` if no update was pending
pub(super) fn apply_pending_buffer_update(
    context: &ProtocolNotifyContext,
    comm_buffers: &mut alloc::vec::Vec<CommunicateBuffer>,
) -> bool {
    if !context.has_pending_update.load(Ordering::Acquire) {
        return false;
    }

    log::info!(target: "mm_comm", "Pending buffer update detected, applying now");

    // Retrieve the pending buffer atomically
    let pending_ptr = context.pending_buffer.swap(core::ptr::null_mut(), Ordering::Acquire);
    if pending_ptr.is_null() {
        log::warn!(target: "mm_comm", "Pending update flag set but no buffer found");
        context.has_pending_update.store(false, Ordering::Release);
        return false;
    }

    // SAFETY: We created this pointer in the protocol callback via Box::into_raw
    let new_buffer = unsafe { *Box::from_raw(pending_ptr) };
    let updatable_buffer_id = new_buffer.id();

    // Disable any existing buffer with the same ID
    if let Some(old_buffer) = comm_buffers.iter_mut().find(|b| b.id() == updatable_buffer_id && b.is_enabled()) {
        log::info!(
            target: "mm_comm",
            "Disabling old comm buffer {}: addr={:p}, size=0x{:X}",
            updatable_buffer_id,
            old_buffer.as_ptr(),
            old_buffer.len()
        );
        old_buffer.disable();
    }

    // Add the new enabled buffer
    log::info!(
        target: "mm_comm",
        "Adding new comm buffer {}: addr={:p}, size=0x{:X}",
        updatable_buffer_id,
        new_buffer.as_ptr(),
        new_buffer.len()
    );
    comm_buffers.push(new_buffer);
    log::info!(target: "mm_comm", "Successfully applied pending comm buffer {} update", updatable_buffer_id);

    // Clear the pending flag
    context.has_pending_update.store(false, Ordering::Release);
    true
}

/// Protocol notification callback function
///
/// This callback is triggered when the MM Communication Buffer Update Protocol is installed.
/// It reads the protocol data, validates the communication buffer information, and stores
/// the buffer update. The update will be applied by communicate().
///
/// ## Coverage
///
/// Note: register_buffer_update_notify() and protocol_notify_callback() are difficult to unit test because they
/// require:
///
/// 1. UEFI boot services with working event creation and protocol notification services
/// 2. A protocol database with functional protocol lookup
/// 3. Raw pointer manipulation of protocol data
///
/// ELements of the protocol update process are unit tested but the notification function as a whole is not.
#[coverage(off)]
extern "efiapi" fn protocol_notify_callback(_event: r_efi::efi::Event, context: &'static ProtocolNotifyContext) {
    log::trace!(target: "mm_comm", "=== Protocol callback ENTRY === communicator ptr: {:p}", context.communicator);
    log::info!(target: "mm_comm", "Protocol notify callback triggered for {}", mm_comm_buffer_update::GUID);

    let updatable_buffer_id = context.updatable_buffer_id;
    log::debug!(target: "mm_comm", "Updatable buffer ID: {}", updatable_buffer_id);

    // SAFETY: The boot_services pointer is passed in via ProtocolNotifyContext construction. A valid GUID reference
    // is used.
    let protocol_ptr = match unsafe {
        context
            .boot_services
            .locate_protocol_unchecked(mm_comm_buffer_update::GUID.as_efi_guid(), core::ptr::null_mut())
    } {
        Ok(ptr) => ptr,
        Err(status) => {
            log::error!(target: "mm_comm", "Failed to locate protocol: status={:?}", status);
            return;
        }
    };

    if protocol_ptr.is_null() {
        log::error!(target: "mm_comm", "Protocol pointer is null");
        return;
    }

    // SAFETY: The protocol pointer found with locate_protocol should be a valid
    // protocol database entry. The pointer was checked for null above.
    let protocol_data = unsafe {
        let protocol_bytes =
            core::slice::from_raw_parts(protocol_ptr as *const u8, core::mem::size_of::<MmCommBufferUpdateProtocol>());

        match MmCommBufferUpdateProtocol::read_from_bytes(protocol_bytes) {
            Ok(data) => data,
            Err(e) => {
                log::error!(target: "mm_comm", "Failed to parse protocol data: {:?}", e);
                return;
            }
        }
    };

    // Copying packed fields to local variables to avoid unaligned references
    let version = protocol_data.version;
    let physical_start = protocol_data.updated_comm_buffer.physical_start;
    let size_pages = protocol_data.updated_comm_buffer.number_of_pages;
    let size_bytes = size_pages * UEFI_PAGE_SIZE as u64;
    let status_address = protocol_data.updated_comm_buffer.status;

    log::info!(
        target: "mm_comm",
        "Received MM comm buffer update: version={}, addr=0x{:X}, size={} pages (0x{:X} bytes), status=0x{:X}",
        version,
        physical_start,
        size_pages,
        size_bytes,
        status_address
    );

    // Validate and create the new buffer from the protocol
    // SAFETY: The firmware providing this protocol guarantees the memory region is valid
    let new_buffer = match unsafe {
        CommunicateBuffer::from_firmware_region(
            physical_start,
            size_bytes as usize,
            updatable_buffer_id,
            Some(status_address),
        )
    } {
        Ok(buffer) => {
            log::info!(
                target: "mm_comm",
                "Successfully validated comm buffer from protocol: id={}, addr={:p}, size=0x{:X}",
                buffer.id(),
                buffer.as_ptr(),
                buffer.len()
            );
            buffer
        }
        Err(err) => {
            log::error!(target: "mm_comm", "Failed to validate comm buffer from protocol data: {:?}", err);
            return;
        }
    };

    // Store the pending buffer update
    // The next communicate() call will apply this update
    let buffer_box = Box::new(new_buffer);
    let buffer_ptr = Box::into_raw(buffer_box);

    // If there's already a pending buffer, free it first
    let old_ptr = context.pending_buffer.swap(buffer_ptr, Ordering::Release);
    if !old_ptr.is_null() {
        log::warn!(target: "mm_comm", "Replacing previous pending buffer update.");
        // SAFETY: old_ptr was created via Box::into_raw and is valid and properly aligned.
        // The box is reconstructed here to drop it.
        unsafe {
            drop(Box::from_raw(old_ptr));
        }
    }

    // Signal that a pending update is available
    context.has_pending_update.store(true, Ordering::Release);
    log::info!(target: "mm_comm", "Buffer update stored atomically, will be applied by next communicate() call");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{component::communicator::MmCommunicator, config::CommunicateBuffer};

    use core::{
        pin::Pin,
        sync::atomic::{AtomicBool, AtomicPtr, Ordering},
    };
    use patina::boot_services::StandardBootServices;

    extern crate alloc;
    use alloc::{boxed::Box, vec};

    /// Helper to create a test protocol notify context without boot services
    fn create_test_context(
        updatable_buffer_id: u8,
        communicator_ptr: *const MmCommunicator,
    ) -> Box<ProtocolNotifyContext> {
        let mock_bs = Box::leak(Box::new([0u8; core::mem::size_of::<r_efi::system::BootServices>()]));
        let bs_ptr = mock_bs.as_ptr() as *const r_efi::system::BootServices;
        // SAFETY: bs_ptr points to an aligned mock buffer that was just created.
        // This is only used for test purposes and the pointer lifetime is managed by the leaked Box.
        let bs = StandardBootServices::new(unsafe { &*bs_ptr });

        Box::new(ProtocolNotifyContext {
            boot_services: bs,
            updatable_buffer_id,
            communicator: communicator_ptr,
            pending_buffer: AtomicPtr::new(core::ptr::null_mut()),
            has_pending_update: AtomicBool::new(false),
        })
    }

    #[test]
    fn test_apply_pending_buffer_update_no_pending_update() {
        let context = create_test_context(0, core::ptr::null());
        let mut comm_buffers = vec![];

        // No pending update should return false
        let result = apply_pending_buffer_update(&context, &mut comm_buffers);
        assert!(!result);
    }

    #[test]
    fn test_apply_pending_buffer_update_with_pending_buffer() {
        let context = create_test_context(5, core::ptr::null());

        // Create a new buffer to be the pending update
        let new_buffer = CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 4096]))), 5);
        let buffer_ptr = Box::into_raw(Box::new(new_buffer));

        context.pending_buffer.store(buffer_ptr, Ordering::Release);
        context.has_pending_update.store(true, Ordering::Release);

        let mut comm_buffers = vec![];

        let result = apply_pending_buffer_update(&context, &mut comm_buffers);
        assert!(result);

        // Verify the buffer was added
        assert_eq!(comm_buffers.len(), 1);
        assert_eq!(comm_buffers[0].id(), 5);

        // Verify the pending update was cleared
        assert!(!context.has_pending_update.load(Ordering::Acquire));
        assert!(context.pending_buffer.load(Ordering::Acquire).is_null());
    }

    #[test]
    fn test_apply_pending_buffer_update_replaces_existing_buffer() {
        let context = create_test_context(3, core::ptr::null());

        // Create existing buffer with ID 3
        let old_buffer = CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0xAA; 1024]))), 3);
        let mut comm_buffers = vec![old_buffer];

        // Verify old buffer is enabled
        assert!(comm_buffers[0].is_enabled());

        // Create new buffer with the same ID
        let new_buffer = CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0xBB; 2048]))), 3);
        let buffer_ptr = Box::into_raw(Box::new(new_buffer));

        context.pending_buffer.store(buffer_ptr, Ordering::Release);
        context.has_pending_update.store(true, Ordering::Release);

        // Apply the pending update
        let result = apply_pending_buffer_update(&context, &mut comm_buffers);
        assert!(result);

        // Verify both buffers are present (old disabled and new enabled)
        assert_eq!(comm_buffers.len(), 2);

        // The first buffer should be disabled
        assert_eq!(comm_buffers[0].id(), 3);
        assert!(!comm_buffers[0].is_enabled());

        // The second buffer should be enabled
        assert_eq!(comm_buffers[1].id(), 3);
        assert!(comm_buffers[1].is_enabled());
        assert_eq!(comm_buffers[1].len(), 2048);
    }

    #[test]
    fn test_apply_pending_buffer_update_flag_set_but_no_buffer() {
        let context = create_test_context(0, core::ptr::null());

        // Set the flag but don't store a buffer
        context.has_pending_update.store(true, Ordering::Release);

        let mut comm_buffers = vec![];

        // It should return false and clear the pending update flag
        let result = apply_pending_buffer_update(&context, &mut comm_buffers);
        assert!(!result);
        assert!(!context.has_pending_update.load(Ordering::Acquire));
    }

    #[test]
    fn test_protocol_notify_context_creation() {
        let communicator_ptr: *const MmCommunicator = 0x1000 as *const MmCommunicator;

        let context = create_test_context(7, communicator_ptr);

        assert_eq!(context.updatable_buffer_id, 7);
        assert_eq!(context.communicator, communicator_ptr);
        assert!(!context.has_pending_update.load(Ordering::Acquire));
        assert!(context.pending_buffer.load(Ordering::Acquire).is_null());
    }

    #[test]
    fn test_multiple_pending_buffer_updates() {
        let context = create_test_context(1, core::ptr::null());

        // Set the first pending buffer
        let buffer1 = CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0xAA; 1024]))), 1);
        let ptr1 = Box::into_raw(Box::new(buffer1));
        context.pending_buffer.store(ptr1, Ordering::Release);
        context.has_pending_update.store(true, Ordering::Release);

        let mut comm_buffers = vec![];

        // Apply first update
        assert!(apply_pending_buffer_update(&context, &mut comm_buffers));
        assert_eq!(comm_buffers.len(), 1);

        // Set the second pending buffer
        let buffer2 = CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0xBB; 2048]))), 1);
        let ptr2 = Box::into_raw(Box::new(buffer2));
        context.pending_buffer.store(ptr2, Ordering::Release);
        context.has_pending_update.store(true, Ordering::Release);

        // Apply second update - should disable the first buffer and add the second buffer
        assert!(apply_pending_buffer_update(&context, &mut comm_buffers));
        assert_eq!(comm_buffers.len(), 2);

        // The first buffer should be disabled (the old buffer was disabled in-place)
        assert_eq!(comm_buffers[0].id(), 1);
        assert!(!comm_buffers[0].is_enabled());
        assert_eq!(comm_buffers[0].len(), 1024);

        // The second buffer should be enabled (new buffer was pushed)
        assert_eq!(comm_buffers[1].id(), 1);
        assert!(comm_buffers[1].is_enabled());
        assert_eq!(comm_buffers[1].len(), 2048);
    }

    #[test]
    fn test_pending_buffer_atomic_operations() {
        let context = create_test_context(10, core::ptr::null());

        // Verify the initial state
        assert!(!context.has_pending_update.load(Ordering::Acquire));
        assert!(context.pending_buffer.load(Ordering::Acquire).is_null());

        // Test atomic flag operations
        context.has_pending_update.store(true, Ordering::Release);
        assert!(context.has_pending_update.load(Ordering::Acquire));

        context.has_pending_update.store(false, Ordering::Release);
        assert!(!context.has_pending_update.load(Ordering::Acquire));

        // Test atomic pointer operations
        let buffer = CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 512]))), 10);
        let buffer_ptr = Box::into_raw(Box::new(buffer));

        context.pending_buffer.store(buffer_ptr, Ordering::Release);
        assert_eq!(context.pending_buffer.load(Ordering::Acquire), buffer_ptr);

        // Swap with null
        let old_ptr = context.pending_buffer.swap(core::ptr::null_mut(), Ordering::Acquire);
        assert_eq!(old_ptr, buffer_ptr);
        assert!(context.pending_buffer.load(Ordering::Acquire).is_null());

        // SAFETY: buffer_ptr was created via Box::into_raw. The box is reconstructed here to drop it.
        unsafe {
            drop(Box::from_raw(buffer_ptr));
        }
    }
}
