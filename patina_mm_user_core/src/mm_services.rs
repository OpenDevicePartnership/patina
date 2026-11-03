//! MM System Table (MMST) Construction - User Core Implementation
//!
//! This module builds the concrete `EfiMmSystemTable` handed to dispatched MM
//! drivers. The table itself contains **no logic**: every function pointer is a
//! thin `extern "efiapi"` thunk that locates the singleton
//! [`MmUserCore`] instance and forwards to the safe Rust implementation living
//! in its databases ([`ProtocolDatabase`], [`MmiDatabase`],
//! [`MmConfigurationTableDb`]). In other words the table is the *interface* and
//! the user core instance is the *implementation* - not the other way around.
//!
//! The *type definitions* (`EfiMmSystemTable`, `MmServices`,
//! `StandardMmServices`, …) live in the Patina SDK at [`patina::management_mode::mm_services`].
//!
//! [`ProtocolDatabase`]: crate::protocol_db::ProtocolDatabase
//! [`MmiDatabase`]: crate::mmi::MmiDatabase
//! [`MmConfigurationTableDb`]: crate::config_table::MmConfigurationTableDb
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;

use alloc::vec::Vec;
use core::{ffi::c_void, ptr::NonNull};

use patina::standard::efi;

use crate::{MmUserCore, pool_allocator::PageAllocatorBackend};
use patina::{
    management_mode::mm_services::{MmServices, ProtocolNotify, Registration, StandardMmServices},
    pi::mm_cis::{
        EfiMmSystemTable, MM_MMST_SIGNATURE, MM_SYSTEM_TABLE_REVISION, MmCpuIoAccess, MmCpuIoProtocol, MmNotifyFn,
        MmiHandlerEntryPoint,
    },
};

/// The bridge the `EfiMmSystemTable` thunks dispatch through.
///
/// Initialized once in [`init_mm_services`] with the user core's [`MmServices`]
/// provider ([`MmUserCore`]). Every thunk forwards into this bridge, which in
/// turn calls the provider - so no Rust ever calls the table's function pointers.
static MM_SERVICES: StandardMmServices = StandardMmServices::new_uninit();

/// Register the [`MmServices`] provider the `EfiMmSystemTable` thunks forward to.
///
/// Must be called once during startup, before any driver can invoke the table.
pub(crate) fn init_mm_services(provider: &'static dyn MmServices) {
    MM_SERVICES.init(provider);
}

/// Records a failure that a thunk itself detected, before any service ran.
///
/// Only the thunk's own argument checks report through here, and they only do so on a
/// failure. A status produced deeper in the core is passed back untouched, because the
/// layer that detected it already logged the reason and named the service.
///
/// `NOT_FOUND`, `UNSUPPORTED` and `BUFFER_TOO_SMALL` are how drivers probe for
/// optional protocols and size their buffers, so they are recorded at debug
/// level. Every other status points at a caller defect and is recorded at error
/// level so it stands out in a boot log.
fn report(service: &str, status: efi::Status) -> efi::Status {
    if status == efi::Status::SUCCESS {
        return status;
    }

    if status == efi::Status::NOT_FOUND || status == efi::Status::UNSUPPORTED || status == efi::Status::BUFFER_TOO_SMALL
    {
        log::debug!("MMST {service}: {status:?}");
    } else {
        log::error!("MMST {service} failed: {status:?}");
    }

    status
}

/// Build the MM System Table value.
///
/// Every field is a thunk that defers to [`MmUserCore::instance`]; the table
/// carries no state of its own beyond the CPU/configuration fields the core
/// updates in place. [`MmUserCore::init_mm_system_table`] boxes the returned
/// value and hands the pointer to dispatched drivers.
pub(crate) fn build_mm_system_table() -> EfiMmSystemTable {
    EfiMmSystemTable {
        hdr: efi::TableHeader {
            signature: u64::from(MM_MMST_SIGNATURE),
            revision: MM_SYSTEM_TABLE_REVISION,
            header_size: core::mem::size_of::<EfiMmSystemTable>() as u32,
            crc32: 0,
            reserved: 0,
        },
        mm_firmware_vendor: core::ptr::null_mut(),
        mm_firmware_revision: 0,

        mm_install_configuration_table: mm_install_configuration_table_impl,

        mm_io: MmCpuIoProtocol {
            mem: MmCpuIoAccess { read: mm_io_not_available, write: mm_io_not_available },
            io: MmCpuIoAccess { read: mm_io_not_available, write: mm_io_not_available },
        },

        mm_allocate_pool: mm_allocate_pool_impl,
        mm_free_pool: mm_free_pool_impl,
        mm_allocate_pages: mm_allocate_pages_impl,
        mm_free_pages: mm_free_pages_impl,

        mm_startup_this_ap: mm_startup_this_ap_not_available,

        currently_executing_cpu: 0,
        number_of_cpus: 0,
        cpu_save_state_size: core::ptr::null_mut(),
        cpu_save_state: core::ptr::null_mut(),

        number_of_table_entries: 0,
        mm_configuration_table: core::ptr::null_mut(),

        mm_install_protocol_interface: mm_install_protocol_interface_impl,
        mm_uninstall_protocol_interface: mm_uninstall_protocol_interface_impl,
        mm_handle_protocol: mm_handle_protocol_impl,
        mm_register_protocol_notify: mm_register_protocol_notify_impl,
        mm_locate_handle: mm_locate_handle_impl,
        mm_locate_protocol: mm_locate_protocol_impl,

        mmi_manage: mmi_manage_impl,
        mmi_handler_register: mmi_handler_register_impl,
        mmi_handler_unregister: mmi_handler_unregister_impl,
    }
}

/// CPU I/O access stub that always reports the service as unavailable.
///
/// # Safety
///
/// Part of the C ABI surface invoked through the system table. It dereferences
/// none of its pointer arguments, so it performs no memory accesses; it remains
/// `unsafe` only to match the `MmCpuIoFn` signature.
unsafe extern "efiapi" fn mm_io_not_available(
    _this: *const MmCpuIoAccess,
    _width: usize,
    _address: u64,
    _count: usize,
    _buffer: *mut c_void,
) -> efi::Status {
    report("MmCpuIo", efi::Status::UNSUPPORTED)
}

