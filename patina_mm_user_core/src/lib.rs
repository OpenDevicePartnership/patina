//! MM User Core
//!
//! The Ring 3 core of a Standalone MM environment, entered by the MM Supervisor through
//! privilege demotion. It discovers MM drivers from HOBs, dispatches them in dependency
//! order, runs the platform's Patina components, and manages MMI handlers. It reaches
//! supervisor resources only through syscalls.
//!
//! ## Entry Protocol
//!
//! The supervisor passes `op_code` (a [`UserCommandType`]) and two command-specific
//! arguments:
//! - `StartUserCore` (0): `arg1` is the HOB list. One-time initialization: discovers the
//!   drivers, builds the MM System Table and registers the core MMI handlers. Component and
//!   driver dispatch waits for the MM dispatch event MMI.
//! - `UserRequest` (1): `arg1` points to an [`MmUserRequest`] and `arg2` is its size.
//!   Dispatches the MMI to the registered handlers.
//! - `UserApProcedure` (2): runs the procedure at `arg1` on an AP with `arg2` as its argument.
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
#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg(target_arch = "x86_64")]

extern crate alloc;

pub mod component_dispatcher;
mod config_table;
mod core_handlers;
mod image_name;
mod mm_dispatcher;
mod mm_mem;
mod mm_services;
mod mmi;
// The pool allocator is installed as the global allocator only on the UEFI target, so host
// builds outside tests do not use it.
#[cfg_attr(not(target_os = "uefi"), allow(dead_code))]
mod pool_allocator;
mod protocol_db;
#[cfg(test)]
mod test_support;

use core::{
    ffi::c_void,
    mem,
    num::NonZeroUsize,
    ptr::NonNull,
    sync::atomic::{AtomicBool, Ordering},
};

use alloc::boxed::Box;
use patina::standard::efi;
use patina::{
    management_mode::mm_services::MmServices,
    pi::hob::{Hob, PhaseHandoffInformationTable},
};
use spin::{Mutex, Once};

use crate::{
    component_dispatcher::{MmComponentDispatcher, MmComponentInfo},
    config_table::MmConfigurationTableDb,
    mm_dispatcher::MmDispatcher,
    mmi::MmiDatabase,
    protocol_db::ProtocolDatabase,
};

use patina::{
    management_mode::supervisor::{MmUserRequest, UserCommandType},
    pi::{mm_cis::EfiMmSystemTable, protocol::communication::EfiMmCommunicateHeader},
};

// The entry shim calls the platform's `user_core_main`, so host builds (tests, doctests) leave it
// out to link.
#[cfg(target_os = "uefi")]
core::arch::global_asm!(include_str!("entry_point.asm"));

/// Address of the published [`MmUserCore`] instance.
static __USER_CORE: Once<NonZeroUsize> = Once::new();

