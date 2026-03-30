//! Platform Initialization Specification MM Core Interface
//!
//! This module contains definitions related to the MM Core Interface as defined
//! in the UEFI Platform Initialization Specification.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;
use r_efi::efi;
use crate::pi::spec_version;

/// MMST signature: `'S', 'M', 'S', 'T'` (same as C `MM_MMST_SIGNATURE`).
pub const MM_MMST_SIGNATURE: u32 = u32::from_le_bytes([b'S', b'M', b'S', b'T']);

/// MMST revision tuples
pub const MM_MMST_REVISION_MAJOR: u32 = spec_version::PI_SEPCIFICATION_MAJOR_REVISION;
pub const MM_MMST_REVISION_MINOR: u32 = spec_version::PI_SEPCIFICATION_MINOR_REVISION;

/// PI Specification version encoded as `(major << 16) | minor`.
pub const MM_SYSTEM_TABLE_REVISION: u32 = (MM_MMST_REVISION_MAJOR << 16) | MM_MMST_REVISION_MINOR;

//
// This gnarly POS of EFI_MM_CPU_IO_PROTOCOL is embedded in MMST,
// so we need to define it here to be able to parse the MMST correctly.
//

/// A single MM I/O access function pointer.
///
/// Matches the C typedef `EFI_MM_CPU_IO`:
/// ```c
/// typedef EFI_STATUS (EFIAPI *EFI_MM_CPU_IO)(
///   IN     CONST EFI_MM_CPU_IO_PROTOCOL *This,
///   IN     EFI_MM_IO_WIDTH              Width,
///   IN     UINT64                       Address,
///   IN     UINTN                        Count,
///   IN OUT VOID                         *Buffer
/// );
/// ```
pub type MmCpuIoFn = unsafe extern "efiapi" fn(
    this: *const MmCpuIoAccess,
    width: usize,
    address: u64,
    count: usize,
    buffer: *mut c_void,
) -> efi::Status;

/// MM CPU I/O access pair (Read + Write).
///
/// Matches `EFI_MM_IO_ACCESS`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MmCpuIoAccess {
    pub read: MmCpuIoFn,
    pub write: MmCpuIoFn,
}

/// The `EFI_MM_CPU_IO_PROTOCOL` embedded in the system table.
///
/// ```c
/// typedef struct _EFI_MM_CPU_IO_PROTOCOL {
///   EFI_MM_IO_ACCESS  Mem;
///   EFI_MM_IO_ACCESS  Io;
/// } EFI_MM_CPU_IO_PROTOCOL;
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MmCpuIoProtocol {
    pub mem: MmCpuIoAccess,
    pub io: MmCpuIoAccess,
}

/// `EFI_MM_INSTALL_CONFIGURATION_TABLE`
pub type MmInstallConfigurationTableFn = unsafe extern "efiapi" fn(
    system_table: *const EfiMmSystemTable,
    guid: *const efi::Guid,
    table: *mut c_void,
    table_size: usize,
) -> efi::Status;

/// `EFI_ALLOCATE_POOL` (shared with Boot Services)
pub type MmAllocatePoolFn = unsafe extern "efiapi" fn(
    pool_type: efi::MemoryType,
    size: usize,
    buffer: *mut *mut c_void,
) -> efi::Status;

/// `EFI_FREE_POOL` (shared with Boot Services)
pub type MmFreePoolFn = unsafe extern "efiapi" fn(
    buffer: *mut c_void,
) -> efi::Status;

/// `EFI_ALLOCATE_PAGES` (shared with Boot Services)
pub type MmAllocatePagesFn = unsafe extern "efiapi" fn(
    alloc_type: efi::AllocateType,
    memory_type: efi::MemoryType,
    pages: usize,
    memory: *mut efi::PhysicalAddress,
) -> efi::Status;

/// `EFI_FREE_PAGES` (shared with Boot Services)
pub type MmFreePagesFn = unsafe extern "efiapi" fn(
    memory: efi::PhysicalAddress,
    pages: usize,
) -> efi::Status;

/// `EFI_MM_STARTUP_THIS_AP`
pub type MmStartupThisApFn = unsafe extern "efiapi" fn(
    procedure: usize,
    cpu_number: usize,
    proc_arguments: *mut c_void,
) -> efi::Status;

/// `EFI_INSTALL_PROTOCOL_INTERFACE` (shared with Boot Services)
pub type MmInstallProtocolInterfaceFn = unsafe extern "efiapi" fn(
    handle: *mut efi::Handle,
    protocol: *mut efi::Guid,
    interface_type: efi::InterfaceType,
    interface: *mut c_void,
) -> efi::Status;

/// `EFI_UNINSTALL_PROTOCOL_INTERFACE` (shared with Boot Services)
pub type MmUninstallProtocolInterfaceFn = unsafe extern "efiapi" fn(
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    interface: *mut c_void,
) -> efi::Status;

/// `EFI_HANDLE_PROTOCOL` (shared with Boot Services)
pub type MmHandleProtocolFn = unsafe extern "efiapi" fn(
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    interface: *mut *mut c_void,
) -> efi::Status;

/// `EFI_MM_REGISTER_PROTOCOL_NOTIFY`
pub type MmRegisterProtocolNotifyFn = unsafe extern "efiapi" fn(
    protocol: *const efi::Guid,
    function: usize,
    registration: *mut *mut c_void,
) -> efi::Status;