/// Installs, updates, or removes a configuration table entry.
///
/// # Safety
///
/// Invoked through the system table by (potentially untrusted) C callers. The
/// `guid` pointer is null-checked before being dereferenced, but a non-null
/// pointer is dereferenced on trust because its validity cannot be verified
/// here - this is intrinsically unsafe as it handles inputs from C code.
unsafe extern "efiapi" fn mm_install_configuration_table_impl(
    _system_table: *const EfiMmSystemTable,
    guid: *const efi::Guid,
    table: *mut c_void,
    table_size: usize,
) -> efi::Status {
    if guid.is_null() {
        return report("MmInstallConfigurationTable", efi::Status::INVALID_PARAMETER);
    }

    // SAFETY: `guid` was null-checked above; the C caller guarantees a non-null pointer references a
    // valid `efi::Guid`. Dereferenced once into a reference; all further use is safe Rust.
    let guid = unsafe { &*guid };
    // SAFETY: `table` is forwarded as provided by the C caller; the `MmServices` contract makes the
    // caller responsible for its validity and lifetime.
    match unsafe { MM_SERVICES.install_configuration_table(guid, table, table_size) } {
        Ok(()) => efi::Status::SUCCESS,
        Err(status) => status,
    }
}

extern "efiapi" fn mm_allocate_pool_impl(
    pool_type: efi::MemoryType,
    size: usize,
    buffer: *mut *mut c_void,
) -> efi::Status {
    if buffer.is_null() {
        return report("MmAllocatePool", efi::Status::INVALID_PARAMETER);
    }

    match MM_SERVICES.allocate_pool(pool_type, size) {
        Ok(ptr) => {
            // SAFETY: `buffer` was null-checked above; the C caller guarantees it references a writable
            // `*mut c_void` out-parameter. Written exactly once.
            unsafe { *buffer = ptr.cast::<c_void>() };
            efi::Status::SUCCESS
        }
        Err(status) => status,
    }
}

extern "efiapi" fn mm_free_pool_impl(buffer: *mut c_void) -> efi::Status {
    match MM_SERVICES.free_pool(buffer.cast::<u8>()) {
        Ok(()) => efi::Status::SUCCESS,
        Err(status) => status,
    }
}

extern "efiapi" fn mm_allocate_pages_impl(
    alloc_type: efi::AllocateType,
    memory_type: efi::MemoryType,
    pages: usize,
    memory: *mut efi::PhysicalAddress,
) -> efi::Status {
    if memory.is_null() {
        return report("MmAllocatePages", efi::Status::INVALID_PARAMETER);
    }

    match MM_SERVICES.allocate_pages(alloc_type, memory_type, pages) {
        Ok(addr) => {
            // SAFETY: `memory` was null-checked above; the C caller guarantees it references a
            // writable `efi::PhysicalAddress` out-parameter. Written exactly once.
            unsafe { *memory = addr };
            efi::Status::SUCCESS
        }
        Err(status) => status,
    }
}

extern "efiapi" fn mm_free_pages_impl(memory: efi::PhysicalAddress, pages: usize) -> efi::Status {
    match MM_SERVICES.free_pages(memory, pages) {
        Ok(()) => efi::Status::SUCCESS,
        Err(status) => status,
    }
}

/// Starts a procedure on an application processor.
///
/// # Safety
///
/// Part of the C ABI surface invoked through the system table. It dereferences
/// none of its arguments and is a not-available stub; it remains `unsafe` only
/// to match the `MmStartupThisApFn` signature.
unsafe extern "efiapi" fn mm_startup_this_ap_not_available(
    _procedure: usize,
    _cpu_number: usize,
    _proc_arguments: *mut c_void,
) -> efi::Status {
    report("MmStartupThisAp", efi::Status::UNSUPPORTED)
}

extern "efiapi" fn mm_install_protocol_interface_impl(
    handle: *mut efi::Handle,
    protocol: *mut efi::Guid,
    _interface_type: efi::InterfaceType,
    interface: *mut c_void,
) -> efi::Status {
    if handle.is_null() || protocol.is_null() {
        return report("MmInstallProtocolInterface", efi::Status::INVALID_PARAMETER);
    }

    // SAFETY: `protocol` was null-checked above; the C caller guarantees it references a valid
    // `efi::Guid`. Dereferenced once into a reference.
    let guid = unsafe { &*protocol };
    // SAFETY: `handle` was null-checked above; the C caller guarantees a readable in/out pointer.
    let caller_handle = unsafe { *handle };
    let in_handle = (!caller_handle.is_null()).then_some(caller_handle);

    // SAFETY: `interface` is forwarded unchanged; the C caller owns its validity and lifetime.
    match unsafe { MM_SERVICES.install_protocol_interface(in_handle, guid, interface) } {
        Ok(new_handle) => {
            // SAFETY: `handle` was null-checked above; it is a writable out-parameter. Written once.
            unsafe { *handle = new_handle };
            efi::Status::SUCCESS
        }
        Err(status) => status,
    }
}

extern "efiapi" fn mm_uninstall_protocol_interface_impl(
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    interface: *mut c_void,
) -> efi::Status {
    if handle.is_null() || protocol.is_null() {
        return report("MmUninstallProtocolInterface", efi::Status::INVALID_PARAMETER);
    }

    // SAFETY: `protocol` was null-checked above; the C caller guarantees it references a valid
    // `efi::Guid`. Dereferenced once into a reference.
    let guid = unsafe { &*protocol };

    // SAFETY: `interface` is forwarded as the pointer the caller previously installed.
    match unsafe { MM_SERVICES.uninstall_protocol_interface(handle, guid, interface) } {
        Ok(()) => efi::Status::SUCCESS,
        Err(status) => status,
    }
}

extern "efiapi" fn mm_handle_protocol_impl(
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    interface: *mut *mut c_void,
) -> efi::Status {
    if protocol.is_null() {
        return report("MmHandleProtocol", efi::Status::INVALID_PARAMETER);
    }

    if interface.is_null() {
        return report("MmHandleProtocol", efi::Status::INVALID_PARAMETER);
    }
    // C reference: *Interface = NULL before lookup.
    // SAFETY: `interface` was null-checked above; the C caller guarantees it references a writable
    // `*mut c_void` out-parameter. Written once.
    unsafe { *interface = core::ptr::null_mut() };

    if handle.is_null() {
        return report("MmHandleProtocol", efi::Status::INVALID_PARAMETER);
    }

    // SAFETY: `protocol` was null-checked above; the C caller guarantees it references a valid
    // `efi::Guid`. Dereferenced once into a reference.
    let guid = unsafe { &*protocol };

    // SAFETY: `handle` was null-checked above.
    match unsafe { MM_SERVICES.handle_protocol(handle, guid) } {
        Ok(i_protocol) => {
            // SAFETY: `interface` was null-checked above and is a writable out-parameter.
            unsafe { *interface = i_protocol };
            efi::Status::SUCCESS
        }
        Err(status) => status,
    }
}

