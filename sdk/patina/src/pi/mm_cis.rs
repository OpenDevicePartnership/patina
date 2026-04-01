//! Management Mode (MM) Core Interface Definitions
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

use crate::pi::spec_version;
use core::ffi::c_void;
use r_efi::{
    efi,
    efi::{
        BootAllocatePages, BootAllocatePool, BootFreePages, BootFreePool, BootHandleProtocol,
        BootInstallProtocolInterface, BootLocateHandle, BootLocateProtocol, BootUninstallProtocolInterface,
    },
};

/// MMST signature: `'S', 'M', 'S', 'T'` (same as C `MM_MMST_SIGNATURE`).
pub const MM_MMST_SIGNATURE: u32 = u32::from_le_bytes([b'S', b'M', b'S', b'T']);

/// MMST major revision, the same as the PI Specification major revision.
pub const MM_MMST_REVISION_MAJOR: u32 = spec_version::PI_SPECIFICATION_MAJOR_REVISION;

/// MMST minor revision, the same as the PI Specification minor revision.
pub const MM_MMST_REVISION_MINOR: u32 = spec_version::PI_SPECIFICATION_MINOR_REVISION;

/// PI Specification version encoded as `(major << 16) | minor`.
pub const MM_SYSTEM_TABLE_REVISION: u32 = (MM_MMST_REVISION_MAJOR << 16) | MM_MMST_REVISION_MINOR;

//
// This EFI_MM_CPU_IO_PROTOCOL is embedded in MMST, so we need to define it here to be able to parse the MMST correctly.
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
/// ```c
/// typedef struct {
///   ///
///   /// This service provides the various modalities of memory and I/O read.
///   ///
///   EFI_MM_CPU_IO    Read;
///   ///
///   /// This service provides the various modalities of memory and I/O write.
///   ///
///   EFI_MM_CPU_IO    Write;
/// } EFI_MM_IO_ACCESS;
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MmCpuIoAccess {
    /// This service provides the various modalities of memory and I/O read.
    pub read: MmCpuIoFn,
    /// This service provides the various modalities of memory and I/O write.
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
    /// MMIO access pair (Read + Write).
    pub mem: MmCpuIoAccess,
    /// I/O port access functions.
    pub io: MmCpuIoAccess,
}

/// Adds, updates, or removes a configuration table entry from the Management Mode System Table.
///
/// This function matches the C typedef `EFI_MM_INSTALL_CONFIGURATION_TABLE`:
/// ```c
/// typedef
/// EFI_STATUS
/// (EFIAPI *EFI_MM_INSTALL_CONFIGURATION_TABLE)(
///   IN CONST EFI_MM_SYSTEM_TABLE    *SystemTable,
///   IN CONST EFI_GUID               *Guid,
///   IN VOID                         *Table,
///   IN UINTN                        TableSize
///   );
/// ```
pub type MmInstallConfigurationTableFn = unsafe extern "efiapi" fn(
    system_table: *const EfiMmSystemTable,
    guid: *const efi::Guid,
    table: *mut c_void,
    table_size: usize,
) -> efi::Status;

/// Allocates pool memory from the specified memory type.
///
/// This function matches the C typedef `EFI_MM_ALLOCATE_POOL`, which is already defined in `r_efi::efi` as `BootAllocatePool`.
pub type MmAllocatePoolFn = BootAllocatePool;

/// Frees pool memory.
///
/// This function matches the C typedef `EFI_MM_FREE_POOL`, which is already defined in `r_efi::efi` as `BootFreePool`.
pub type MmFreePoolFn = BootFreePool;

/// Allocates memory pages from the system.
///
/// This function matches the C typedef `EFI_ALLOCATE_PAGES`, which is already defined in `r_efi::efi` as `BootAllocatePages`.
pub type MmAllocatePagesFn = BootAllocatePages;

/// Frees memory pages.
///
/// This function matches the C typedef `EFI_FREE_PAGES`, which is already defined in `r_efi::efi` as `BootFreePages`.
pub type MmFreePagesFn = BootFreePages;

/// `EFI_MM_STARTUP_THIS_AP`
pub type MmStartupThisApFn =
    unsafe extern "efiapi" fn(procedure: usize, cpu_number: usize, proc_arguments: *mut c_void) -> efi::Status;

/// Installs a protocol interface on a device handle.
///
/// This function matches the C typedef `EFI_INSTALL_PROTOCOL_INTERFACE`, which is already defined in `r_efi::efi` as `BootInstallProtocolInterface`.
pub type MmInstallProtocolInterfaceFn = BootInstallProtocolInterface;

/// Removes a protocol interface from a device handle.
///
/// This function matches the C typedef `EFI_UNINSTALL_PROTOCOL_INTERFACE`, which is already defined in `r_efi::efi` as `BootUninstallProtocolInterface`.
pub type MmUninstallProtocolInterfaceFn = BootUninstallProtocolInterface;