/// `EFI_LOCATE_HANDLE` (shared with Boot Services)
pub type MmLocateHandleFn = unsafe extern "efiapi" fn(
    search_type: efi::LocateSearchType,
    protocol: *mut efi::Guid,
    search_key: *mut c_void,
    buffer_size: *mut usize,
    buffer: *mut efi::Handle,
) -> efi::Status;

/// `EFI_LOCATE_PROTOCOL` (shared with Boot Services)
pub type MmLocateProtocolFn = unsafe extern "efiapi" fn(
    protocol: *mut efi::Guid,
    registration: *mut c_void,
    interface: *mut *mut c_void,
) -> efi::Status;

/// `EFI_MM_INTERRUPT_MANAGE`
pub type MmiManageFn = unsafe extern "efiapi" fn(
    handler_type: *const efi::Guid,
    context: *const c_void,
    comm_buffer: *mut c_void,
    comm_buffer_size: *mut usize,
) -> efi::Status;

/// MMI handler entry point.
///
/// Matches the C typedef `EFI_MM_HANDLER_ENTRY_POINT`.
pub type MmiHandlerEntryPoint = unsafe extern "efiapi" fn(
    dispatch_handle: efi::Handle,
    context: *const c_void,
    comm_buffer: *mut c_void,
    comm_buffer_size: *mut usize,
) -> efi::Status;

/// `EFI_MM_INTERRUPT_REGISTER`
pub type MmiHandlerRegisterFn = unsafe extern "efiapi" fn(
    handler: MmiHandlerEntryPoint,
    handler_type: *const efi::Guid,
    dispatch_handle: *mut efi::Handle,
) -> efi::Status;

/// `EFI_MM_INTERRUPT_UNREGISTER`
pub type MmiHandlerUnregisterFn = unsafe extern "efiapi" fn(
    dispatch_handle: efi::Handle,
) -> efi::Status;

/// EFI_MM_ENTRY_CONTEXT structure.
///
/// Processor information and functionality needed by MM Foundation.
/// Matches the C `EFI_MM_ENTRY_CONTEXT` / `EFI_SMM_ENTRY_CONTEXT` from PI specification.
///
/// Layout (x86_64, all fields 8 bytes):
/// - `mm_startup_this_ap`: Function pointer for `EFI_MM_STARTUP_THIS_AP`
/// - `currently_executing_cpu`: Index of the processor executing the MM Foundation
/// - `number_of_cpus`: Total number of possible processors in the platform (1-based)
/// - `cpu_save_state_size`: Pointer to array of save state sizes per CPU
/// - `cpu_save_state`: Pointer to array of CPU save state pointers
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct EfiMmEntryContext {
    /// Function pointer for EFI_MM_STARTUP_THIS_AP.
    pub mm_startup_this_ap: u64,
    /// Index of the currently executing CPU.
    pub currently_executing_cpu: u64,
    /// Total number of CPUs (1-based).
    pub number_of_cpus: u64,
    /// Pointer to array of per-CPU save state sizes.
    pub cpu_save_state_size: u64,
    /// Pointer to array of per-CPU save state pointers.
    pub cpu_save_state: u64,
}

/// The Management Mode System Table (MMST).
///
/// This is the `#[repr(C)]` Rust definition of the C `_EFI_MM_SYSTEM_TABLE`
/// from `PiMmCis.h`. The table pointer is passed as the second argument to
/// every MM driver's entry point:
///
/// ```c
/// EFI_STATUS EFIAPI DriverEntry(EFI_HANDLE ImageHandle, EFI_MM_SYSTEM_TABLE *MmSt);
/// ```
#[repr(C)]
pub struct EfiMmSystemTable {
    // ---- Table Header ----
    pub hdr: efi::TableHeader,

    // ---- Firmware info ----
    /// Pointer to a NUL-terminated UCS-2 vendor string (may be null).
    pub mm_firmware_vendor: *mut u16,
    /// Firmware revision number.
    pub mm_firmware_revision: u32,

    // ---- Configuration Table ----
    pub mm_install_configuration_table: MmInstallConfigurationTableFn,

    // ---- I/O services (embedded protocol) ----
    pub mm_io: MmCpuIoProtocol,

    // ---- Memory services ----
    pub mm_allocate_pool: MmAllocatePoolFn,
    pub mm_free_pool: MmFreePoolFn,
    pub mm_allocate_pages: MmAllocatePagesFn,
    pub mm_free_pages: MmFreePagesFn,

    // ---- MP service ----
    pub mm_startup_this_ap: MmStartupThisApFn,

    // ---- CPU information ----
    pub currently_executing_cpu: usize,
    pub number_of_cpus: usize,
    pub cpu_save_state_size: *mut usize,
    pub cpu_save_state: *mut *mut c_void,

    // ---- Extensibility table ----
    pub number_of_table_entries: usize,
    pub mm_configuration_table: *mut efi::ConfigurationTable,

    // ---- Protocol services ----
    pub mm_install_protocol_interface: MmInstallProtocolInterfaceFn,
    pub mm_uninstall_protocol_interface: MmUninstallProtocolInterfaceFn,
    pub mm_handle_protocol: MmHandleProtocolFn,
    pub mm_register_protocol_notify: MmRegisterProtocolNotifyFn,
    pub mm_locate_handle: MmLocateHandleFn,
    pub mm_locate_protocol: MmLocateProtocolFn,

    // ---- MMI management ----
    pub mmi_manage: MmiManageFn,
    pub mmi_handler_register: MmiHandlerRegisterFn,
    pub mmi_handler_unregister: MmiHandlerUnregisterFn,
}
