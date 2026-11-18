//! MM System Table (MMST) Construction
//!
//! Builds the `EfiMmSystemTable` handed to MM drivers. Each function pointer is an
//! `extern "efiapi"` thunk that checks its raw arguments and forwards through the
//! `MM_SERVICES` bridge to the [`MmServices`] implementation on [`MmUserCore`], which uses
//! the core's databases ([`ProtocolDatabase`], [`MmiDatabase`], [`MmConfigurationTableDb`]).
//! The table and trait types come from [`patina::management_mode::mm_services`].
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

/// The bridge the `EfiMmSystemTable` thunks forward through.
///
/// [`init_mm_services`] points it at the [`MmUserCore`] provider. Rust code calls the
/// provider directly and never goes through the table's function pointers.
static MM_SERVICES: StandardMmServices = StandardMmServices::new_uninit();

/// Registers the [`MmServices`] provider the `EfiMmSystemTable` thunks forward to.
///
/// Must be called once during startup, before any driver can call the table.
pub(crate) fn init_mm_services(provider: &'static (dyn MmServices + Sync)) {
    MM_SERVICES.init(provider);
}

/// Logs a failure that a thunk found in its own argument checks, then returns `status`.
///
/// Statuses from deeper in the core are already logged there. `NOT_FOUND`, `UNSUPPORTED` and
/// `BUFFER_TOO_SMALL` are how drivers probe for protocols and size buffers, so they are
/// logged at debug level; any other failure is logged as an error.
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

/// Returns the status `MmFreePages` reports for a failed free.
///
/// A failed `FreePage` syscall carries the EFI status the supervisor returned, such as
/// `NOT_FOUND` for pages it did not allocate. Every other failure is a request the user core
/// rejected before the syscall.
fn free_pages_status(err: crate::pool_allocator::PageAllocError) -> efi::Status {
    match err {
        crate::pool_allocator::PageAllocError::SyscallFailed(status) => efi::Status::from_usize(status as usize),
        _ => efi::Status::INVALID_PARAMETER,
    }
}

/// Builds the MM System Table value.
///
/// Every service is a thunk into `MM_SERVICES`; the only state in the table is the CPU
/// and configuration table fields that the core updates in place.
/// [`MmUserCore::init_mm_system_table`] boxes the value and hands the pointer to drivers.
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