extern "efiapi" fn mm_register_protocol_notify_impl(
    protocol: *const efi::Guid,
    function: usize,
    registration: *mut *mut c_void,
) -> efi::Status {
    if protocol.is_null() || registration.is_null() {
        return report("MmRegisterProtocolNotify", efi::Status::INVALID_PARAMETER);
    }

    // SAFETY: `protocol` was null-checked above; the C caller guarantees it references a valid
    // `efi::Guid`. Dereferenced once into a reference.
    let guid = unsafe { &*protocol };

    if function == 0 {
        // Function is NULL → unregister the notification identified by *Registration.
        // SAFETY: `registration` was null-checked above; the C caller guarantees it references a
        // readable `*mut c_void`. Read once.
        let reg = unsafe { *registration };
        match NonNull::new(reg).map(Registration::new) {
            Some(reg_token) => match MM_SERVICES.unregister_protocol_notify(guid, reg_token) {
                Ok(()) => efi::Status::SUCCESS,
                Err(status) => status,
            },
            None => report("MmRegisterProtocolNotify", efi::Status::INVALID_PARAMETER),
        }
    } else {
        // Register a new notification.
        // SAFETY: `function` is a non-null `EFI_MM_NOTIFY_FN` function pointer passed as a usize by
        // the C caller; transmuting it back to the matching ABI function pointer type is sound.
        let notify_fn: MmNotifyFn = unsafe { core::mem::transmute(function) };
        match MM_SERVICES.register_protocol_notify(guid, ProtocolNotify::efi(notify_fn)) {
            Ok(token) => {
                // SAFETY: `registration` was null-checked above and is a writable out-parameter.
                unsafe { *registration = token.as_ptr() };
                efi::Status::SUCCESS
            }
            Err(status) => status,
        }
    }
}

extern "efiapi" fn mm_locate_handle_impl(
    search_type: efi::LocateSearchType,
    protocol: *mut efi::Guid,
    _search_key: *mut c_void,
    buffer_size: *mut usize,
    buffer: *mut efi::Handle,
) -> efi::Status {
    if buffer_size.is_null() {
        return report("MmLocateHandle", efi::Status::INVALID_PARAMETER);
    }

    // SAFETY: `protocol`, when non-null, is guaranteed by the C caller to reference a valid `efi::Guid`.
    let protocol = if protocol.is_null() { None } else { Some(unsafe { &*protocol }) };

    let handles = match MM_SERVICES.locate_handle(search_type, protocol) {
        Ok(handles) => handles,
        Err(status) => return status,
    };

    if handles.is_empty() {
        return report("MmLocateHandle", efi::Status::NOT_FOUND);
    }

    let required_size = handles.len() * core::mem::size_of::<efi::Handle>();
    // SAFETY: `buffer_size` was null-checked at the top of the function; the C caller guarantees it
    // references a readable/writable `usize`. Read once, then written once.
    let caller_size = unsafe { *buffer_size };
    // SAFETY: see above - `buffer_size` is a valid writable out-parameter.
    unsafe { *buffer_size = required_size };

    if caller_size < required_size {
        return report("MmLocateHandle", efi::Status::BUFFER_TOO_SMALL);
    }

    if buffer.is_null() {
        return report("MmLocateHandle", efi::Status::INVALID_PARAMETER);
    }

    // SAFETY: `buffer` was null-checked above and `caller_size >= required_size`, so the destination
    // holds at least `handles.len()` `efi::Handle` entries. Source and destination do not overlap.
    unsafe {
        core::ptr::copy_nonoverlapping(handles.as_ptr(), buffer, handles.len());
    }
    efi::Status::SUCCESS
}

extern "efiapi" fn mm_locate_protocol_impl(
    protocol: *mut efi::Guid,
    _registration: *mut c_void,
    interface: *mut *mut c_void,
) -> efi::Status {
    if protocol.is_null() || interface.is_null() {
        return report("MmLocateProtocol", efi::Status::INVALID_PARAMETER);
    }

    // SAFETY: `protocol` was null-checked above; the C caller guarantees it references a valid
    // `efi::Guid`. Dereferenced once into a reference.
    let guid = unsafe { &*protocol };

    // SAFETY: locating a protocol performs only database lookups.
    match unsafe { MM_SERVICES.locate_protocol(guid) } {
        Ok(i_protocol) => {
            // SAFETY: `interface` was null-checked above and is a writable out-parameter.
            unsafe { *interface = i_protocol };
            efi::Status::SUCCESS
        }
        Err(status) => status,
    }
}

/// Dispatches an MMI of a particular type to the registered handlers.
///
/// # Safety
///
/// Invoked through the system table by (potentially untrusted) C callers. The
/// `handler_type` pointer is null-checked before being dereferenced, but a
/// non-null pointer is dereferenced on trust; `context`/`comm_buffer` are passed
/// through to the handlers. This is intrinsically unsafe as it handles inputs
/// from C code.
unsafe extern "efiapi" fn mmi_manage_impl(
    handler_type: *const efi::Guid,
    context: *const c_void,
    comm_buffer: *mut c_void,
    comm_buffer_size: *mut usize,
) -> efi::Status {
    // SAFETY: `handler_type` is null-checked here; when non-null the C caller guarantees it
    // references a valid `efi::Guid`, dereferenced once into a reference.
    let guid = if handler_type.is_null() { None } else { Some(unsafe { &*handler_type }) };

    // SAFETY: `context`/`comm_buffer`/`comm_buffer_size` are forwarded unchanged to the handlers.
    // The MMI database logs when no handler matches.
    unsafe { MM_SERVICES.mmi_manage(guid, context, comm_buffer, comm_buffer_size) }
}

/// Registers an MMI handler entry point for a particular handler type.
///
/// # Safety
///
/// Invoked through the system table by (potentially untrusted) C callers. A null `handler` and
/// the `dispatch_handle` and `handler_type` pointers are checked before use, but non-null
/// pointers are dereferenced on trust. This is intrinsically unsafe as it handles inputs from C
/// code.
unsafe extern "efiapi" fn mmi_handler_register_impl(
    handler: Option<MmiHandlerEntryPoint>,
    handler_type: *const efi::Guid,
    dispatch_handle: *mut efi::Handle,
) -> efi::Status {
    let Some(handler) = handler else {
        return report("MmiHandlerRegister", efi::Status::INVALID_PARAMETER);
    };
    if dispatch_handle.is_null() {
        return report("MmiHandlerRegister", efi::Status::INVALID_PARAMETER);
    }

    // SAFETY: `handler_type` is null-checked here; when non-null the C caller guarantees it
    // references a valid `efi::Guid`, dereferenced once into a reference.
    let guid = if handler_type.is_null() { None } else { Some(unsafe { &*handler_type }) };

    match MM_SERVICES.mmi_handler_register(handler, guid) {
        Ok(handle) => {
            // SAFETY: `dispatch_handle` was null-checked above and is a writable out-parameter.
            unsafe { *dispatch_handle = handle };
            efi::Status::SUCCESS
        }
        Err(status) => status,
    }
}

