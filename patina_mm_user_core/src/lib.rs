//! MM User Core
//!
//! A pure Rust implementation of the MM User Core for standalone MM mode environments.
//!
//! This crate provides the core functionality for a user-mode (Ring 3) MM module that is
//! invoked by the MM Supervisor Core via privilege demotion. It implements the equivalent
//! functionality of the C `StandaloneMmCore` — discovering drivers from HOBs, evaluating
//! dependency expressions, dispatching drivers, and managing MMI handlers.
//!
//! ## Architecture
//!
//! The user core is invoked by the supervisor with three command types:
//! - **StartUserCore**: One-time initialization. Walk HOBs to discover drivers and dispatch them.
//! - **UserRequest**: Runtime MMI dispatch. Parse the communication buffer and invoke registered handlers.
//! - **UserApProcedure**: Execute a procedure on behalf of an AP.
//!
//! ## Entry Protocol
//!
//! The supervisor calls the user core entry point with three arguments:
//! - `arg1` (`u64`): Command type (0 = StartUserCore, 1 = UserRequest, 2 = UserApProcedure)
//! - `arg2` (`u64`): Command-specific data pointer
//! - `arg3` (`u64`): Command-specific size or auxiliary data
//!
//! ## Memory Model
//!
//! This crate runs in Ring 3 (user mode). It does not have direct access to supervisor
//! resources. All supervisor services are accessed through syscalls.
//!
//! ## Example
//!
//! ```rust,ignore
//! use patina_mm_user_core::*;
//!
//! static USER_CORE: MmUserCore = MmUserCore::new();
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![cfg(target_arch = "x86_64")]

extern crate alloc;

pub mod component_dispatcher;
pub mod config_table;
pub mod core_handlers;
pub mod mm_dispatcher;
pub mod mm_mem;
pub mod mm_services;
pub mod mmi;
pub mod pool_allocator;
pub mod protocol_db;

use core::{
    ffi::c_void,
    mem,
    num::NonZeroUsize,
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use alloc::boxed::Box;
use patina::{
    mm_services::MmServices,
    pi::hob::{Hob, PhaseHandoffInformationTable},
};
use r_efi::efi;
use spin::{Mutex, Once};

use crate::{
    component_dispatcher::{MmComponentDispatcher, MmComponentInfo},
    config_table::MmConfigurationTableDb,
    mm_dispatcher::MmDispatcher,
    mmi::MmiDatabase,
    protocol_db::ProtocolDatabase,
};

use patina::{
    management_mode::{
        MmCommBufferStatus,
        comm_buffer_hob::{MM_COMM_BUFFER_HOB_GUID, MmCommonBufferHobData},
        supervisor::UserCommandType,
    },
    pi::{
        mm_cis::{EfiMmEntryContext, EfiMmSystemTable},
        protocols::communication::EfiMmCommunicateHeader,
    },
};
use zerocopy::FromBytes;

// The entry-point shim references `user_core_main`, which is provided by the platform binary, and
// is only meaningful on the firmware (UEFI) target. Exclude it from host builds (tests, doctests)
// so their harnesses can link.
#[cfg(target_os = "uefi")]
core::arch::global_asm!(include_str!("entry_point.asm"));

/// GUID identifying the MM Supervisor Core module (to be skipped during driver discovery).
///
/// `gMmSupervisorCoreGuid`
pub const MM_SUPERVISOR_CORE_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("4e4c89dc-a452-4b6b-b183-f16a2a223733");

/// GUID for depex data HOBs paired with driver `MemoryAllocationModule` HOBs.
///
/// `gMmSupervisorDepexHobGuid`
pub const MM_SUPERVISOR_DEPEX_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("b17f0049-affd-4530-acd6-e245e19deaf1");

/// Mirrors the MM_SUPV_DEPEX_HOB_DATA structure defined in the supervisor.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DepexHobData {
    /// Protocol GUID the dependency expression applies to.
    pub name: patina::BinaryGuid,
    /// Size in bytes of the dependency expression that follows.
    pub depex_expression_size: u64,
    /// Variable-length dependency expression bytes (flexible array member).
    pub depex_expression: [u8; 0],
}

/// Base address of the user communication buffer (discovered from HOBs).
///
/// The supervisor rewrites the HOB's `physical_start` to point to the internal
/// (MMRAM-resident, user-accessible) copy of the communication buffer before
/// invoking `StartUserCore`.
static COMM_BUFFER_BASE: AtomicU64 = AtomicU64::new(0);