/// Crate version, kept in the image so a debugger can identify the core.
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
    pub(crate) mmi_db: MmiDatabase,
    /// The protocol/handle database (for depex evaluation and driver services).
    pub(crate) protocol_db: ProtocolDatabase,
    /// The configuration-table database backing `MmInstallConfigurationTable`.
    pub(crate) config_table_db: MmConfigurationTableDb,
    /// The driver dispatcher.
    pub(crate) dispatcher: MmDispatcher,
    /// The Patina component dispatcher (dependency-injected `#[component]` entry points).
    pub(crate) component_dispatcher: Mutex<MmComponentDispatcher>,
    /// Address of the supervisor's persistent user HOB list, recorded by `StartUserCore`.
    hob_list: Once<usize>,
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
            hob_list: Once::new(),
            mm_system_table: Once::new(),
            initialized: AtomicBool::new(false),
        }
    }

    /// Publishes this instance for global access.
    ///
    /// Returns false if a different instance was already published.
    #[must_use]
    fn set_instance(&'static self) -> bool {
        let physical_address = NonNull::from_ref(self).expose_provenance();
        &physical_address == __USER_CORE.call_once(|| physical_address)
    }

    /// Returns the published MM User Core instance.
    ///
    /// ## Panics
    ///
    /// Panics if no instance has been published yet.
    pub(crate) fn instance<'a>() -> &'a Self {
        // SAFETY: `set_instance` stored this address from a `&'static Self`.
        unsafe {
            NonNull::<Self>::with_exposed_provenance(*__USER_CORE.get().expect("MM User Core is not initialized."))
                .as_ref()
        }
    }

    /// Returns the published MM User Core instance, or `None` if none has been published.
    #[cfg(test)]
    pub(crate) fn try_instance<'a>() -> Option<&'a Self> {
        // SAFETY: A stored address came from a `&'static Self` in `set_instance`.
        __USER_CORE.get().map(|&addr| unsafe { NonNull::<Self>::with_exposed_provenance(addr).as_ref() })
    }

    /// Builds the MM System Table on the heap on the first call and returns it.
    ///
    /// The table's function pointers are thunks into this core (see [`crate::mm_services`]).
    /// The heap must be available.
    fn init_mm_system_table(&'static self) -> *mut EfiMmSystemTable {
        let addr = *self.mm_system_table.call_once(|| {
            let ptr = Box::into_raw(Box::new(mm_services::build_mm_system_table()));
            log::debug!("MM System Table allocated at {ptr:p}");
            ptr.expose_provenance()
        });
        core::ptr::with_exposed_provenance_mut(addr)
    }

    /// Returns the MM System Table pointer, or null if it has not been built yet.
    pub(crate) fn mm_system_table_ptr(&self) -> *mut EfiMmSystemTable {
        self.mm_system_table.get().map_or(core::ptr::null_mut(), |&addr| core::ptr::with_exposed_provenance_mut(addr))
    }

    /// Copies the current processor state into the MM System Table.
    ///
    /// Called at the start of each `UserRequest`. Does nothing if the table has not been built.
    fn update_cpu_context(&self, currently_executing_cpu: usize, number_of_cpus: usize) {
        let ptr = self.mm_system_table_ptr();
        if ptr.is_null() {
            return;
        }
        // SAFETY: The table lives for the lifetime of the core, and these fields are only
        // written here, on the BSP.
        unsafe {
            (*ptr).currently_executing_cpu = currently_executing_cpu;
            (*ptr).number_of_cpus = number_of_cpus;
        }
    }

    /// Main entry point for the MM User Core.
    ///
    /// The supervisor reaches this through `invoke_demoted_routine` and the `efi_main` shim.
    /// `op_code` is a [`UserCommandType`]; `arg1` and `arg2` depend on the command.
    ///
    /// Returns an `efi::Status` value: 0 on success, non-zero on failure.
    pub fn entry_point_worker<C: MmComponentInfo>(&'static self, op_code: u64, arg1: u64, arg2: u64) -> u64 {
        let command = match UserCommandType::try_from(op_code) {
            Ok(command) => command,
            Err(unknown) => {
                log::error!("Unknown command type: {unknown}");
                return efi::Status::INVALID_PARAMETER.as_usize() as u64;
            }
        };

        match command {
            UserCommandType::StartUserCore => self.handle_start_user_core::<C>(arg1 as *const c_void),
            UserCommandType::UserRequest => self.handle_user_request(arg1, arg2),
            UserCommandType::UserApProcedure => self.handle_user_ap_procedure(arg1, arg2),
        }
    }

    /// Handles `StartUserCore`. `hob_list` is the supervisor's HOB list.
    ///
    /// Publishes the instance, discovers the MM drivers, builds the MM System Table,
    /// publishes the HOB list as a configuration table and registers the core MMI handlers.
    /// Returns `ALREADY_STARTED` on a repeat call and `INVALID_PARAMETER` for a null HOB list.
    fn handle_start_user_core<C: MmComponentInfo>(&'static self, hob_list: *const c_void) -> u64 {
        // `set_instance` accepts this core again, so a repeat `StartUserCore` is caught here.
        if self.initialized.load(Ordering::Acquire) {
            log::warn!("MM User Core is already initialized, skipping re-initialization.");
            return efi::Status::ALREADY_STARTED.as_usize() as u64;
        }

        if !self.set_instance() {
            log::warn!("MM User Core instance was already set, skipping re-initialization.");
            return efi::Status::ALREADY_STARTED.as_usize() as u64;
        }

        // From here on, every MM System Table call is served by this instance.
        mm_services::init_mm_services(self);

        if hob_list.is_null() {
            log::error!("HOB list pointer is null.");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        log::info!("MM User Core v{MM_USER_CORE_VERSION} starting initialization...");

        // Enable the heap before anything allocates.
        mm_mem::SYSCALL_PAGE_ALLOCATOR.set_initialized();

        // SAFETY: The supervisor passes a pointer to the HOB list it built, which outlives this
        // call, and the null case was rejected above.
        let hob_list_info = unsafe { &*hob_list.cast::<PhaseHandoffInformationTable>() };

        let hob = Hob::Handoff(hob_list_info);

        // Dispatch waits for the `MM_DISPATCH_EVENT_GROUP_GUID` handler.
        self.dispatcher.discover(&hob);

        let mm_system_table = self.init_mm_system_table();
        log::info!("MM System Table initialized at {mm_system_table:p}");

        // Publish the HOB list so drivers can find it through the system table.
        // SAFETY: `hob_list` points to the supervisor-provided HOB list, which outlives the
        // configuration-table entry.
        let published =
            unsafe { self.install_configuration_table(&patina::pi::guid::HOB_LIST_TABLE_GUID, hob_list.cast_mut(), 0) };
        if let Err(status) = published {
            log::error!("Failed to install HOB list configuration table: {:#x}", status.as_usize());
        }

        // Lifecycle handlers, including the one-shot component and driver dispatch handler.
        core_handlers::register_core_mmi_handlers::<C>();

        self.hob_list.call_once(|| hob_list.expose_provenance());

        self.initialized.store(true, Ordering::Release);
        log::info!("MM User Core initialization complete.");

        efi::Status::SUCCESS.as_usize() as u64
    }

    /// Dispatches the MM drivers discovered during `StartUserCore` in dependency order.
    ///
    /// Called by the `MM_DISPATCH_EVENT_GROUP_GUID` handler in [`crate::core_handlers`].
    /// Returns the number of drivers that returned success.
    ///
    /// ## Errors
    ///
    /// Returns `ALREADY_STARTED` if a dispatch pass is already running.
    pub(crate) fn dispatch_drivers(&self) -> Result<usize, efi::Status> {
        self.dispatcher.dispatch(&self.protocol_db, self.mm_system_table_ptr() as *const c_void)
    }

    /// Registers the platform's Patina components, runs the HOB parsers, and dispatches the
    /// components with configs unlocked (for `ConfigMut<T>`) and again after locking them (for
    /// `Config<T>`).
    ///
    /// Called once by the `MM_DISPATCH_EVENT_GROUP_GUID` handler, before driver dispatch.
    ///
    /// ## Errors
    ///
    /// Returns `NOT_READY` if `StartUserCore` has not recorded the HOB list.
    pub(crate) fn dispatch_components<C: MmComponentInfo>(&self) -> Result<(), efi::Status> {
        let Some(&hob_list) = self.hob_list.get() else {
            log::error!("MM component dispatch requested before the HOB list was recorded");
            return Err(efi::Status::NOT_READY);
        };

        // SAFETY: `StartUserCore` recorded the non-null, supervisor-validated HOB list, which
        // stays mapped across MM entries.
        let hob =
            Hob::Handoff(unsafe { &*core::ptr::with_exposed_provenance::<PhaseHandoffInformationTable>(hob_list) });

        // Back the `MmServiceProvider` component parameter with this core.
        patina::management_mode::mm_services::register_component_mm_services(MmUserCore::instance());

        let mut dispatcher = self.component_dispatcher.lock();
        dispatcher.apply_component_info::<C>();
        dispatcher.insert_hobs(&hob);

        dispatcher.dispatch_to_completion();
        dispatcher.lock_configs();
        dispatcher.dispatch_to_completion();

        dispatcher.display_not_dispatched();
        Ok(())
    }

    /// Handles `UserRequest` (runtime MMI dispatch).
    ///
    /// `supv_to_user_buffer` points to an [`MmUserRequest`]. When its status marks the
    /// communication buffer valid, the buffer is checked with the `MmIsCommBuffer` syscall and
    /// the handlers for its GUID run; any failure is reported as `NOT_FOUND`. The root handlers
    /// then run on every request, and the updated status is written back.
    fn handle_user_request(&self, supv_to_user_buffer: u64, request_size: u64) -> u64 {
        if supv_to_user_buffer == 0 {
            log::error!("Supervisor-to-user buffer is null.");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        if request_size != size_of::<MmUserRequest>() as u64 {
            log::error!("Invalid user request handoff size: {request_size}, expected {}", size_of::<MmUserRequest>());
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        let request_ptr = supv_to_user_buffer as *mut MmUserRequest;
        // SAFETY: the supervisor supplies a live handoff buffer, and its pointer and layout
        // size were checked above. Take a snapshot before using the shared fields.
        let request = unsafe { request_ptr.read_unaligned() };

        self.update_cpu_context(
            request.entry_context.currently_executing_cpu as usize,
            request.entry_context.number_of_cpus as usize,
        );

        // ---- Synchronous MMI dispatch ----
        let comm_buffer_base = request.comm_buffer_address;
        let comm_buffer_size = request.comm_buffer_size;

        let mut updated_status = request.status;

        if request.status.is_comm_buffer_valid != 0 {
            let mut return_buffer_size: u64 = 0;
            let sync_status = if mm_mem::is_comm_buffer(comm_buffer_base, comm_buffer_size) {
                self.dispatch_synchronous_mmi(comm_buffer_base, comm_buffer_size, &mut return_buffer_size)
            } else {
                log::error!("MmIsCommBuffer rejected buffer at 0x{comm_buffer_base:x} size 0x{comm_buffer_size:x}");
                efi::Status::NOT_FOUND
            };

            updated_status.is_comm_buffer_valid = 0;
            let return_status = if sync_status == efi::Status::SUCCESS { sync_status } else { efi::Status::NOT_FOUND };
            updated_status.return_status = return_status.as_usize() as u64;
            updated_status.return_buffer_size = return_buffer_size;
        }

        // ---- Asynchronous MMI dispatch (always runs) ----
        // SAFETY: Root dispatch passes no context or communication buffer.
        unsafe { self.mmi_manage(None, core::ptr::null(), core::ptr::null_mut(), core::ptr::null_mut()) };

        // SAFETY: the status field lies within the supervisor's writable handoff buffer.
        unsafe {
            core::ptr::addr_of_mut!((*request_ptr).status).write_unaligned(updated_status);
        }

        efi::Status::SUCCESS.as_usize() as u64
    }

    /// Parses the `EfiMmCommunicateHeader` (legacy or V3) at the start of the communication
    /// buffer and dispatches the handlers for its GUID.
    ///
    /// Returns the dispatch status, or `BAD_BUFFER_SIZE` if the header or message does not fit
    /// the buffer. On a dispatch, `return_buffer_size` is set to the response size (header
    /// plus data), or 0 if the handler reported a size the buffer cannot hold.
    fn dispatch_synchronous_mmi(
        &self,
        comm_buffer_base: u64,
        comm_buffer_size: u64,
        return_buffer_size: &mut u64,
    ) -> efi::Status {
        let buffer_size = comm_buffer_size as usize;

        if buffer_size < EfiMmCommunicateHeader::size() {
            log::error!(
                "Communication buffer too small for header: {buffer_size} < {}",
                EfiMmCommunicateHeader::size()
            );
            return efi::Status::BAD_BUFFER_SIZE;
        }

        // SAFETY: The buffer was checked to be large enough for the header.
        let header = unsafe { core::ptr::read_unaligned(comm_buffer_base as *const EfiMmCommunicateHeader) };

        // A V3 header is identified by its header GUID; anything else is legacy. The dispatch
        // GUID is copied out of the buffer, so handlers see the value checked here.
        let (comm_guid, comm_header_size, mut data_size) = if header.header_guid()
            == patina::Guid::from_ref(&patina::pi::protocol::communication3::COMMUNICATE_HEADER_V3_GUID)
        {
            let header_size = mem::size_of::<patina::pi::protocol::communication3::EfiMmCommunicateHeader>();

            // The V3 header is larger than the legacy one checked above.
            if buffer_size < header_size {
                log::error!("Communication buffer too small for a V3 header: {buffer_size} < {header_size}");
                return efi::Status::BAD_BUFFER_SIZE;
            }

            // SAFETY: The buffer was checked above to hold a complete V3 header.
            let v3_header = unsafe {
                core::ptr::read_unaligned(
                    comm_buffer_base as *const patina::pi::protocol::communication3::EfiMmCommunicateHeader,
                )
            };

            // A total below the header size would leave the payload span negative.
            let total = v3_header.buffer_size as usize;
            if total < header_size || total > buffer_size {
                log::error!(
                    "V3 buffer_size 0x{total:x} is outside the valid range 0x{header_size:x}..=0x{buffer_size:x}"
                );
                return efi::Status::BAD_BUFFER_SIZE;
            }

            // V3 dispatches on `message_guid`.
            (v3_header.message_guid, header_size, total - header_size)
        } else {
            // Compare against the space after the header; adding the header size to the
            // caller-supplied `message_length` could overflow.
            let message_length = header.message_length();
            let available = buffer_size - EfiMmCommunicateHeader::size();
            if message_length > available {
                log::error!("Legacy message_length 0x{message_length:x} exceeds available 0x{available:x}");
                return efi::Status::BAD_BUFFER_SIZE;
            }
            // The legacy header dispatches on `header_guid`, its first field.
            // SAFETY: the buffer was checked above to hold a whole legacy header, which starts
            // with the GUID.
            let guid = unsafe { core::ptr::read_unaligned(comm_buffer_base as *const efi::Guid) };
            (guid, EfiMmCommunicateHeader::size(), message_length)
        };

        // Zero the unused tail of the buffer.
        let used = comm_header_size + data_size;
        if used < buffer_size {
            // SAFETY: `used` is less than `buffer_size`, so the tail lies inside the comm buffer
            // the supervisor validated.
            unsafe {
                core::ptr::write_bytes((comm_buffer_base as *mut u8).add(used), 0, buffer_size - used);
            }
        }

        // SAFETY: `comm_header_size` is within the buffer, whose size was checked above.
        let comm_data_ptr = unsafe { (comm_buffer_base as *mut u8).add(comm_header_size).cast::<c_void>() };

        // SAFETY: `comm_guid` is the message GUID copied from the validated comm buffer;
        // `comm_data_ptr`/`data_size` describe the comm-buffer payload.
        let status = unsafe { self.mmi_manage(Some(&comm_guid), core::ptr::null(), comm_data_ptr, &raw mut data_size) };

        // The handler returns its response length in `data_size`. A length past the end of the
        // buffer cannot describe a written response, so no data is returned.
        let reported = comm_header_size.saturating_add(data_size);
        if reported > buffer_size {
            log::error!(
                "Handler reported a 0x{reported:x}-byte response for a 0x{buffer_size:x}-byte communication buffer; \
                 rejecting"
            );
            *return_buffer_size = 0;
            return efi::Status::BAD_BUFFER_SIZE;
        }
        *return_buffer_size = reported as u64;

        status
    }

    /// Handles `UserApProcedure` by calling `procedure` with `argument`. No privilege change
    /// is needed because the core already runs in user mode.
    fn handle_user_ap_procedure(&self, procedure: u64, argument: u64) -> u64 {
        if procedure == 0 {
            log::error!("AP procedure pointer is null.");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        log::trace!("Executing AP procedure at 0x{procedure:016x} with arg 0x{argument:016x}");

        type EfiApProcedure = unsafe extern "efiapi" fn(*mut c_void);
        // SAFETY: The supervisor has validated the procedure pointer before dispatching.
        // The procedure follows the EFI AP_PROCEDURE calling convention.
        let proc_fn: EfiApProcedure = unsafe { core::mem::transmute(procedure) };
        // SAFETY: As above.
        unsafe { proc_fn(argument as *mut c_void) };

        efi::Status::SUCCESS.as_usize() as u64
    }
}