/// Unregisters a previously registered MMI handler.
///
/// # Safety
///
/// Part of the C ABI surface invoked through the system table. It dereferences
/// none of its arguments; it remains `unsafe` only to match the
/// `MmiHandlerUnregisterFn` signature.
unsafe extern "efiapi" fn mmi_handler_unregister_impl(dispatch_handle: efi::Handle) -> efi::Status {
    match MM_SERVICES.mmi_handler_unregister(dispatch_handle) {
        Ok(()) => efi::Status::SUCCESS,
        Err(status) => status,
    }
}

/// Free a pool allocation previously produced by [`MmServices::allocate_pool`].
///
/// Kept as a private helper so the public `free_pool` method stays a safe wrapper
/// around the single raw-pointer deallocation.
fn dealloc_pool(buffer: *mut u8) {
    // SAFETY: `Layout::from_size_align_unchecked` is sound for size/alignment of 1. The MM free ABI
    // provides no size, so the original layout cannot be reconstructed; `buffer` is trusted to be a
    // pointer previously returned by `allocate_pool` and is freed exactly once.
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(1, 1);
        alloc::alloc::dealloc(buffer, layout);
    }
}

/// Native [`MmServices`] implementation, backed directly by the user core's databases.
///
/// This is *the* MM services implementation. Both the `EfiMmSystemTable` thunks
/// above and the core's own Rust code drive MM services through this trait, so
/// neither path calls back out through the C function-pointer table.
impl MmServices for MmUserCore {
    /// Allocate pool memory.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmAllocatePool`
    fn allocate_pool(&self, _pool_type: efi::MemoryType, size: usize) -> Result<*mut u8, efi::Status> {
        if size == 0 {
            log::error!("MmAllocatePool: size is 0");
            return Err(efi::Status::INVALID_PARAMETER);
        }
        let layout = core::alloc::Layout::from_size_align(size, 8).map_err(|_| {
            log::error!("MmAllocatePool: size {size} does not form a valid layout");
            efi::Status::INVALID_PARAMETER
        })?;
        // SAFETY: `layout` has a non-zero size (checked above), satisfying the `GlobalAlloc::alloc` contract.
        let ptr = unsafe { alloc::alloc::alloc(layout) };
        if ptr.is_null() {
            log::error!("MmAllocatePool: the heap could not satisfy {size} bytes");
            Err(efi::Status::OUT_OF_RESOURCES)
        } else {
            Ok(ptr)
        }
    }

    /// Free pool memory.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmFreePool`
    fn free_pool(&self, buffer: *mut u8) -> Result<(), efi::Status> {
        if buffer.is_null() {
            log::error!("MmFreePool: buffer pointer is null");
            return Err(efi::Status::INVALID_PARAMETER);
        }
        dealloc_pool(buffer);
        Ok(())
    }

    /// Allocate pages.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmAllocatePages`
    fn allocate_pages(
        &self,
        _alloc_type: efi::AllocateType,
        _memory_type: efi::MemoryType,
        pages: usize,
    ) -> Result<u64, efi::Status> {
        if pages == 0 {
            log::error!("MmAllocatePages: page count is 0");
            return Err(efi::Status::INVALID_PARAMETER);
        }
        // The syscall allocator logs why the request failed.
        crate::mm_mem::SYSCALL_PAGE_ALLOCATOR.allocate_pages(pages).map_err(|_| efi::Status::OUT_OF_RESOURCES)
    }

    /// Free pages.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmFreePages`
    fn free_pages(&self, memory: u64, pages: usize) -> Result<(), efi::Status> {
        if memory == 0 || pages == 0 {
            log::error!("MmFreePages: rejected address 0x{memory:016x} with {pages} page(s)");
            return Err(efi::Status::INVALID_PARAMETER);
        }
        // The syscall allocator logs why the request failed.
        crate::mm_mem::SYSCALL_PAGE_ALLOCATOR.free_pages(memory, pages).map_err(|_| efi::Status::INVALID_PARAMETER)
    }

    /// Install a protocol interface on a handle.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmInstallProtocolInterface`
    ///
    /// # Safety
    ///
    /// `interface` must be a valid pointer to the protocol structure or null, and
    /// must remain valid for as long as the interface is installed.
    unsafe fn install_protocol_interface(
        &self,
        handle: Option<efi::Handle>,
        protocol: &efi::Guid,
        interface: *mut c_void,
    ) -> Result<efi::Handle, efi::Status> {
        // A `None` handle maps to a null handle, which the database treats as "allocate a new one".
        self.protocol_db.install_protocol(handle.unwrap_or(core::ptr::null_mut()), protocol, interface)
    }

    /// Uninstall a protocol interface from a handle.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmUninstallProtocolInterface`
    ///
    /// # Safety
    ///
    /// `interface` must match the pointer that was installed.
    unsafe fn uninstall_protocol_interface(
        &self,
        handle: efi::Handle,
        protocol: &efi::Guid,
        interface: *mut c_void,
    ) -> Result<(), efi::Status> {
        self.protocol_db.uninstall_protocol(handle, protocol, interface)
    }

    /// Query a handle for a protocol.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmHandleProtocol`
    ///
    /// # Safety
    ///
    /// The returned pointer must be used carefully to avoid aliasing violations.
    unsafe fn handle_protocol(&self, handle: efi::Handle, protocol: &efi::Guid) -> Result<*mut c_void, efi::Status> {
        self.protocol_db.handle_protocol(handle, protocol).ok_or_else(|| {
            log::debug!("MmHandleProtocol: {protocol:?} is not installed on handle {handle:p}");
            efi::Status::UNSUPPORTED
        })
    }

    /// Locate the first device that supports a protocol.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmLocateProtocol`
    ///
    /// # Safety
    ///
    /// The returned pointer must be used carefully to avoid aliasing violations.
    unsafe fn locate_protocol(&self, protocol: &efi::Guid) -> Result<*mut c_void, efi::Status> {
        self.protocol_db.locate_protocol(protocol).ok_or_else(|| {
            log::debug!("MmLocateProtocol: {protocol:?} is not installed on any handle");
            efi::Status::NOT_FOUND
        })
    }

    /// Manage (dispatch) an MMI.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmiManage`
    ///
    /// # Safety
    ///
    /// `context`, `comm_buffer`, and `comm_buffer_size` are all optional pointers.
    /// But they must be valid if provided.
    unsafe fn mmi_manage(
        &self,
        handler_type: Option<&efi::Guid>,
        context: *const c_void,
        comm_buffer: *mut c_void,
        comm_buffer_size: *mut usize,
    ) -> efi::Status {
        self.mmi_db.mmi_manage(handler_type, context, comm_buffer, comm_buffer_size)
    }

    /// Register an MMI handler.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmiHandlerRegister`
    fn mmi_handler_register(
        &self,
        handler: MmiHandlerEntryPoint,
        handler_type: Option<&efi::Guid>,
    ) -> Result<efi::Handle, efi::Status> {
        self.mmi_db.mmi_handler_register(handler, handler_type)
    }