/// Queries a handle to determine if it supports a specified protocol.
///
/// This function matches the C typedef `EFI_HANDLE_PROTOCOL`, which is already defined in `r_efi::efi` as `BootHandleProtocol`.
pub type MmHandleProtocolFn = BootHandleProtocol;

/// Register a callback function be called when a particular protocol interface is installed.
///
/// This function matches the C typedef `EFI_MM_REGISTER_PROTOCOL_NOTIFY`:
/// ```c
/// typedef
/// EFI_STATUS
/// (EFIAPI *EFI_MM_REGISTER_PROTOCOL_NOTIFY)(
///   IN  CONST EFI_GUID     *Protocol,
///   IN  EFI_MM_NOTIFY_FN   Function,
///   OUT VOID               **Registration
///   );
/// ```
pub type MmRegisterProtocolNotifyFn = unsafe extern "efiapi" fn(
    protocol: *const efi::Guid,
    function: usize,
    registration: *mut *mut c_void,
) -> efi::Status;

/// Returns an array of handles that support a specified protocol.
///
/// This function matches the C typedef `EFI_LOCATE_HANDLE`, which is already defined in `r_efi::efi` as `BootLocateHandle`.
pub type MmLocateHandleFn = BootLocateHandle;

/// Returns the first protocol instance that matches the given protocol.
///
/// This function matches the C typedef `EFI_LOCATE_PROTOCOL`, which is already defined in `r_efi::efi` as `BootLocateProtocol`.
pub type MmLocateProtocolFn = BootLocateProtocol;

/// Manage MMI of a particular type.
///
/// This function matches the C typedef `EFI_MM_INTERRUPT_MANAGE`:
/// ```c
/// typedef
/// EFI_STATUS
/// (EFIAPI *EFI_MM_INTERRUPT_MANAGE)(
///   IN CONST EFI_GUID  *HandlerType,
///   IN CONST VOID      *Context         OPTIONAL,
///   IN OUT VOID        *CommBuffer      OPTIONAL,
///   IN OUT UINTN       *CommBufferSize  OPTIONAL
///   );
/// ```
pub type MmiManageFn = unsafe extern "efiapi" fn(
    handler_type: *const efi::Guid,
    context: *const c_void,
    comm_buffer: *mut c_void,
    comm_buffer_size: *mut usize,
) -> efi::Status;

/// Main entry point for an MM handler dispatch or communicate-based callback.
///
/// This function matches the C typedef `EFI_MM_HANDLER_ENTRY_POINT`:
/// ```c
/// typedef
/// EFI_STATUS
/// (EFIAPI *EFI_MM_HANDLER_ENTRY_POINT)(
///   IN EFI_HANDLE  DispatchHandle,
///   IN CONST VOID  *Context         OPTIONAL,
///   IN OUT VOID    *CommBuffer      OPTIONAL,
///   IN OUT UINTN   *CommBufferSize  OPTIONAL
///   );
/// ```
pub type MmiHandlerEntryPoint = unsafe extern "efiapi" fn(
    dispatch_handle: efi::Handle,
    context: *const c_void,
    comm_buffer: *mut c_void,
    comm_buffer_size: *mut usize,
) -> efi::Status;

/// Registers a handler entry point for a particular MMI handler type.
///
/// This function matches the C typedef `EFI_MM_INTERRUPT_REGISTER`:
/// ```c
/// typedef
/// EFI_STATUS
/// (EFIAPI *EFI_MM_INTERRUPT_REGISTER)(
///   IN  EFI_MM_HANDLER_ENTRY_POINT    Handler,
///   IN  CONST EFI_GUID                *HandlerType OPTIONAL,
///   OUT EFI_HANDLE                    *DispatchHandle
///   );
/// ```
pub type MmiHandlerRegisterFn = unsafe extern "efiapi" fn(
    handler: MmiHandlerEntryPoint,
    handler_type: *const efi::Guid,
    dispatch_handle: *mut efi::Handle,
) -> efi::Status;

/// Unregister a handler in MM.
///
/// This function matches the C typedef `EFI_MM_INTERRUPT_UNREGISTER`:
/// ```c
/// typedef
/// EFI_STATUS
/// (EFIAPI *EFI_MM_INTERRUPT_UNREGISTER)(
///   IN  EFI_HANDLE  DispatchHandle
///   );
/// ```
pub type MmiHandlerUnregisterFn = unsafe extern "efiapi" fn(dispatch_handle: efi::Handle) -> efi::Status;

/// EFI_MM_ENTRY_CONTEXT structure.
///
/// Processor information and functionality needed by MM Foundation.
/// Matches the C `EFI_MM_ENTRY_CONTEXT` from PI specification.
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
/// This is the `#[repr(C)]` Rust definition of the C `EFI_MM_SYSTEM_TABLE` structure
/// from `PiMmCis.h`. The table pointer is passed as the second argument to
/// every MM driver's entry point:
///
/// ```c
/// EFI_STATUS EFIAPI DriverEntry(EFI_HANDLE ImageHandle, EFI_MM_SYSTEM_TABLE *MmSt);
/// ```
#[repr(C)]
pub struct EfiMmSystemTable {
    ///
    /// The table header for the SMST.
    ///
    pub hdr: efi::TableHeader,