/// CPU I/O stub that always returns `UNSUPPORTED`.
///
/// ## Safety
///
/// Dereferences none of its arguments. It is `unsafe` only to match the `MmCpuIoFn`
/// signature.
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
/// ## Safety
///
/// `guid` must be null or point to a valid `efi::Guid`. A non-null `table` must stay valid
/// while the entry is installed.
unsafe extern "efiapi" fn mm_install_configuration_table_impl(
    _system_table: *const EfiMmSystemTable,
    guid: *const efi::Guid,
    table: *mut c_void,
    table_size: usize,
) -> efi::Status {
    if guid.is_null() {
        return report("MmInstallConfigurationTable", efi::Status::INVALID_PARAMETER);
    }

    // SAFETY: `guid` was null-checked above, and the caller guarantees a non-null pointer
    // references a valid `efi::Guid`.
    let guid = unsafe { &*guid };
    // SAFETY: `table` is forwarded unchanged; the caller owns its validity and lifetime.
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
            // SAFETY: `buffer` was null-checked above and is a writable out-parameter.
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
            // SAFETY: `memory` was null-checked above and is a writable out-parameter.
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

/// AP startup stub that always returns `UNSUPPORTED`.
///
/// ## Safety
///
/// Dereferences none of its arguments. It is `unsafe` only to match the
/// `MmStartupThisApFn` signature.
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

    // SAFETY: `protocol` was null-checked above, and the caller guarantees it references a valid
    // `efi::Guid`.
    let guid = unsafe { &*protocol };
    // SAFETY: `handle` was null-checked above and is a readable in/out parameter.
    let caller_handle = unsafe { *handle };
    let in_handle = (!caller_handle.is_null()).then_some(caller_handle);

    // SAFETY: `interface` is forwarded unchanged; the caller owns its validity and lifetime.
    match unsafe { MM_SERVICES.install_protocol_interface(in_handle, guid, interface) } {
        Ok(new_handle) => {
            // SAFETY: `handle` was null-checked above and is a writable out-parameter.
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

    // SAFETY: `protocol` was null-checked above, and the caller guarantees it references a valid
    // `efi::Guid`.
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
    // Clear the out-parameter before the lookup.
    // SAFETY: `interface` was null-checked above and is a writable out-parameter.
    unsafe { *interface = core::ptr::null_mut() };

    if handle.is_null() {
        return report("MmHandleProtocol", efi::Status::INVALID_PARAMETER);
    }

    // SAFETY: `protocol` was null-checked above, and the caller guarantees it references a valid
    // `efi::Guid`.
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

    // SAFETY: `protocol` was null-checked above, and the caller guarantees it references a valid
    // `efi::Guid`.
    let guid = unsafe { &*protocol };

    if function == 0 {
        // A null function unregisters the notification identified by `*registration`.
        // SAFETY: `registration` was null-checked above and is readable.
        let registration_ptr = unsafe { *registration };
        match NonNull::new(registration_ptr).map(Registration::new) {
            Some(registration_token) => match MM_SERVICES.unregister_protocol_notify(guid, registration_token) {
                Ok(()) => efi::Status::SUCCESS,
                Err(status) => status,
            },
            None => report("MmRegisterProtocolNotify", efi::Status::INVALID_PARAMETER),
        }
    } else {
        // SAFETY: `function` is a non-null `EFI_MM_NOTIFY_FN` pointer passed as a `usize`;
        // transmuting it back to the matching function pointer type is sound.
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
    // SAFETY: `buffer_size` was null-checked at the top of the function and is a readable and
    // writable `usize`.
    let caller_size = unsafe { *buffer_size };
    // SAFETY: As above.
    unsafe { *buffer_size = required_size };

    if caller_size < required_size {
        return report("MmLocateHandle", efi::Status::BUFFER_TOO_SMALL);
    }

    if buffer.is_null() {
        return report("MmLocateHandle", efi::Status::INVALID_PARAMETER);
    }

    // SAFETY: `buffer` was null-checked above and `caller_size >= required_size`, so it holds at
    // least `handles.len()` handles. The source is a local `Vec`, so they do not overlap.
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
    // Clear the out-parameter before the lookup.
    // SAFETY: `interface` was null-checked above and is a writable out-parameter.
    unsafe { *interface = core::ptr::null_mut() };

    // SAFETY: `protocol` was null-checked above, and the caller guarantees it references a valid
    // `efi::Guid`.
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
/// ## Safety
///
/// `handler_type` must be null or point to a valid `efi::Guid`. `context`, `comm_buffer`
/// and `comm_buffer_size` are passed to the handlers unchanged and must be valid for them.
unsafe extern "efiapi" fn mmi_manage_impl(
    handler_type: *const efi::Guid,
    context: *const c_void,
    comm_buffer: *mut c_void,
    comm_buffer_size: *mut usize,
) -> efi::Status {
    // SAFETY: `handler_type` is null-checked here; when non-null the caller guarantees it
    // references a valid `efi::Guid`.
    let guid = if handler_type.is_null() { None } else { Some(unsafe { &*handler_type }) };

    // SAFETY: `context`, `comm_buffer` and `comm_buffer_size` are forwarded unchanged to the handlers.
    unsafe { MM_SERVICES.mmi_manage(guid, context, comm_buffer, comm_buffer_size) }
}

/// Registers an MMI handler entry point for a particular handler type.
///
/// ## Safety
///
/// `handler_type` must be null or point to a valid `efi::Guid`, and a non-null
/// `dispatch_handle` must be writable.
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

    // SAFETY: `handler_type` is null-checked here; when non-null the caller guarantees it
    // references a valid `efi::Guid`.
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
/// ## Safety
///
/// Dereferences none of its arguments. It is `unsafe` only to match the
/// `MmiHandlerUnregisterFn` signature.
unsafe extern "efiapi" fn mmi_handler_unregister_impl(dispatch_handle: efi::Handle) -> efi::Status {
    match MM_SERVICES.mmi_handler_unregister(dispatch_handle) {
        Ok(()) => efi::Status::SUCCESS,
        Err(status) => status,
    }
}

/// Header stored in front of every pool allocation.
///
/// `MmFreePool` gets no size, but `GlobalAlloc::dealloc` needs the allocation's layout. The
/// header records the size for [`dealloc_pool`], and its signature lets `MmFreePool` reject a
/// pointer `MmAllocatePool` never returned.
#[repr(C)]
struct PoolHeader {
    /// [`POOL_SIGNATURE`] while the allocation is live.
    signature: u64,
    /// Size of the whole allocation in bytes, header included.
    size: usize,
}

/// Marks a live pool allocation.
const POOL_SIGNATURE: u64 = u64::from_le_bytes(*b"PoolHead");

/// Alignment of every pool allocation. The PI specification requires eight bytes.
const POOL_ALIGNMENT: usize = 8;

/// Bytes the header takes in front of the caller's buffer. A multiple of [`POOL_ALIGNMENT`], so
/// the caller's buffer keeps the allocation's alignment.
const POOL_HEADER_SIZE: usize = core::mem::size_of::<PoolHeader>();

const _: () = assert!(POOL_HEADER_SIZE.is_multiple_of(POOL_ALIGNMENT));

/// Allocates `size` bytes after a [`PoolHeader`] and returns the address after the header.
///
/// Returns `INVALID_PARAMETER` when `size` with the header cannot form a layout, and
/// `OUT_OF_RESOURCES` when the heap cannot satisfy it.
fn alloc_pool(size: usize) -> Result<*mut u8, efi::Status> {
    let total = size.checked_add(POOL_HEADER_SIZE);
    let layout =
        total.and_then(|total| core::alloc::Layout::from_size_align(total, POOL_ALIGNMENT).ok()).ok_or_else(|| {
            log::error!("MmAllocatePool: size {size} does not form a valid layout");
            efi::Status::INVALID_PARAMETER
        })?;

    // SAFETY: `layout` is at least `POOL_HEADER_SIZE` bytes, so it is not zero sized, as the
    // `GlobalAlloc::alloc` contract requires.
    let base = unsafe { alloc::alloc::alloc(layout) };
    if base.is_null() {
        log::error!("MmAllocatePool: the heap could not satisfy {size} bytes");
        return Err(efi::Status::OUT_OF_RESOURCES);
    }

    // SAFETY: `base` is a fresh allocation of `layout`, which starts with room for an aligned
    // header and has the caller's `size` bytes after it.
    unsafe {
        base.cast::<PoolHeader>().write(PoolHeader { signature: POOL_SIGNATURE, size: layout.size() });
        Ok(base.add(POOL_HEADER_SIZE))
    }
}

/// Frees a pool allocation made by [`MmServices::allocate_pool`].
///
/// Returns `INVALID_PARAMETER`, without freeing, when `buffer` is not preceded by a live
/// [`PoolHeader`].
fn dealloc_pool(buffer: *mut u8) -> Result<(), efi::Status> {
    let header = (buffer as usize)
        .checked_sub(POOL_HEADER_SIZE)
        .filter(|address| address.is_multiple_of(POOL_ALIGNMENT))
        .map(|address| buffer.with_addr(address).cast::<PoolHeader>())
        .ok_or_else(|| {
            log::error!("MmFreePool: {buffer:p} is not a pool allocation");
            efi::Status::INVALID_PARAMETER
        })?;

    // SAFETY: `buffer` is trusted to come from `allocate_pool`, which places an aligned
    // `PoolHeader` right before it. A pointer that did not come from there is caught by the
    // signature check. Clearing the signature before the free stops a second free from passing it.
    unsafe {
        let PoolHeader { signature, size } = header.read();
        if signature != POOL_SIGNATURE {
            log::error!("MmFreePool: {buffer:p} is not a live pool allocation");
            return Err(efi::Status::INVALID_PARAMETER);
        }
        (*header).signature = 0;
        let layout = core::alloc::Layout::from_size_align_unchecked(size, POOL_ALIGNMENT);
        alloc::alloc::dealloc(header.cast::<u8>(), layout);
    }
    Ok(())
}

/// The [`MmServices`] implementation, backed by the user core's databases.
///
/// Both the `EfiMmSystemTable` thunks and the core's own Rust code use this trait, so
/// neither calls through the C function-pointer table.
impl MmServices for MmUserCore {
    /// Allocate pool memory. `pool_type` is ignored; the pool always uses runtime services data.
    fn allocate_pool(&self, _pool_type: efi::MemoryType, size: usize) -> Result<*mut u8, efi::Status> {
        if size == 0 {
            log::error!("MmAllocatePool: size is 0");
            return Err(efi::Status::INVALID_PARAMETER);
        }
        alloc_pool(size)
    }

    /// Free pool memory.
    fn free_pool(&self, buffer: *mut u8) -> Result<(), efi::Status> {
        if buffer.is_null() {
            log::error!("MmFreePool: buffer pointer is null");
            return Err(efi::Status::INVALID_PARAMETER);
        }
        dealloc_pool(buffer)
    }

    /// Allocate pages.
    ///
    /// The supervisor chooses the address, so only `AllocateAnyPages` is supported and other
    /// types return `UNSUPPORTED`. `memory_type` is ignored; pages are runtime services data.
    fn allocate_pages(
        &self,
        alloc_type: efi::AllocateType,
        _memory_type: efi::MemoryType,
        pages: usize,
    ) -> Result<u64, efi::Status> {
        if alloc_type != efi::ALLOCATE_ANY_PAGES {
            log::error!("MmAllocatePages: allocation type {alloc_type} is not supported");
            return Err(efi::Status::UNSUPPORTED);
        }
        if pages == 0 {
            log::error!("MmAllocatePages: page count is 0");
            return Err(efi::Status::INVALID_PARAMETER);
        }
        // The syscall allocator logs why the request failed.
        crate::mm_mem::SYSCALL_PAGE_ALLOCATOR.allocate_pages(pages).map_err(|_| efi::Status::OUT_OF_RESOURCES)
    }

    /// Free pages.
    fn free_pages(&self, memory: u64, pages: usize) -> Result<(), efi::Status> {
        if memory == 0 || pages == 0 {
            log::error!("MmFreePages: rejected address 0x{memory:016x} with {pages} page(s)");
            return Err(efi::Status::INVALID_PARAMETER);
        }
        // The syscall allocator logs why the request failed.
        crate::mm_mem::SYSCALL_PAGE_ALLOCATOR.free_pages(memory, pages).map_err(free_pages_status)
    }

    /// Install a protocol interface on a handle.
    ///
    /// ## Safety
    ///
    /// `interface` must be a valid pointer to the protocol structure or null, and
    /// must remain valid for as long as the interface is installed.
    unsafe fn install_protocol_interface(
        &self,
        handle: Option<efi::Handle>,
        protocol: &efi::Guid,
        interface: *mut c_void,
    ) -> Result<efi::Handle, efi::Status> {
        // `None` becomes a null handle, for which the database allocates a new handle.
        self.protocol_db.install_protocol(handle.unwrap_or(core::ptr::null_mut()), protocol, interface)
    }

    /// Uninstall a protocol interface from a handle.
    ///
    /// ## Safety
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

    /// Query a handle for a protocol. Returns `UNSUPPORTED` if the handle lacks it.
    ///
    /// ## Safety
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
    /// ## Safety
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
    /// ## Safety
    ///
    /// `context`, `comm_buffer`, and `comm_buffer_size` are optional, but each must be valid when
    /// non-null.
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
    fn mmi_handler_register(
        &self,
        handler: MmiHandlerEntryPoint,
        handler_type: Option<&efi::Guid>,
    ) -> Result<efi::Handle, efi::Status> {
        self.mmi_db.mmi_handler_register(handler, handler_type)
    }

    /// Unregister an MMI handler.
    fn mmi_handler_unregister(&self, dispatch_handle: efi::Handle) -> Result<(), efi::Status> {
        self.mmi_db.mmi_handler_unregister(dispatch_handle)
    }

    /// Add, update, or remove a configuration-table entry.
    ///
    /// ## Safety
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
    fn register_protocol_notify(
        &self,
        protocol: &efi::Guid,
        notify: ProtocolNotify,
    ) -> Result<Registration, efi::Status> {
        self.protocol_db.register_protocol_notify(protocol, notify).map(Registration::new)
    }

    /// Unregister a previously registered protocol-install notification.
    fn unregister_protocol_notify(&self, protocol: &efi::Guid, registration: Registration) -> Result<(), efi::Status> {
        self.protocol_db.unregister_protocol_notify(protocol, registration.as_ptr())
    }

    /// Return the handles matching a search type and optional protocol. Only `ALL_HANDLES` and
    /// `BY_PROTOCOL` are supported; other search types return `UNSUPPORTED`.
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