    /// Unregister an MMI handler.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmiHandlerUnRegister`
    fn mmi_handler_unregister(&self, dispatch_handle: efi::Handle) -> Result<(), efi::Status> {
        self.mmi_db.mmi_handler_unregister(dispatch_handle)
    }

    /// Add, update, or remove a configuration-table entry.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmInstallConfigurationTable`
    ///
    /// # Safety
    ///
    /// `table` must remain valid for as long as the entry is installed, or be
    /// null to remove an existing entry.
    unsafe fn install_configuration_table(
        &self,
        guid: &efi::Guid,
        table: *mut c_void,
        _table_size: usize,
    ) -> Result<(), efi::Status> {
        let status = self.config_table_db.install_configuration_table(self.mm_system_table_ptr(), guid, table);
        if status == efi::Status::SUCCESS { Ok(()) } else { Err(status) }
    }

    /// Register a callback invoked when a protocol interface is installed.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmRegisterProtocolNotify` (register form)
    ///
    /// Returns a [`Registration`] token that can be passed to
    /// `unregister_protocol_notify`.
    fn register_protocol_notify(
        &self,
        protocol: &efi::Guid,
        notify: ProtocolNotify,
    ) -> Result<Registration, efi::Status> {
        // Tokens are derived from a non-zero counter, so they are never null.
        let token = self.protocol_db.register_protocol_notify(protocol, notify);
        NonNull::new(token).map(Registration::new).ok_or_else(|| {
            log::error!("MmRegisterProtocolNotify: ran out of registration tokens for {protocol:?}");
            efi::Status::OUT_OF_RESOURCES
        })
    }

    /// Unregister a previously registered protocol-install notification.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmRegisterProtocolNotify` (unregister form)
    fn unregister_protocol_notify(&self, protocol: &efi::Guid, registration: Registration) -> Result<(), efi::Status> {
        self.protocol_db.unregister_protocol_notify(protocol, registration.as_ptr())
    }