    ///
    /// A pointer to a NULL-terminated Unicode string containing the vendor name.
    /// It is permissible for this pointer to be NULL.
    ///
    pub mm_firmware_vendor: *mut u16,
    ///
    /// The particular revision of the firmware.
    ///
    pub mm_firmware_revision: u32,

    /// Function to add, update, or remove a configuration table entry from the MMST.
    pub mm_install_configuration_table: MmInstallConfigurationTableFn,

    ///
    /// I/O Service
    ///
    pub mm_io: MmCpuIoProtocol,

    ///
    /// Runtime memory services
    ///
    /// This function matches the C typedef `EFI_MM_ALLOCATE_POOL`, which allocates pool memory from the specified memory type.
    pub mm_allocate_pool: MmAllocatePoolFn,
    /// This function matches the C typedef `EFI_MM_FREE_POOL`, which frees pool memory.
    pub mm_free_pool: MmFreePoolFn,
    /// This function matches the C typedef `EFI_ALLOCATE_PAGES`, which allocates memory pages from the system.
    pub mm_allocate_pages: MmAllocatePagesFn,
    /// This function matches the C typedef `EFI_FREE_PAGES`, which frees memory pages.
    pub mm_free_pages: MmFreePagesFn,

    ///
    /// MP service
    ///
    pub mm_startup_this_ap: MmStartupThisApFn,

    ///
    /// CPU information records
    ///
    /// A number between zero and and the NumberOfCpus field. This field designates
    /// which processor is executing the MM infrastructure.
    ///
    pub currently_executing_cpu: usize,
    ///
    /// The number of possible processors in the platform.  This is a 1 based counter.
    ///
    pub number_of_cpus: usize,
    ///
    /// Points to an array, where each element describes the number of bytes in the
    /// corresponding save state specified by CpuSaveState. There are always
    /// NumberOfCpus entries in the array.
    ///
    pub cpu_save_state_size: *mut usize,
    ///
    /// Points to an array, where each element is a pointer to a CPU save state. The
    /// corresponding element in CpuSaveStateSize specifies the number of bytes in the
    /// save state area. There are always NumberOfCpus entries in the array.
    ///
    pub cpu_save_state: *mut *mut c_void,

    ///
    /// Extensibility table
    ///
    ///
    /// The number of UEFI Configuration Tables in the buffer MmConfigurationTable.
    ///
    pub number_of_table_entries: usize,
    ///
    /// A pointer to the UEFI Configuration Tables. The number of entries in the table is
    /// NumberOfTableEntries.
    ///
    pub mm_configuration_table: *mut efi::ConfigurationTable,

    ///
    /// Protocol services
    ///
    ///
    /// This function matches the C typedef `EFI_INSTALL_PROTOCOL_INTERFACE`, which installs a protocol interface on a device handle.
    pub mm_install_protocol_interface: MmInstallProtocolInterfaceFn,
    /// This function matches the C typedef `EFI_UNINSTALL_PROTOCOL_INTERFACE`, which removes a protocol interface from a device handle.
    pub mm_uninstall_protocol_interface: MmUninstallProtocolInterfaceFn,
    /// This function matches the C typedef `EFI_HANDLE_PROTOCOL`, which queries a handle to determine if it supports a specified protocol.
    pub mm_handle_protocol: MmHandleProtocolFn,
    /// This function matches the C typedef `EFI_MM_REGISTER_PROTOCOL_NOTIFY`, which registers a callback function be called when a particular protocol interface is installed.
    pub mm_register_protocol_notify: MmRegisterProtocolNotifyFn,
    /// This function matches the C typedef `EFI_LOCATE_HANDLE`, which returns an array of handles that support a specified protocol.
    pub mm_locate_handle: MmLocateHandleFn,
    /// This function matches the C typedef `EFI_LOCATE_PROTOCOL`, which returns the first protocol instance that matches the given protocol.
    pub mm_locate_protocol: MmLocateProtocolFn,

    ///
    /// MMI Management functions
    ///
    /// This function matches the C typedef `EFI_MM_INTERRUPT_MANAGE`, which manages MMI of a particular type.
    pub mmi_manage: MmiManageFn,
    /// This function matches the C typedef `EFI_MM_HANDLER_REGISTER`, which registers a handler entry point for a particular MMI handler type.
    pub mmi_handler_register: MmiHandlerRegisterFn,
    /// This function matches the C typedef `EFI_MM_INTERRUPT_UNREGISTER`, which unregisters a handler in MM.
    pub mmi_handler_unregister: MmiHandlerUnregisterFn,
}
