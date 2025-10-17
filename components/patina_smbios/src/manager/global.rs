//! Global state management for SMBIOS manager
//!
//! This module contains global state that is excluded from coverage as it's
//! FFI/integration code that's tested implicitly through end-to-end tests.

#![coverage(off)]

use core::cell::UnsafeCell;

use patina::boot_services::StandardBootServices;
use patina::tpl_mutex::TplMutex;
use r_efi::efi;

use crate::error::SmbiosError;
use crate::service::SmbiosTableHeader;

use super::core::SmbiosManager;

/// Global storage for boot_services reference
///
/// This reference is stored during protocol installation and remains valid for
/// the lifetime of the system. Required for TplMutex construction.
///
/// # Safety
///
/// - Initialized once during `install_smbios_protocol`
/// - The boot_services reference must have 'static lifetime
/// - Access is thread-safe due to UEFI's single-threaded DXE model
pub(super) struct GlobalBootServices {
    boot_services: UnsafeCell<Option<&'static StandardBootServices>>,
}

unsafe impl Sync for GlobalBootServices {}

impl GlobalBootServices {
    const fn new() -> Self {
        Self { boot_services: UnsafeCell::new(None) }
    }

    /// Initialize with a boot_services reference
    ///
    /// # Safety
    ///
    /// Must be called exactly once during system initialization.
    /// The boot_services reference must have 'static lifetime.
    pub(super) unsafe fn initialize(&self, boot_services: &'static StandardBootServices) {
        unsafe { *self.boot_services.get() = Some(boot_services) };
    }

    /// Get the stored boot_services reference
    ///
    /// # Safety
    ///
    /// Returns None if not initialized
    #[allow(dead_code)] // Reserved for future diagnostic access to raw boot services
    unsafe fn get(&self) -> Option<&'static StandardBootServices> {
        unsafe { *self.boot_services.get() }
    }
}

pub(super) static BOOT_SERVICES: GlobalBootServices = GlobalBootServices::new();

/// Global SMBIOS manager with TplMutex protection
///
/// Provides a global reference to the SmbiosManager wrapped in TplMutex for
/// thread-safe access. This is required for C/EDKII protocol compatibility,
/// as the protocol functions cannot receive a Rust `self` parameter.
///
/// Uses TplMutex for TPL-aware synchronization. When locked, TPL is raised to NOTIFY
/// level, preventing timer interrupt reentrancy. TPL is automatically restored when
/// the lock guard is dropped.
///
/// # Safety
///
/// - Stores a 'static reference to a TplMutex (created and leaked in component.rs)
/// - Access is protected by TplMutex.lock() which raises TPL to NOTIFY
/// - The manager is initialized once during protocol installation
/// - The reference remains valid for the lifetime of the system
pub(super) struct GlobalSmbiosManager {
    manager: UnsafeCell<Option<&'static TplMutex<'static, SmbiosManager, StandardBootServices>>>,
}

unsafe impl Sync for GlobalSmbiosManager {}

impl GlobalSmbiosManager {
    const fn new() -> Self {
        Self { manager: UnsafeCell::new(None) }
    }

    /// Initialize the global manager with a 'static reference to a TplMutex
    ///
    /// # Safety
    ///
    /// Caller must ensure this is called only once during system initialization
    pub(super) unsafe fn initialize(
        &self,
        tpl_mutex: &'static TplMutex<'static, SmbiosManager, StandardBootServices>,
    ) -> Result<(), SmbiosError> {
        let ptr = self.manager.get();
        if unsafe { (*ptr).is_some() } {
            return Err(SmbiosError::AlreadyInitialized);
        }
        unsafe { *ptr = Some(tpl_mutex) };
        Ok(())
    }

    /// Get a reference to the TplMutex (returns None if not initialized)
    ///
    /// # Safety
    ///
    /// Returns a raw reference to the TplMutex. Caller must call .lock()
    /// to get TPL-protected access to the manager.
    pub(super) unsafe fn get(&self) -> Option<&'static TplMutex<'static, SmbiosManager, StandardBootServices>> {
        unsafe { *self.manager.get() }
    }

    /// Clear the manager (for cleanup on error)
    ///
    /// # Safety
    ///
    /// Caller must ensure this is only called during error cleanup
    pub(super) unsafe fn clear(&self) {
        unsafe { *self.manager.get() = None };
    }
}

pub(super) static SMBIOS_MANAGER: GlobalSmbiosManager = GlobalSmbiosManager::new();

/// Storage for the protocol interface pointer (for lifetime management)
pub(super) struct GlobalProtocolInterface {
    interface: UnsafeCell<*mut ()>,
}

unsafe impl Sync for GlobalProtocolInterface {}

impl GlobalProtocolInterface {
    const fn new() -> Self {
        Self { interface: UnsafeCell::new(core::ptr::null_mut()) }
    }

    pub(super) unsafe fn set(&self, ptr: *mut ()) {
        unsafe { *self.interface.get() = ptr };
    }

    #[allow(dead_code)]
    unsafe fn get(&self) -> *mut () {
        unsafe { *self.interface.get() }
    }

    pub(super) unsafe fn clear(&self) {
        unsafe { *self.interface.get() = core::ptr::null_mut() };
    }
}

pub(super) static SMBIOS_PROTOCOL_INTERFACE: GlobalProtocolInterface = GlobalProtocolInterface::new();

/// Storage for the protocol handle
pub(super) struct GlobalProtocolHandle {
    handle: UnsafeCell<efi::Handle>,
}

unsafe impl Sync for GlobalProtocolHandle {}

impl GlobalProtocolHandle {
    const fn new() -> Self {
        Self { handle: UnsafeCell::new(core::ptr::null_mut()) }
    }

    pub(super) unsafe fn set(&self, h: efi::Handle) {
        unsafe { *self.handle.get() = h };
    }

    #[allow(dead_code)]
    unsafe fn get(&self) -> efi::Handle {
        unsafe { *self.handle.get() }
    }
}

pub(super) static SMBIOS_PROTOCOL_HANDLE: GlobalProtocolHandle = GlobalProtocolHandle::new();

/// Wrapper for static SMBIOS header buffer that implements Sync
pub(super) struct StaticHeaderBuffer(core::cell::UnsafeCell<SmbiosTableHeader>);

unsafe impl Sync for StaticHeaderBuffer {}

impl StaticHeaderBuffer {
    const fn new(header: SmbiosTableHeader) -> Self {
        Self(core::cell::UnsafeCell::new(header))
    }

    pub(super) unsafe fn get(&self) -> *mut SmbiosTableHeader {
        self.0.get()
    }
}

/// Static storage for header returned by C protocol's get_next_ext
///
/// This avoids heap allocation issues. The header is stored in a static location
/// that persists for the lifetime of the program. Since SMBIOS headers are small
/// (4 bytes) and the C protocol is typically called sequentially, a single static
/// buffer is sufficient. The caller receives a pointer to this buffer which remains
/// valid until the next call to get_next_ext.
pub(super) static SMBIOS_HEADER_BUFFER: StaticHeaderBuffer =
    StaticHeaderBuffer::new(SmbiosTableHeader { record_type: 0, length: 0, handle: 0 });