    /// Return the handles matching a search type and optional protocol.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmLocateHandle`
    fn locate_handle(
        &self,
        search_type: efi::LocateSearchType,
        protocol: Option<&efi::Guid>,
    ) -> Result<Vec<efi::Handle>, efi::Status> {
        match search_type {
            efi::ALL_HANDLES => Ok(self.protocol_db.all_handles()),
            efi::BY_PROTOCOL => {
                let guid = protocol.ok_or_else(|| {
                    log::error!("MmLocateHandle: BY_PROTOCOL search with a null protocol GUID");
                    efi::Status::INVALID_PARAMETER
                })?;
                Ok(self.protocol_db.locate_handle_by_protocol(guid))
            }
            _ => {
                log::warn!("MmLocateHandle: search type {search_type} not yet supported");
                Err(efi::Status::UNSUPPORTED)
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    static PROTOCOL_A: efi::Guid = efi::Guid::from_fields(0xA000_0001, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 1]);
    static PROTOCOL_B: efi::Guid = efi::Guid::from_fields(0xB000_0002, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 2]);
    static TABLE_GUID: efi::Guid = efi::Guid::from_fields(0xC000_0003, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 3]);
    static HANDLER_GUID: efi::Guid = efi::Guid::from_fields(0xD000_0004, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 4]);

    /// The provider the thunks forward to. One per test process, so each test starts clean.
    static CORE: MmUserCore = MmUserCore::new();
    static MMI_CALLS: AtomicUsize = AtomicUsize::new(0);
    static NOTIFY_CALLS: AtomicUsize = AtomicUsize::new(0);

    /// Interface pointers only ever need to be distinct and non-null; they are never dereferenced.
    fn interface(tag: usize) -> *mut c_void {
        core::ptr::without_provenance_mut(tag)
    }

    /// Registers the core behind the table thunks and returns it.
    fn services() -> &'static MmUserCore {
        init_mm_services(&CORE);
        &CORE
    }

    /// Makes the syscall-backed page allocator answer with `reply`.
    fn page_allocator_replies(reply: u64) {
        crate::mm_mem::mock::set_handler(move |_, _, _, _| reply);
        crate::mm_mem::SYSCALL_PAGE_ALLOCATOR.set_initialized();
    }

    unsafe extern "efiapi" fn recording_handler(
        _dispatch_handle: efi::Handle,
        _context: *const c_void,
        _comm_buffer: *mut c_void,
        _comm_buffer_size: *mut usize,
    ) -> efi::Status {
        MMI_CALLS.fetch_add(1, Ordering::Relaxed);
        efi::Status::SUCCESS
    }

    unsafe extern "efiapi" fn recording_notify(
        _protocol: *const efi::Guid,
        _interface: *mut c_void,
        _handle: efi::Handle,
    ) -> efi::Status {
        NOTIFY_CALLS.fetch_add(1, Ordering::Relaxed);
        efi::Status::SUCCESS
    }

    /// The notify callback as the `usize` the C ABI passes it in.
    fn notify_fn() -> usize {
        (recording_notify as MmNotifyFn) as usize
    }

    /// Installs a protocol through the table thunk and returns the handle it landed on.
    fn install_on(
        mut handle: efi::Handle,
        mut guid: efi::Guid,
        interface: *mut c_void,
    ) -> Result<efi::Handle, efi::Status> {
        let status =
            mm_install_protocol_interface_impl(&raw mut handle, &raw mut guid, efi::NATIVE_INTERFACE, interface);
        if status == efi::Status::SUCCESS { Ok(handle) } else { Err(status) }
    }

    fn install(guid: efi::Guid, interface: *mut c_void) -> efi::Handle {
        install_on(core::ptr::null_mut(), guid, interface).expect("install succeeds")
    }

    #[test]
    fn test_the_system_table_is_stamped_with_the_pi_signature_and_revision() {
        let table = build_mm_system_table();

        assert_eq!(table.hdr.signature, u64::from(MM_MMST_SIGNATURE));
        assert_eq!(table.hdr.revision, MM_SYSTEM_TABLE_REVISION);
        assert_eq!(table.hdr.header_size as usize, core::mem::size_of::<EfiMmSystemTable>());
        assert_eq!(table.number_of_table_entries, 0);
        assert!(table.mm_configuration_table.is_null());
        assert!(table.cpu_save_state.is_null());
    }

    #[test]
    fn test_cpu_io_and_ap_startup_report_themselves_unavailable() {
        let table = build_mm_system_table();

        // SAFETY: the stubs dereference none of their arguments.
        unsafe {
            assert_eq!(
                (table.mm_io.mem.read)(&raw const table.mm_io.mem, 0, 0, 0, core::ptr::null_mut()),
                efi::Status::UNSUPPORTED
            );
            assert_eq!(
                (table.mm_io.io.write)(&raw const table.mm_io.io, 0, 0, 0, core::ptr::null_mut()),
                efi::Status::UNSUPPORTED
            );
            assert_eq!((table.mm_startup_this_ap)(0, 0, core::ptr::null_mut()), efi::Status::UNSUPPORTED);
        }
    }

    #[test]
    fn test_allocate_pool_hands_back_writable_memory_and_free_pool_reclaims_it() {
        services();
        let mut buffer: *mut c_void = core::ptr::null_mut();

        assert_eq!(mm_allocate_pool_impl(efi::RUNTIME_SERVICES_DATA, 64, &raw mut buffer), efi::Status::SUCCESS);
        assert!(!buffer.is_null());

        // SAFETY: `allocate_pool` returned a 64-byte allocation aligned to 8.
        unsafe { core::ptr::write_bytes(buffer.cast::<u8>(), 0xAB, 64) };

        assert_eq!(mm_free_pool_impl(buffer), efi::Status::SUCCESS);
    }

    #[test]
    fn test_allocate_pool_rejects_a_zero_size_and_a_null_out_parameter() {
        services();
        let mut buffer: *mut c_void = core::ptr::null_mut();

        assert_eq!(
            mm_allocate_pool_impl(efi::RUNTIME_SERVICES_DATA, 0, &raw mut buffer),
            efi::Status::INVALID_PARAMETER
        );
        assert_eq!(
            mm_allocate_pool_impl(efi::RUNTIME_SERVICES_DATA, 64, core::ptr::null_mut()),
            efi::Status::INVALID_PARAMETER
        );
    }

    #[test]
    fn test_allocate_pool_rejects_a_size_that_cannot_form_a_layout() {
        services();
        let mut buffer: *mut c_void = core::ptr::null_mut();

        assert_eq!(
            mm_allocate_pool_impl(efi::RUNTIME_SERVICES_DATA, usize::MAX, &raw mut buffer),
            efi::Status::INVALID_PARAMETER
        );
    }

    #[test]
    fn test_allocate_pool_reports_a_request_the_heap_cannot_satisfy() {
        services();
        let mut buffer: *mut c_void = core::ptr::null_mut();

        // The largest size that still forms a valid layout, which no heap can supply.
        let too_large = (isize::MAX as usize) - 7;
        assert_eq!(
            mm_allocate_pool_impl(efi::RUNTIME_SERVICES_DATA, too_large, &raw mut buffer),
            efi::Status::OUT_OF_RESOURCES
        );
        assert!(buffer.is_null());
    }

    #[test]
    fn test_report_passes_success_through_untouched() {
        // Only failures are reported, so a success status is returned as-is.
        assert_eq!(report("MmTest", efi::Status::SUCCESS), efi::Status::SUCCESS);
    }

    #[test]
    fn test_free_pool_rejects_a_null_buffer() {
        services();

        assert_eq!(mm_free_pool_impl(core::ptr::null_mut()), efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_allocate_pages_returns_the_address_the_supervisor_supplied() {
        services();
        page_allocator_replies(0x4000);
        let mut memory: efi::PhysicalAddress = 0;

        assert_eq!(
            mm_allocate_pages_impl(efi::ALLOCATE_ANY_PAGES, efi::RUNTIME_SERVICES_DATA, 2, &raw mut memory),
            efi::Status::SUCCESS
        );
        assert_eq!(memory, 0x4000);
        assert_eq!(mm_free_pages_impl(0x4000, 2), efi::Status::SUCCESS);
    }

    #[test]
    fn test_allocate_pages_reports_out_of_resources_when_the_supervisor_returns_null() {
        services();
        page_allocator_replies(0);
        let mut memory: efi::PhysicalAddress = 0;

        assert_eq!(
            mm_allocate_pages_impl(efi::ALLOCATE_ANY_PAGES, efi::RUNTIME_SERVICES_DATA, 2, &raw mut memory),
            efi::Status::OUT_OF_RESOURCES
        );
    }

    #[test]
    fn test_page_services_reject_degenerate_requests() {
        services();
        let mut memory: efi::PhysicalAddress = 0;

        assert_eq!(
            mm_allocate_pages_impl(efi::ALLOCATE_ANY_PAGES, efi::RUNTIME_SERVICES_DATA, 0, &raw mut memory),
            efi::Status::INVALID_PARAMETER
        );
        assert_eq!(
            mm_allocate_pages_impl(efi::ALLOCATE_ANY_PAGES, efi::RUNTIME_SERVICES_DATA, 1, core::ptr::null_mut()),
            efi::Status::INVALID_PARAMETER
        );
        assert_eq!(mm_free_pages_impl(0, 1), efi::Status::INVALID_PARAMETER);
        assert_eq!(mm_free_pages_impl(0x4000, 0), efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_free_pages_fails_when_the_page_allocator_is_not_ready() {
        services();

        assert_eq!(mm_free_pages_impl(0x4000, 1), efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_installing_a_protocol_allocates_a_handle_and_makes_it_locatable() {
        let core = services();
        let handle = install(PROTOCOL_A, interface(0x11));

        assert!(!handle.is_null());
        assert!(core.protocol_db.is_protocol_installed(&PROTOCOL_A));

        let mut found: *mut c_void = core::ptr::null_mut();
        let mut guid = PROTOCOL_A;
        assert_eq!(mm_handle_protocol_impl(handle, &raw mut guid, &raw mut found), efi::Status::SUCCESS);
        assert_eq!(found, interface(0x11));

        found = core::ptr::null_mut();
        assert_eq!(mm_locate_protocol_impl(&raw mut guid, core::ptr::null_mut(), &raw mut found), efi::Status::SUCCESS);
        assert_eq!(found, interface(0x11));
    }

    #[test]
    fn test_a_second_protocol_can_join_an_existing_handle() {
        services();
        let handle = install(PROTOCOL_A, interface(0x11));

        assert_eq!(install_on(handle, PROTOCOL_B, interface(0x22)), Ok(handle));
        // The same protocol cannot be installed twice on one handle.
        assert_eq!(install_on(handle, PROTOCOL_B, interface(0x33)), Err(efi::Status::INVALID_PARAMETER));
    }

    #[test]
    fn test_install_protocol_rejects_null_handle_and_protocol_pointers() {
        services();
        let mut handle: efi::Handle = core::ptr::null_mut();
        let mut guid = PROTOCOL_A;

        assert_eq!(
            mm_install_protocol_interface_impl(
                core::ptr::null_mut(),
                &raw mut guid,
                efi::NATIVE_INTERFACE,
                interface(0x11)
            ),
            efi::Status::INVALID_PARAMETER
        );
        assert_eq!(
            mm_install_protocol_interface_impl(
                &raw mut handle,
                core::ptr::null_mut(),
                efi::NATIVE_INTERFACE,
                interface(0x11)
            ),
            efi::Status::INVALID_PARAMETER
        );
    }

    #[test]
    fn test_uninstalling_a_protocol_removes_it_from_the_database() {
        let core = services();
        let handle = install(PROTOCOL_A, interface(0x11));
        let mut guid = PROTOCOL_A;

        assert_eq!(mm_uninstall_protocol_interface_impl(handle, &raw mut guid, interface(0x11)), efi::Status::SUCCESS);
        assert!(!core.protocol_db.is_protocol_installed(&PROTOCOL_A));
    }

    #[test]
    fn test_uninstall_protocol_rejects_null_arguments_and_unknown_handles() {
        services();
        let handle = install(PROTOCOL_A, interface(0x11));
        let mut guid = PROTOCOL_A;

        assert_eq!(
            mm_uninstall_protocol_interface_impl(core::ptr::null_mut(), &raw mut guid, interface(0x11)),
            efi::Status::INVALID_PARAMETER
        );
        assert_eq!(
            mm_uninstall_protocol_interface_impl(handle, core::ptr::null_mut(), interface(0x11)),
            efi::Status::INVALID_PARAMETER
        );
        // Right handle, protocol that was never installed on it.
        let mut other = PROTOCOL_B;
        assert_ne!(mm_uninstall_protocol_interface_impl(handle, &raw mut other, interface(0x11)), efi::Status::SUCCESS);
    }

    #[test]
    fn test_handle_protocol_clears_the_out_parameter_before_rejecting_a_null_handle() {
        services();
        let mut found: *mut c_void = interface(0xDEAD);
        let mut guid = PROTOCOL_A;

        assert_eq!(
            mm_handle_protocol_impl(core::ptr::null_mut(), &raw mut guid, &raw mut found),
            efi::Status::INVALID_PARAMETER
        );
        assert!(found.is_null(), "the interface out-parameter is cleared before the handle is validated");
    }

    #[test]
    fn test_handle_protocol_rejects_null_pointers_and_uninstalled_protocols() {
        services();
        let handle = install(PROTOCOL_A, interface(0x11));
        let mut found: *mut c_void = core::ptr::null_mut();
        let mut guid = PROTOCOL_A;
        let mut absent = PROTOCOL_B;

        assert_eq!(
            mm_handle_protocol_impl(handle, core::ptr::null_mut(), &raw mut found),
            efi::Status::INVALID_PARAMETER
        );
        assert_eq!(
            mm_handle_protocol_impl(handle, &raw mut guid, core::ptr::null_mut()),
            efi::Status::INVALID_PARAMETER
        );
        assert_eq!(mm_handle_protocol_impl(handle, &raw mut absent, &raw mut found), efi::Status::UNSUPPORTED);
    }

    #[test]
    fn test_locate_protocol_reports_not_found_and_rejects_null_pointers() {
        services();
        let mut found: *mut c_void = core::ptr::null_mut();
        let mut guid = PROTOCOL_A;

        assert_eq!(
            mm_locate_protocol_impl(&raw mut guid, core::ptr::null_mut(), &raw mut found),
            efi::Status::NOT_FOUND
        );
        assert_eq!(
            mm_locate_protocol_impl(core::ptr::null_mut(), core::ptr::null_mut(), &raw mut found),
            efi::Status::INVALID_PARAMETER
        );
        assert_eq!(
            mm_locate_protocol_impl(&raw mut guid, core::ptr::null_mut(), core::ptr::null_mut()),
            efi::Status::INVALID_PARAMETER
        );
    }

    #[test]
    fn test_locate_handle_reports_the_required_size_before_filling_the_buffer() {
        services();
        let expected = install(PROTOCOL_A, interface(0x11));
        let mut guid = PROTOCOL_A;

        // Undersized buffer: the required size is reported and nothing is written.
        let mut size = 0usize;
        assert_eq!(
            mm_locate_handle_impl(
                efi::BY_PROTOCOL,
                &raw mut guid,
                core::ptr::null_mut(),
                &raw mut size,
                core::ptr::null_mut()
            ),
            efi::Status::BUFFER_TOO_SMALL
        );
        assert_eq!(size, core::mem::size_of::<efi::Handle>());

        // Correctly sized buffer: the handle is copied out.
        let mut handles = [core::ptr::null_mut::<c_void>(); 1];
        assert_eq!(
            mm_locate_handle_impl(
                efi::BY_PROTOCOL,
                &raw mut guid,
                core::ptr::null_mut(),
                &raw mut size,
                handles.as_mut_ptr()
            ),
            efi::Status::SUCCESS
        );
        assert_eq!(handles[0], expected);
    }

    #[test]
    fn test_locate_handle_rejects_a_null_buffer_that_claims_to_be_large_enough() {
        services();
        install(PROTOCOL_A, interface(0x11));
        let mut guid = PROTOCOL_A;
        let mut size = core::mem::size_of::<efi::Handle>();

        assert_eq!(
            mm_locate_handle_impl(
                efi::BY_PROTOCOL,
                &raw mut guid,
                core::ptr::null_mut(),
                &raw mut size,
                core::ptr::null_mut()
            ),
            efi::Status::INVALID_PARAMETER
        );
    }

    #[test]
    fn test_locate_handle_returns_every_handle_for_an_all_handles_search() {
        services();
        install(PROTOCOL_A, interface(0x11));
        install(PROTOCOL_B, interface(0x22));

        let mut size = 2 * core::mem::size_of::<efi::Handle>();
        let mut handles = [core::ptr::null_mut::<c_void>(); 2];
        assert_eq!(
            mm_locate_handle_impl(
                efi::ALL_HANDLES,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                &raw mut size,
                handles.as_mut_ptr()
            ),
            efi::Status::SUCCESS
        );
        assert!(handles.iter().all(|h| !h.is_null()));
    }

    #[test]
    fn test_locate_handle_rejects_unsupported_searches_and_missing_arguments() {
        services();
        let mut size = 0usize;

        // No `buffer_size` out-parameter at all.
        assert_eq!(
            mm_locate_handle_impl(
                efi::ALL_HANDLES,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut()
            ),
            efi::Status::INVALID_PARAMETER
        );
        // BY_PROTOCOL without a protocol.
        assert_eq!(
            mm_locate_handle_impl(
                efi::BY_PROTOCOL,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                &raw mut size,
                core::ptr::null_mut()
            ),
            efi::Status::INVALID_PARAMETER
        );
        // A search type the user core does not implement.
        assert_eq!(
            mm_locate_handle_impl(
                efi::BY_REGISTER_NOTIFY,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                &raw mut size,
                core::ptr::null_mut()
            ),
            efi::Status::UNSUPPORTED
        );
        // Nothing installed yet.
        assert_eq!(
            mm_locate_handle_impl(
                efi::ALL_HANDLES,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                &raw mut size,
                core::ptr::null_mut()
            ),
            efi::Status::NOT_FOUND
        );
    }

    #[test]
    fn test_a_protocol_notify_fires_when_the_protocol_arrives_and_stops_once_unregistered() {
        services();
        let mut registration: *mut c_void = core::ptr::null_mut();
        let guid = PROTOCOL_A;

        assert_eq!(
            mm_register_protocol_notify_impl(&raw const guid, notify_fn(), &raw mut registration),
            efi::Status::SUCCESS
        );
        assert!(!registration.is_null());

        install(PROTOCOL_A, interface(0x11));
        assert_eq!(NOTIFY_CALLS.load(Ordering::Relaxed), 1);

        // Function == NULL is the unregister form.
        assert_eq!(mm_register_protocol_notify_impl(&raw const guid, 0, &raw mut registration), efi::Status::SUCCESS);

        install_on(core::ptr::null_mut(), PROTOCOL_A, interface(0x22)).expect("install succeeds");
        assert_eq!(NOTIFY_CALLS.load(Ordering::Relaxed), 1, "the callback was unregistered");
    }

    #[test]
    fn test_register_protocol_notify_rejects_null_pointers_and_unknown_registrations() {
        services();
        let mut registration: *mut c_void = core::ptr::null_mut();
        let guid = PROTOCOL_A;

        assert_eq!(
            mm_register_protocol_notify_impl(core::ptr::null_mut(), notify_fn(), &raw mut registration),
            efi::Status::INVALID_PARAMETER
        );
        assert_eq!(
            mm_register_protocol_notify_impl(&raw const guid, notify_fn(), core::ptr::null_mut()),
            efi::Status::INVALID_PARAMETER
        );
        // Unregister form with a null token.
        assert_eq!(
            mm_register_protocol_notify_impl(&raw const guid, 0, &raw mut registration),
            efi::Status::INVALID_PARAMETER
        );
        // Unregister form with a token that was never issued.
        registration = interface(0xBEEF);
        assert_ne!(mm_register_protocol_notify_impl(&raw const guid, 0, &raw mut registration), efi::Status::SUCCESS);
    }

    #[test]
    fn test_a_registered_mmi_handler_runs_when_its_type_is_dispatched() {
        services();
        let mut dispatch_handle: efi::Handle = core::ptr::null_mut();

        // SAFETY: the thunks dereference only the pointers they null-check.
        unsafe {
            assert_eq!(
                mmi_handler_register_impl(Some(recording_handler), &raw const HANDLER_GUID, &raw mut dispatch_handle),
                efi::Status::SUCCESS
            );

            assert_eq!(
                mmi_manage_impl(
                    &raw const HANDLER_GUID,
                    core::ptr::null(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut()
                ),
                efi::Status::SUCCESS
            );
            assert_eq!(MMI_CALLS.load(Ordering::Relaxed), 1);

            assert_eq!(mmi_handler_unregister_impl(dispatch_handle), efi::Status::SUCCESS);
            assert_eq!(
                mmi_manage_impl(
                    &raw const HANDLER_GUID,
                    core::ptr::null(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut()
                ),
                efi::Status::NOT_FOUND
            );
            assert_eq!(MMI_CALLS.load(Ordering::Relaxed), 1, "the handler was unregistered");
        }
    }

    #[test]
    fn test_a_root_mmi_handler_is_registered_when_no_type_is_given() {
        services();
        let mut dispatch_handle: efi::Handle = core::ptr::null_mut();

        // SAFETY: the thunks dereference only the pointers they null-check.
        unsafe {
            assert_eq!(
                mmi_handler_register_impl(Some(recording_handler), core::ptr::null(), &raw mut dispatch_handle),
                efi::Status::SUCCESS
            );
            assert_eq!(
                mmi_manage_impl(core::ptr::null(), core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut()),
                efi::Status::SUCCESS
            );
            assert_eq!(MMI_CALLS.load(Ordering::Relaxed), 1);
        }
    }

    #[test]
    fn test_mmi_handler_register_requires_a_dispatch_handle_out_parameter() {
        services();

        // SAFETY: the thunk dereferences only the pointers it null-checks.
        unsafe {
            assert_eq!(
                mmi_handler_register_impl(Some(recording_handler), &raw const HANDLER_GUID, core::ptr::null_mut()),
                efi::Status::INVALID_PARAMETER
            );
        }
    }

    #[test]
    fn test_mmi_handler_register_rejects_a_null_handler() {
        services();
        let mut dispatch_handle: efi::Handle = core::ptr::null_mut();

        // SAFETY: the thunk dereferences only the pointers it null-checks.
        unsafe {
            assert_eq!(
                mmi_handler_register_impl(None, &raw const HANDLER_GUID, &raw mut dispatch_handle),
                efi::Status::INVALID_PARAMETER
            );
        }
        assert!(dispatch_handle.is_null(), "no handle is returned for a rejected handler");
    }

    #[test]
    fn test_unregistering_an_unknown_mmi_handler_is_reported_not_found() {
        services();

        // SAFETY: unregistering by handle never dereferences it.
        unsafe {
            assert_ne!(mmi_handler_unregister_impl(interface(0x99)), efi::Status::SUCCESS);
        }
    }

    #[test]
    fn test_a_configuration_table_entry_can_be_added_then_removed() {
        let core = services();
        core.init_mm_system_table();

        // SAFETY: the thunk dereferences only the null-checked GUID pointer.
        unsafe {
            assert_eq!(
                mm_install_configuration_table_impl(
                    core.mm_system_table_ptr(),
                    &raw const TABLE_GUID,
                    interface(0x77),
                    8
                ),
                efi::Status::SUCCESS
            );
            assert_eq!(core.config_table_db.get_configuration_table(&TABLE_GUID), Some(interface(0x77)));

            // A null table removes the entry.
            assert_eq!(
                mm_install_configuration_table_impl(
                    core.mm_system_table_ptr(),
                    &raw const TABLE_GUID,
                    core::ptr::null_mut(),
                    0
                ),
                efi::Status::SUCCESS
            );
            assert_eq!(core.config_table_db.get_configuration_table(&TABLE_GUID), None);
        }
    }

    #[test]
    fn test_install_configuration_table_rejects_a_null_guid_and_an_unknown_removal() {
        let core = services();

        // SAFETY: the thunk dereferences only the null-checked GUID pointer.
        unsafe {
            assert_eq!(
                mm_install_configuration_table_impl(core.mm_system_table_ptr(), core::ptr::null(), interface(0x77), 8),
                efi::Status::INVALID_PARAMETER
            );
            // Removing an entry that was never added.
            assert_eq!(
                mm_install_configuration_table_impl(
                    core.mm_system_table_ptr(),
                    &raw const TABLE_GUID,
                    core::ptr::null_mut(),
                    0
                ),
                efi::Status::NOT_FOUND
            );
        }
    }
}