/// Size in bytes of the user communication buffer.
static COMM_BUFFER_SIZE: AtomicU64 = AtomicU64::new(0);

/// Static reference to the user core instance.
static __USER_CORE: Once<NonZeroUsize> = Once::new();

/// Useful for offline inspection (like debugging) to determine core version.
#[used]
static MM_USER_CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The MM User Core responsible for driver dispatch and MMI handling in user mode.
///
/// Create a static instance and call [`entry_point_worker`](MmUserCore::entry_point_worker)
/// from the binary entry point.
///
/// ## Examples
///
/// ```rust,ignore
/// use patina_mm_user_core::component_dispatcher::{Add, Component, MmComponentInfo};
///
/// static USER_CORE: MmUserCore = MmUserCore::new();
///
/// // The platform provides its components via an `MmComponentInfo` type.
/// struct MyMmPlatform;
/// impl MmComponentInfo for MyMmPlatform {
///     fn components(mut add: Add<Component>) {
///         // add.component(...);
///     }
/// }
///
/// #[unsafe(export_name = "user_core_main")]
/// pub extern "efiapi" fn _start(op_code: u64, arg1: u64, arg2: u64) -> u64 {
///     USER_CORE.entry_point_worker::<MyMmPlatform>(op_code, arg1, arg2)
/// }
/// ```
pub struct MmUserCore {
    /// The MMI handler database.
    pub mmi_db: MmiDatabase,
    /// The protocol/handle database (for depex evaluation and driver services).
    pub protocol_db: ProtocolDatabase,
    /// The configuration-table database backing `MmInstallConfigurationTable`.
    pub config_table_db: MmConfigurationTableDb,
    /// The driver dispatcher.
    pub dispatcher: MmDispatcher,
    /// The Patina component dispatcher (dependency-injected `#[component]` entry points).
    pub component_dispatcher: Mutex<MmComponentDispatcher>,
    /// Address of the heap-allocated MM System Table, set once it is built.
    mm_system_table: Once<usize>,
    /// Whether the core has completed initialization.
    initialized: AtomicBool,
}

impl Default for MmUserCore {
    fn default() -> Self {
        Self::new()
    }
}

impl MmUserCore {
    /// Creates a new instance of the MM User Core.
    pub const fn new() -> Self {
        Self {
            mmi_db: MmiDatabase::new(),
            protocol_db: ProtocolDatabase::new(),
            config_table_db: MmConfigurationTableDb::new(),
            dispatcher: MmDispatcher::new(),
            component_dispatcher: Mutex::new(MmComponentDispatcher::new()),
            mm_system_table: Once::new(),
            initialized: AtomicBool::new(false),
        }
    }

    /// Sets the static user core instance for global access.
    ///
    /// Returns true if the address was successfully stored, false if already set.
    #[must_use]
    fn set_instance(&'static self) -> bool {
        let physical_address = NonNull::from_ref(self).expose_provenance();
        &physical_address == __USER_CORE.call_once(|| physical_address)
    }

    /// Gets the static MM User Core instance for global access.
    pub fn instance<'a>() -> &'a Self {
        // SAFETY: The pointer is guaranteed to be valid as set_instance ensures single initialization.
        unsafe {
            NonNull::<Self>::with_exposed_provenance(*__USER_CORE.get().expect("MM User Core is not initialized."))
                .as_ref()
        }
    }

    /// Gets the static MM User Core instance if it has been initialized.
    ///
    /// Unlike [`instance`](Self::instance), this returns `None` instead of
    /// panicking when the instance has not yet been set via `set_instance`.
    pub fn try_instance<'a>() -> Option<&'a Self> {
        // SAFETY: The pointer, if present, was stored by `set_instance` from a
        // `&'static Self` and remains valid for the lifetime of the program.
        __USER_CORE.get().map(|&addr| unsafe { NonNull::<Self>::with_exposed_provenance(addr).as_ref() })
    }

