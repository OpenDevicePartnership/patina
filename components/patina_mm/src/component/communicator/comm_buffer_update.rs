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

use crate::config::CommunicateBuffer;
use patina::{
    UEFI_PAGE_SIZE,
    component::service::{
        Service,
        uefi_services::protocol::{Handle, ProtocolServices, ProtocolServicesExt, Tpl},
    },
    management_mode::protocol::mm_comm_buffer_update::{self, MmCommBufferUpdateProtocol},
};

use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use alloc::boxed::Box;

/// Context for the MM Comm Buffer Update Protocol notify callback
///
/// This context is shared between the protocol callback and the `communicate()` method.
/// When a protocol callback triggers, it stores the pending buffer update atomically.
/// The next `communicate()` call will apply the pending update.
pub(super) struct ProtocolNotifyContext {
    pub(super) updatable_buffer_id: u8,
    /// Pending buffer update - set by protocol callback, consumed by `communicate()`
    pub(super) pending_buffer: AtomicPtr<CommunicateBuffer>,
    /// Flag indicating if a buffer update is pending
    pub(super) has_pending_update: AtomicBool,
}

/// Register a protocol installation notification for MM Communication Buffer Updates
///
/// This function registers a callback that runs when the MM Communication Buffer Update Protocol
/// is installed, whether it is already present or is installed later.
///
/// # Parameters
/// - `protocols`: Protocol services used to register the installation notification
/// - `updatable_buffer_id`: The buffer ID that should be updated when protocol is installed
///
/// # Returns
/// - `Ok(&'static ProtocolNotifyContext)`: Context that should be stored for later use
/// - `Err(patina::error::Error)`: If the installation notification could not be registered
///
/// # Safety
/// - The returned context is leaked and will live for a static lifetime
pub(super) fn register_buffer_update_notify(
    protocols: Service<dyn ProtocolServices>,
    updatable_buffer_id: u8,
) -> patina::error::Result<&'static ProtocolNotifyContext> {
    log::trace!(target: "mm_comm", "Setting up protocol notify callback for buffer ID {updatable_buffer_id}");

    // Bind as a shared reference (Box::leak returns `&'static mut`) so it is `Copy` and can be
    // captured by the FnMut notification callback, which may run more than once.
    let context: &'static ProtocolNotifyContext = Box::leak(Box::new(ProtocolNotifyContext {
        updatable_buffer_id,
        pending_buffer: AtomicPtr::new(core::ptr::null_mut()),
        has_pending_update: AtomicBool::new(false),
    }));

    log::trace!(target: "mm_comm", "Registering protocol notify - callback runs for present and future installs");
    protocols.on_protocol_installed::<MmCommBufferUpdateProtocol>(Tpl::Callback, move |handle| {
        handle_protocol_installed(protocols, handle, context);
    })?;

    log::debug!(
        target: "mm_comm",
        "Registered protocol notify on {} with updatable_buffer_id={}",
        mm_comm_buffer_update::PROTOCOL_GUID,
        updatable_buffer_id
    );

    Ok(context)
}

/// Apply any pending buffer update if available
///
/// This function checks if a pending buffer update is available (set by the protocol callback)
/// and applies it if needed. It should be called from `communicate()` before processing
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
    log::info!(target: "mm_comm", "Successfully applied pending comm buffer {updatable_buffer_id} update");

    // Clear the pending flag
    context.has_pending_update.store(false, Ordering::Release);
    true
}

/// Handles a single installation of the MM Communication Buffer Update Protocol
///
/// Reads the updated communication buffer information from the protocol, validates it, and
/// stores it as a pending update. The update will be applied by `communicate()`.
///
/// ## Coverage
///
/// This is difficult to unit test end-to-end because it requires an active protocol installation
/// notification from the DXE Core. Elements of the protocol update process (buffer validation,
/// pending-update application) are unit tested independently. This function as a whole is
/// exercised through integration testing.
#[cfg_attr(coverage, coverage(off))]
fn handle_protocol_installed(
    protocols: Service<dyn ProtocolServices>,
    handle: Handle,
    context: &'static ProtocolNotifyContext,
) {
    log::info!(target: "mm_comm", "Protocol notify callback triggered for {}", mm_comm_buffer_update::PROTOCOL_GUID);

    let updatable_buffer_id = context.updatable_buffer_id;
    log::debug!(target: "mm_comm", "Updatable buffer ID: {updatable_buffer_id}");

    let fields = protocols.with_protocol_on::<MmCommBufferUpdateProtocol, _>(handle, |protocol| {
        // Note: Copying packed fields to local variables to avoid unaligned references
        (
            protocol.version,
            protocol.updated_comm_buffer.physical_start,
            protocol.updated_comm_buffer.number_of_pages,
            protocol.updated_comm_buffer.status,
        )
    });

    let (version, physical_start, size_pages, status_address) = match fields {
        Ok(fields) => fields,
        Err(err) => {
            log::error!(
                target: "mm_comm",
                "Failed to read MM comm buffer update protocol on handle {handle:?}: {err:?}"
            );
            return;
        }
    };

    let size_bytes = size_pages * UEFI_PAGE_SIZE as u64;

    log::info!(
        target: "mm_comm",
        "Received MM comm buffer update: version={version}, addr=0x{physical_start:X}, size={size_pages} pages (0x{size_bytes:X} bytes), status=0x{status_address:X}"
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
            log::error!(target: "mm_comm", "Failed to validate comm buffer from protocol data: {err:?}");
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
    use crate::config::CommunicateBuffer;

    use core::{
        pin::Pin,
        sync::atomic::{AtomicBool, AtomicPtr, Ordering},
    };

    use alloc::boxed::Box;

    /// Helper to create a test protocol notify context
    fn create_test_context(updatable_buffer_id: u8) -> Box<ProtocolNotifyContext> {
        Box::new(ProtocolNotifyContext {
            updatable_buffer_id,
            pending_buffer: AtomicPtr::new(core::ptr::null_mut()),
            has_pending_update: AtomicBool::new(false),
        })
    }

    #[test]
    fn test_apply_pending_buffer_update_no_pending_update() {
        let context = create_test_context(0);
        let mut comm_buffers = vec![];

        // No pending update should return false
        let result = apply_pending_buffer_update(&context, &mut comm_buffers);
        assert!(!result);
    }

    #[test]
    fn test_apply_pending_buffer_update_with_pending_buffer() {
        let context = create_test_context(5);

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
        let context = create_test_context(3);

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
        let context = create_test_context(0);

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
        let context = create_test_context(7);

        assert_eq!(context.updatable_buffer_id, 7);
        assert!(!context.has_pending_update.load(Ordering::Acquire));
        assert!(context.pending_buffer.load(Ordering::Acquire).is_null());
    }

    #[test]
    fn test_multiple_pending_buffer_updates() {
        let context = create_test_context(1);

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
        let context = create_test_context(10);

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
