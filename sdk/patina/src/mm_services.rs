//! MM (Management Mode) Services type definitions and trait.
//!
//! This module provides the Rust definitions for the PI `EFI_MM_SYSTEM_TABLE`
//! and an `MmServices` trait that wraps the raw C function-pointer table with
//! safe Rust method signatures, following the same pattern as
//! [`boot_services::BootServices`](crate::boot_services::BootServices).
//!
//! ## Layout
//!
//! * [`EfiMmSystemTable`] — `#[repr(C)]` struct matching the C
//!   `_EFI_MM_SYSTEM_TABLE` layout from `PiMmCis.h`.
//! * [`MmServices`] — Safe Rust trait exposing the system-table services.
//! * [`StandardMmServices`] — Concrete wrapper around `*mut EfiMmSystemTable`
//!   that implements `MmServices` by calling through the function pointers.
//!
//! Cores (e.g., `patina_mm_user_core`) allocate an `EfiMmSystemTable`, populate
//! its function pointers with their own `extern "efiapi"` thunks, and hand the
//! raw pointer to dispatched MM drivers. Drivers that want safe access can wrap
//! it in a `StandardMmServices`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;

use r_efi::efi;
use spin::Once;

/// MMST signature: `'S', 'M', 'S', 'T'` (same as C `MM_MMST_SIGNATURE`).
pub const MM_MMST_SIGNATURE: u64 = 0x5453_4D53;

/// PI Specification version encoded as `(major << 16) | minor`.
/// PI 1.8 → `0x0001_0050`.
pub const MM_SYSTEM_TABLE_REVISION: u32 = (1 << 16) | 80;

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

// SAFETY: The system table is allocated once and its pointer is shared read-only
// with dispatched drivers. Internal mutation goes through synchronized databases.
unsafe impl Send for EfiMmSystemTable {}
unsafe impl Sync for EfiMmSystemTable {}

/// Wrapper around a raw `*mut EfiMmSystemTable` pointer that implements
/// [`MmServices`] by calling through the C function-pointer table.
///
/// This is the MM equivalent of
/// [`StandardBootServices`](crate::boot_services::StandardBootServices).
pub struct StandardMmServices {
    efi_mm_system_table: Once<*mut EfiMmSystemTable>,
}

// SAFETY: The raw pointer is only written once (protected by `Once`) and the
// underlying table is not expected to change after initialisation.
unsafe impl Sync for StandardMmServices {}
unsafe impl Send for StandardMmServices {}

impl StandardMmServices {
    /// Create a new `StandardMmServices` from an existing system table pointer.
    pub fn new(mm_system_table: *mut EfiMmSystemTable) -> Self {
        let this = Self::new_uninit();
        this.init(mm_system_table);
        this
    }

    /// Create an uninitialised instance.
    pub const fn new_uninit() -> Self {
        Self { efi_mm_system_table: Once::new() }
    }

    /// Initialise with the given system table pointer.
    pub fn init(&self, mm_system_table: *mut EfiMmSystemTable) {
        self.efi_mm_system_table.call_once(|| mm_system_table);
    }

    /// Returns `true` if the instance has been initialised.
    pub fn is_init(&self) -> bool {
        self.efi_mm_system_table.is_completed()
    }

    /// Returns the raw system table pointer (panics if uninitialised).
    pub fn as_mut_ptr(&self) -> *mut EfiMmSystemTable {
        *self.efi_mm_system_table.get().expect("StandardMmServices is not initialized!")
    }
}

impl Clone for StandardMmServices {
    fn clone(&self) -> Self {
        if let Some(ptr) = self.efi_mm_system_table.get() {
            StandardMmServices::new(*ptr)
        } else {
            StandardMmServices::new_uninit()
        }
    }
}

impl core::fmt::Debug for StandardMmServices {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if !self.is_init() {
            return f.debug_struct("StandardMmServices").field("table", &"Not Initialized").finish();
        }
        f.debug_struct("StandardMmServices").field("table", &self.as_mut_ptr()).finish()
    }
}