    /// Build (once) and return the heap-allocated MM System Table.
    ///
    /// The table's function pointers are thin thunks that forward to this
    /// instance's databases (see [`crate::mm_services`]). Must be called after
    /// [`set_instance`](Self::set_instance) and after the heap is available.
    fn init_mm_system_table(&'static self) -> *mut EfiMmSystemTable {
        let addr = *self.mm_system_table.call_once(|| {
            let ptr = Box::into_raw(Box::new(mm_services::build_mm_system_table()));
            log::info!("MM System Table allocated at {:p}", ptr);
            ptr.expose_provenance()
        });
        core::ptr::with_exposed_provenance_mut(addr)
    }

    /// Returns the MM System Table pointer, or null if it has not been built yet.
    pub(crate) fn mm_system_table_ptr(&self) -> *mut EfiMmSystemTable {
        self.mm_system_table.get().map_or(core::ptr::null_mut(), |&addr| core::ptr::with_exposed_provenance_mut(addr))
    }

    /// Reflect the current processor state into the MM System Table.
    ///
    /// Called at the start of each `UserRequest` so dispatched drivers observe
    /// the CPU that is executing the MM foundation.
    fn update_cpu_context(&self, currently_executing_cpu: usize, number_of_cpus: usize) {
        let ptr = self.mm_system_table_ptr();
        if ptr.is_null() {
            return;
        }
        // SAFETY: The table is heap-allocated, lives for the lifetime of the core, and these two
        // scalar fields are only written here on the BSP — there is no concurrent writer.
        unsafe {
            (*ptr).currently_executing_cpu = currently_executing_cpu;
            (*ptr).number_of_cpus = number_of_cpus;
        }
    }

    /// Main entry point for the MM User Core.
    ///
    /// This is called by the supervisor via `invoke_demoted_routine`. The arguments
    /// correspond to the three parameters passed by the supervisor:
    ///
    /// - `arg1`: Command type ([`UserCommandType`])
    /// - `arg2`: Command-specific data pointer
    /// - `arg3`: Command-specific size or auxiliary data
    ///
    /// Returns 0 on success, or a non-zero status on failure.
    pub fn entry_point_worker<C: MmComponentInfo>(&'static self, op_code: u64, arg1: u64, arg2: u64) -> u64 {
        let command = match UserCommandType::try_from(op_code) {
            Ok(cmd) => cmd,
            Err(unknown) => {
                log::error!("Unknown command type: {}", unknown);
                return efi::Status::INVALID_PARAMETER.as_usize() as u64;
            }
        };

        match command {
            UserCommandType::StartUserCore => self.handle_start_user_core::<C>(arg1 as *const c_void),
            UserCommandType::UserRequest => self.handle_user_request(arg1, arg2),
            UserCommandType::UserApProcedure => self.handle_user_ap_procedure(arg1, arg2),
        }
    }