/// Safe Rust interface to the MM System Table services.
///
/// This is the MM analogue of
/// [`BootServices`](crate::boot_services::BootServices).
/// Each method maps 1:1 to a function pointer in [`EfiMmSystemTable`].
pub trait MmServices {
    /// Allocate pool memory.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmAllocatePool`
    fn allocate_pool(&self, pool_type: efi::MemoryType, size: usize) -> Result<*mut u8, efi::Status>;

    /// Free pool memory.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmFreePool`
    fn free_pool(&self, buffer: *mut u8) -> Result<(), efi::Status>;

    /// Allocate pages.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmAllocatePages`
    fn allocate_pages(
        &self,
        alloc_type: efi::AllocateType,
        memory_type: efi::MemoryType,
        pages: usize,
    ) -> Result<u64, efi::Status>;

    /// Free pages.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmFreePages`
    fn free_pages(&self, memory: u64, pages: usize) -> Result<(), efi::Status>;

    /// Install a protocol interface on a handle.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmInstallProtocolInterface`
    ///
    /// # Safety
    ///
    /// `interface` must be a valid pointer to the protocol structure or null.
    unsafe fn install_protocol_interface(
        &self,
        handle: *mut efi::Handle,
        protocol: &efi::Guid,
        interface_type: efi::InterfaceType,
        interface: *mut c_void,
    ) -> Result<(), efi::Status>;

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
    ) -> Result<(), efi::Status>;

    /// Query a handle for a protocol.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmHandleProtocol`
    ///
    /// # Safety
    ///
    /// The returned pointer must be used carefully to avoid aliasing violations.
    unsafe fn handle_protocol(
        &self,
        handle: efi::Handle,
        protocol: &efi::Guid,
    ) -> Result<*mut c_void, efi::Status>;

    /// Locate the first device that supports a protocol.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmLocateProtocol`
    ///
    /// # Safety
    ///
    /// The returned pointer must be used carefully to avoid aliasing violations.
    unsafe fn locate_protocol(
        &self,
        protocol: &efi::Guid,
    ) -> Result<*mut c_void, efi::Status>;

    /// Manage (dispatch) an MMI.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmiManage`
    fn mmi_manage(
        &self,
        handler_type: Option<&efi::Guid>,
        context: *const c_void,
        comm_buffer: *mut c_void,
        comm_buffer_size: *mut usize,
    ) -> efi::Status;

    /// Register an MMI handler.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmiHandlerRegister`
    fn mmi_handler_register(
        &self,
        handler: MmiHandlerEntryPoint,
        handler_type: Option<&efi::Guid>,
    ) -> Result<efi::Handle, efi::Status>;

    /// Unregister an MMI handler.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmiHandlerUnRegister`
    fn mmi_handler_unregister(
        &self,
        dispatch_handle: efi::Handle,
    ) -> Result<(), efi::Status>;
}

impl MmServices for StandardMmServices {
    fn allocate_pool(&self, pool_type: efi::MemoryType, size: usize) -> Result<*mut u8, efi::Status> {
        let mmst = unsafe { &*self.as_mut_ptr() };
        let mut buffer: *mut c_void = core::ptr::null_mut();
        let status = unsafe { (mmst.mm_allocate_pool)(pool_type, size, &mut buffer) };
        if status == efi::Status::SUCCESS {
            Ok(buffer as *mut u8)
        } else {
            Err(status)
        }
    }

    fn free_pool(&self, buffer: *mut u8) -> Result<(), efi::Status> {
        let mmst = unsafe { &*self.as_mut_ptr() };
        let status = unsafe { (mmst.mm_free_pool)(buffer as *mut c_void) };
        if status == efi::Status::SUCCESS { Ok(()) } else { Err(status) }
    }

    fn allocate_pages(
        &self,
        alloc_type: efi::AllocateType,
        memory_type: efi::MemoryType,
        pages: usize,
    ) -> Result<u64, efi::Status> {
        let mmst = unsafe { &*self.as_mut_ptr() };
        let mut memory: efi::PhysicalAddress = 0;
        let status = unsafe { (mmst.mm_allocate_pages)(alloc_type, memory_type, pages, &mut memory) };
        if status == efi::Status::SUCCESS { Ok(memory) } else { Err(status) }
    }

    fn free_pages(&self, memory: u64, pages: usize) -> Result<(), efi::Status> {
        let mmst = unsafe { &*self.as_mut_ptr() };
        let status = unsafe { (mmst.mm_free_pages)(memory, pages) };
        if status == efi::Status::SUCCESS { Ok(()) } else { Err(status) }
    }

    unsafe fn install_protocol_interface(
        &self,
        handle: *mut efi::Handle,
        protocol: &efi::Guid,
        interface_type: efi::InterfaceType,
        interface: *mut c_void,
    ) -> Result<(), efi::Status> {
        let mmst = unsafe { &*self.as_mut_ptr() };
        let status = unsafe {
            (mmst.mm_install_protocol_interface)(
                handle,
                protocol as *const efi::Guid as *mut efi::Guid,
                interface_type,
                interface,
            )
        };
        if status == efi::Status::SUCCESS { Ok(()) } else { Err(status) }
    }

    unsafe fn uninstall_protocol_interface(
        &self,
        handle: efi::Handle,
        protocol: &efi::Guid,
        interface: *mut c_void,
    ) -> Result<(), efi::Status> {
        let mmst = unsafe { &*self.as_mut_ptr() };
        let status = unsafe {
            (mmst.mm_uninstall_protocol_interface)(
                handle,
                protocol as *const efi::Guid as *mut efi::Guid,
                interface,
            )
        };
        if status == efi::Status::SUCCESS { Ok(()) } else { Err(status) }
    }

    unsafe fn handle_protocol(
        &self,
        handle: efi::Handle,
        protocol: &efi::Guid,
    ) -> Result<*mut c_void, efi::Status> {
        let mmst = unsafe { &*self.as_mut_ptr() };
        let mut interface: *mut c_void = core::ptr::null_mut();
        let status = unsafe {
            (mmst.mm_handle_protocol)(
                handle,
                protocol as *const efi::Guid as *mut efi::Guid,
                &mut interface,
            )
        };
        if status == efi::Status::SUCCESS { Ok(interface) } else { Err(status) }
    }

    unsafe fn locate_protocol(
        &self,
        protocol: &efi::Guid,
    ) -> Result<*mut c_void, efi::Status> {
        let mmst = unsafe { &*self.as_mut_ptr() };
        let mut interface: *mut c_void = core::ptr::null_mut();
        let status = unsafe {
            (mmst.mm_locate_protocol)(
                protocol as *const efi::Guid as *mut efi::Guid,
                core::ptr::null_mut(),
                &mut interface,
            )
        };
        if status == efi::Status::SUCCESS { Ok(interface) } else { Err(status) }
    }

    fn mmi_manage(
        &self,
        handler_type: Option<&efi::Guid>,
        context: *const c_void,
        comm_buffer: *mut c_void,
        comm_buffer_size: *mut usize,
    ) -> efi::Status {
        let mmst = unsafe { &*self.as_mut_ptr() };
        let guid_ptr = handler_type.map_or(core::ptr::null(), |g| g as *const efi::Guid);
        unsafe { (mmst.mmi_manage)(guid_ptr, context, comm_buffer, comm_buffer_size) }
    }

    fn mmi_handler_register(
        &self,
        handler: MmiHandlerEntryPoint,
        handler_type: Option<&efi::Guid>,
    ) -> Result<efi::Handle, efi::Status> {
        let mmst = unsafe { &*self.as_mut_ptr() };
        let guid_ptr = handler_type.map_or(core::ptr::null(), |g| g as *const efi::Guid);
        let mut dispatch_handle: efi::Handle = core::ptr::null_mut();
        let status = unsafe { (mmst.mmi_handler_register)(handler, guid_ptr, &mut dispatch_handle) };
        if status == efi::Status::SUCCESS { Ok(dispatch_handle) } else { Err(status) }
    }

    fn mmi_handler_unregister(
        &self,
        dispatch_handle: efi::Handle,
    ) -> Result<(), efi::Status> {
        let mmst = unsafe { &*self.as_mut_ptr() };
        let status = unsafe { (mmst.mmi_handler_unregister)(dispatch_handle) };
        if status == efi::Status::SUCCESS { Ok(()) } else { Err(status) }
    }
}