    /// Handle the `StartUserCore` command.
    ///
    /// This is called once during initialization. The supervisor passes the HOB list
    /// pointer as `arg2`. We:
    /// 1. Set the static instance
    /// 2. Walk HOBs to discover the communication buffer and MM drivers
    /// 3. Build the MM System Table and publish the HOB list configuration table
    /// 4. Register the core MMI handlers (driver dispatch is deferred to the
    ///    `MM_DISPATCH_EVENT` handler, see [`dispatch_drivers`](Self::dispatch_drivers))
    fn handle_start_user_core<C: MmComponentInfo>(&'static self, hob_list: *const c_void) -> u64 {
        if !self.set_instance() {
            log::warn!("MM User Core instance was already set, skipping re-initialization.");
            return efi::Status::ALREADY_STARTED.as_usize() as u64;
        }

        // Register this instance as the MmServices provider that the EfiMmSystemTable
        // thunks forward into. From here, every table call lands in our native impl.
        mm_services::init_mm_services(self);

        if hob_list.is_null() {
            log::error!("HOB list pointer is null.");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        log::info!("MM User Core v{} starting initialization...", env!("CARGO_PKG_VERSION"));

        // Enable the heap (syscall page allocator) before doing anything that
        // requires dynamic allocation (driver discovery, depex parsing, etc.).
        mm_mem::SYSCALL_PAGE_ALLOCATOR.set_initialized();

        // Parse the HOB list
        let hob_list_info = unsafe {
            match (hob_list as *const PhaseHandoffInformationTable).as_ref() {
                Some(info) => info,
                None => {
                    log::error!("Failed to read HOB list header.");
                    return efi::Status::INVALID_PARAMETER.as_usize() as u64;
                }
            }
        };

        let hob = Hob::Handoff(hob_list_info);

        // Discover communication buffer from HOBs
        self.discover_comm_buffer(&hob);

        // Discover MM drivers from HOBs now, while the HOB list is available. The
        // actual dispatch is deferred to the `MM_DISPATCH_EVENT` handler.
        self.dispatcher.discover(&hob);

        // Initialize the MM System Table (heap-allocated, function pointers
        // are thunks that forward to this instance's databases).
        let mm_system_table = self.init_mm_system_table();
        log::info!("MM System Table initialized at {:p}", mm_system_table);

        // Publish the HOB list as a configuration table entry so dispatched
        // drivers can locate it via the system table (mirrors the C
        // `MmInstallConfigurationTable(&gMmCoreMmst, &gEfiHobListGuid, ...)`
        // call in `InitializeMmHobList`).
        // SAFETY: `hob_list` points to the supervisor-provided HOB list and remains valid for the
        // lifetime of the configuration-table entry.
        if let Err(status) =
            unsafe { self.install_configuration_table(&patina::guids::HOB_LIST, hob_list as *mut c_void, 0) }
        {
            log::error!("Failed to install HOB list configuration table: {:?}", status);
        }

        // Register core MMI handlers (lifecycle events like ready-to-lock,
        // end-of-DXE, exit-boot-services, etc.). Driver dispatch is deferred to
        // the `MM_DISPATCH_EVENT` handler, which the supervisor forwards once the
        // MM foundation is ready.
        core_handlers::register_core_mmi_handlers();

        // Register and dispatch platform-provided Patina components.
        self.dispatch_components::<C>(&hob);

        self.initialized.store(true, Ordering::Release);
        log::info!("MM User Core initialization complete.");

        efi::Status::SUCCESS.as_usize() as u64
    }

    /// Dispatch the MM drivers discovered during `StartUserCore` in dependency order.
    ///
    /// Discovery happens eagerly in [`handle_start_user_core`](Self::handle_start_user_core);
    /// the dispatch itself is deferred and driven by the `MM_DISPATCH_EVENT` handler
    /// (see [`mm_driver_dispatch_handler`]).
    ///
    /// [`mm_driver_dispatch_handler`]: crate::core_handlers
    pub(crate) fn dispatch_drivers(&self) -> Result<usize, efi::Status> {
        self.dispatcher.dispatch(&self.protocol_db, self.mm_system_table_ptr() as *const c_void)
    }

    /// Register platform-provided Patina components, parse guided HOBs into
    /// component storage, and dispatch all components to completion.
    ///
    /// Runs once during `StartUserCore` on the BSP. Components may install MM
    /// protocols, register MMI handlers, and consume configs, services, and HOBs
    /// via dependency injection. Configs are dispatched in two rounds: unlocked
    /// (for `ConfigMut<T>` components), then locked (for `Config<T>` consumers).
    fn dispatch_components<C: MmComponentInfo>(&self, hob: &Hob<'_>) {
        // Expose the MM services (protocol install/locate, MMI handler registration,
        // pool/page allocation) to components via the `MmServiceProvider` parameter.
        patina::mm_services::register_component_mm_services(MmUserCore::instance());

        let mut cd = self.component_dispatcher.lock();
        cd.apply_component_info::<C>();
        cd.insert_hobs(hob);

        cd.dispatch_to_completion();
        cd.lock_configs();
        cd.dispatch_to_completion();

        cd.display_not_dispatched();
    }

    /// Handle the `UserRequest` command (runtime MMI dispatch).
    ///
    /// The supervisor passes a pointer to a buffer containing:
    /// - `EfiMmEntryContext` (at offset 0)
    /// - `MmCommBufferStatus` (at offset `context_size`)
    ///
    /// For synchronous MMIs the supervisor has already copied the external
    /// communication buffer into an internal (user-accessible) region.  We:
    /// 1. Validate the buffer via the `MmIsCommBuffer` syscall
    /// 2. Parse the `EfiMmCommunicateHeader` to extract the handler GUID and data
    /// 3. Dispatch via `mmi_manage` with the GUID and data pointer
    ///
    /// Asynchronous MMIs (timer, etc.) are always dispatched as root-only
    /// (`mmi_manage(None, …)`).
    ///
    /// Mirrors the C `MmEntryPoint` flow in `StandaloneMmCore.c`.
    fn handle_user_request(&self, supv_to_user_buffer: u64, context_size: u64) -> u64 {
        if supv_to_user_buffer == 0 {
            log::error!("Supervisor-to-user buffer is null.");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        // Read the EfiMmEntryContext
        let entry_context = unsafe { core::ptr::read(supv_to_user_buffer as *const EfiMmEntryContext) };

        self.update_cpu_context(entry_context.currently_executing_cpu as usize, entry_context.number_of_cpus as usize);

        // Read MmCommBufferStatus (immediately after the context)
        let comm_status = unsafe {
            core::ptr::read((supv_to_user_buffer as *const u8).add(context_size as usize) as *const MmCommBufferStatus)
        };

        // ---- Synchronous MMI dispatch ----
        let comm_buffer_base = COMM_BUFFER_BASE.load(Ordering::Acquire);
        let comm_buffer_size = COMM_BUFFER_SIZE.load(Ordering::Acquire);

        let mut updated_status = comm_status;

        if comm_buffer_base != 0 && comm_status.is_comm_buffer_valid != 0 {
            // Validate the communication buffer via a supervisor syscall.
            let mut return_buffer_size: u64 = 0;
            let sync_status = if !mm_mem::is_comm_buffer(comm_buffer_base, comm_buffer_size) {
                log::error!("MmIsCommBuffer rejected buffer at 0x{:x} size 0x{:x}", comm_buffer_base, comm_buffer_size);
                efi::Status::NOT_FOUND
            } else {
                self.dispatch_synchronous_mmi(comm_buffer_base, comm_buffer_size, &mut return_buffer_size)
            };

            updated_status.is_comm_buffer_valid = 0;
            updated_status.return_status = if sync_status == efi::Status::SUCCESS {
                efi::Status::SUCCESS.as_usize() as u64
            } else {
                efi::Status::NOT_FOUND.as_usize() as u64
            };
            updated_status.return_buffer_size = return_buffer_size;
        }

        // ---- Asynchronous MMI dispatch (always runs) ----
        // SAFETY: no comm buffer is supplied for the async (root-handler) dispatch.
        unsafe { self.mmi_manage(None, core::ptr::null(), core::ptr::null_mut(), core::ptr::null_mut()) };

        // Write back the updated status to the supervisor-to-user buffer
        unsafe {
            core::ptr::write(
                (supv_to_user_buffer as *mut u8).add(context_size as usize) as *mut MmCommBufferStatus,
                updated_status,
            );
        }

        efi::Status::SUCCESS.as_usize() as u64
    }

    /// Parse the `EfiMmCommunicateHeader` from the communication buffer and
    /// dispatch the appropriate GUID-specific MMI handler.
    ///
    /// Returns the dispatch status and updates `return_buffer_size` with the
    /// total response size (header + data).
    fn dispatch_synchronous_mmi(
        &self,
        comm_buffer_base: u64,
        comm_buffer_size: u64,
        return_buffer_size: &mut u64,
    ) -> efi::Status {
        let buffer_size = comm_buffer_size as usize;

        // The buffer must be large enough for at least the communicate header.
        if buffer_size < EfiMmCommunicateHeader::size() {
            log::error!(
                "Communication buffer too small for header: {} < {}",
                buffer_size,
                EfiMmCommunicateHeader::size()
            );
            return efi::Status::BAD_BUFFER_SIZE;
        }

        // SAFETY: We verified the buffer is large enough for the header.
        let header = unsafe { core::ptr::read_unaligned(comm_buffer_base as *const EfiMmCommunicateHeader) };

        // Determine header layout: check for V3 signature first, then fall
        // back to the legacy `EfiMmCommunicateHeader`.
        let (comm_guid_ptr, comm_header_size, mut data_size) = if header.header_guid()
            == patina::Guid::from_ref(&patina::pi::protocols::communication3::COMMUNICATE_HEADER_V3_GUID)
        {
            // V3 header
            let v3 = unsafe {
                core::ptr::read_unaligned(
                    comm_buffer_base as *const patina::pi::protocols::communication3::EfiMmCommunicateHeader,
                )
            };
            let header_size = mem::size_of::<patina::pi::protocols::communication3::EfiMmCommunicateHeader>();
            let total = v3.buffer_size as usize;
            if total > buffer_size {
                log::error!("V3 buffer_size 0x{:x} exceeds available 0x{:x}", total, buffer_size);
                return efi::Status::BAD_BUFFER_SIZE;
            }
            // GUID to dispatch is `message_guid` in V3
            let guid_offset =
                core::mem::offset_of!(patina::pi::protocols::communication3::EfiMmCommunicateHeader, message_guid);
            let guid_ptr = (comm_buffer_base as *const u8).wrapping_add(guid_offset) as *const efi::Guid;
            (guid_ptr, header_size, total.saturating_sub(header_size))
        } else {
            // Legacy header
            let message_length = header.message_length();
            let total = EfiMmCommunicateHeader::size() + message_length;
            if total > buffer_size {
                log::error!(
                    "Legacy message_length 0x{:x} exceeds available 0x{:x}",
                    message_length,
                    buffer_size.saturating_sub(EfiMmCommunicateHeader::size())
                );
                return efi::Status::BAD_BUFFER_SIZE;
            }
            // GUID to dispatch is `header_guid` in legacy
            let guid_ptr = comm_buffer_base as *const efi::Guid;
            (guid_ptr, EfiMmCommunicateHeader::size(), message_length)
        };

        // Zero the remainder of the buffer past the message (matches C behaviour).
        let used = comm_header_size + data_size;
        if used < buffer_size {
            unsafe {
                core::ptr::write_bytes((comm_buffer_base as *mut u8).add(used), 0, buffer_size - used);
            }
        }

        // Dispatch the GUID-specific handler.
        let comm_data_ptr = unsafe { (comm_buffer_base as *mut u8).add(comm_header_size) as *mut c_void };

        // SAFETY: `comm_guid_ptr` references the message GUID parsed from the validated comm buffer;
        // `comm_data_ptr`/`data_size` describe the comm-buffer payload.
        let status = unsafe {
            self.mmi_manage(Some(&*comm_guid_ptr), core::ptr::null(), comm_data_ptr, &mut data_size as *mut usize)
        };

        *return_buffer_size = (data_size + comm_header_size) as u64;
        status
    }

    /// Handle the `UserApProcedure` command.
    ///
    /// The supervisor passes the procedure pointer and argument. We call the procedure
    /// directly since we're already in user mode.
    fn handle_user_ap_procedure(&self, procedure: u64, argument: u64) -> u64 {
        if procedure == 0 {
            log::error!("AP procedure pointer is null.");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        log::trace!("Executing AP procedure at 0x{:016x} with arg 0x{:016x}", procedure, argument);

        // SAFETY: The supervisor has validated the procedure pointer before dispatching.
        // The procedure follows the EFI AP_PROCEDURE calling convention.
        type EfiApProcedure = unsafe extern "efiapi" fn(*mut c_void);
        let proc_fn: EfiApProcedure = unsafe { core::mem::transmute(procedure) };
        unsafe { proc_fn(argument as *mut c_void) };

        efi::Status::SUCCESS.as_usize() as u64
    }

    /// Discover the communication buffer address from HOBs and store it for
    /// later use in `handle_user_request`.
    ///
    /// The supervisor rewrites the HOB's `physical_start` field to point to
    /// the internal (user-accessible) copy of the buffer before invoking
    /// `StartUserCore`, so the address we read here is the one we should
    /// read from at runtime.
    fn discover_comm_buffer(&self, hob: &Hob<'_>) {
        for current_hob in hob {
            if let Hob::GuidHob(guid_hob, data) = current_hob
                && guid_hob.name == MM_COMM_BUFFER_HOB_GUID
                && let Ok((buffer_data, _)) = MmCommonBufferHobData::read_from_prefix(data)
            {
                let physical_start = buffer_data.physical_start;
                let number_of_pages = buffer_data.number_of_pages;

                let buffer_size = number_of_pages.saturating_mul(4096);

                COMM_BUFFER_BASE.store(physical_start, Ordering::Release);
                COMM_BUFFER_SIZE.store(buffer_size, Ordering::Release);

                log::info!(
                    "Found MM communication buffer: base=0x{:016x}, pages={}, size=0x{:x}",
                    physical_start,
                    number_of_pages,
                    buffer_size,
                );
                return;
            }
        }

        log::warn!("No MM communication buffer HOB found — only root MMI handlers will be supported.");
    }
}
