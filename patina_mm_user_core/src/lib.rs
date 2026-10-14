//! MM User Core
//!
//! A Rust implementation of the MM User Core for Standalone MM.
//!
//! The user core runs in Ring 3 and is entered by the MM Supervisor through privilege
//! demotion. It does the work of the C `StandaloneMmCore`: it discovers drivers from HOBs,
//! evaluates their dependency expressions, dispatches them, and manages MMI handlers. It has
//! no direct access to supervisor resources and reaches them through syscalls.
//!
//! ## Entry Protocol
//!
//! The supervisor calls the entry point with three arguments:
//! - `op_code`: the command, a [`UserCommandType`].
//! - `arg1`: a command-specific pointer (HOB list, request buffer, or AP procedure).
//! - `arg2`: a command-specific size or argument.
//!
//! The commands are:
//! - `StartUserCore` (0): one-time initialization. Discovers drivers from the HOB list,
//!   builds the MM System Table, registers the core MMI handlers. The platform's component
//!   and driver dispatch waits for the MM dispatch event MMI.
//! - `UserRequest` (1): runtime MMI dispatch. `arg1` points to [`MmUserRequest`] and `arg2`
//!   is the size of that complete handoff. Parses the supplied communication buffer and
//!   calls the registered handlers.
//! - `UserApProcedure` (2): runs a procedure on an AP.
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
pub mod config_table;
pub mod core_handlers;
mod image_name;
pub mod mm_dispatcher;
pub mod mm_mem;
pub mod mm_services;
pub mod mmi;
pub mod pool_allocator;
pub mod protocol_db;
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

// The entry-point shim references `user_core_main`, which is provided by the platform binary, and
// is only meaningful on the firmware (UEFI) target. Exclude it from host builds (tests, doctests)
// so their harnesses can link.
#[cfg(target_os = "uefi")]
core::arch::global_asm!(include_str!("entry_point.asm"));

/// Static reference to the user core instance.
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
    pub mmi_db: MmiDatabase,
    /// The protocol/handle database (for depex evaluation and driver services).
    pub protocol_db: ProtocolDatabase,
    /// The configuration-table database backing `MmInstallConfigurationTable`.
    pub config_table_db: MmConfigurationTableDb,
    /// The driver dispatcher.
    pub dispatcher: MmDispatcher,
    /// The Patina component dispatcher (dependency-injected `#[component]` entry points).
    pub component_dispatcher: Mutex<MmComponentDispatcher>,
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
    pub fn instance<'a>() -> &'a Self {
        // SAFETY: `set_instance` stored this address from a `&'static Self`.
        unsafe {
            NonNull::<Self>::with_exposed_provenance(*__USER_CORE.get().expect("MM User Core is not initialized."))
                .as_ref()
        }
    }

    /// Returns the published MM User Core instance, or `None` if none has been published.
    pub fn try_instance<'a>() -> Option<&'a Self> {
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
    /// Publishes the instance, discovers the MM drivers from the HOB list,
    /// builds the MM System Table, publishes the HOB list as a configuration
    /// table, registers the core MMI handlers.
    fn handle_start_user_core<C: MmComponentInfo>(&'static self, hob_list: *const c_void) -> u64 {
        // `set_instance` only rejects a different core, so a repeat `StartUserCore` on this
        // core has to be caught here or initialization runs a second time.
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

        // Discover the drivers now, while the HOB list is available. Dispatch waits for the
        // `MM_DISPATCH_EVENT_GROUP_GUID` handler.
        self.dispatcher.discover(&hob);

        let mm_system_table = self.init_mm_system_table();
        log::info!("MM System Table initialized at {mm_system_table:p}");

        // Publish the HOB list so drivers can find it through the system table, as the C
        // `InitializeMmHobList` does.
        // SAFETY: `hob_list` points to the supervisor-provided HOB list and remains valid for
        // the lifetime of the configuration-table entry.
        let published =
            unsafe { self.install_configuration_table(&patina::pi::guid::HOB_LIST_TABLE_GUID, hob_list.cast_mut(), 0) };
        if let Err(status) = published {
            log::error!("Failed to install HOB list configuration table: {:#x}", status.as_usize());
        }

        // Lifecycle handlers (ready-to-lock, end-of-DXE, and so on), including the deferred
        // component and driver dispatch handler that the supervisor triggers once the MM
        // foundation is ready.
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

    /// Registers the platform's Patina components, feeds guided HOBs to their parsers, and
    /// dispatches the components until no more can run.
    ///
    /// Called once by the `MM_DISPATCH_EVENT_GROUP_GUID` handler, before MM drivers. Components
    /// run before configs are locked (for `ConfigMut<T>`) and after (for `Config<T>` consumers).
    ///
    /// ## Errors
    ///
    /// Returns `NOT_READY` if `StartUserCore` has not recorded the HOB list.
    pub(crate) fn dispatch_components<C: MmComponentInfo>(&self) -> Result<(), efi::Status> {
        let Some(&hob_list) = self.hob_list.get() else {
            log::error!("MM component dispatch requested before the HOB list was recorded");
            return Err(efi::Status::NOT_READY);
        };

        // SAFETY: `StartUserCore` recorded the non-null, supervisor-validated user HOB copy.
        // That read-only copy remains live across MM entries, as required by its MMST publication.
        let hob =
            Hob::Handoff(unsafe { &*core::ptr::with_exposed_provenance::<PhaseHandoffInformationTable>(hob_list) });

        // Expose the MM services (protocol install/locate, MMI handler registration,
        // pool/page allocation) to components through the `MmServiceProvider` parameter.
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
    /// The supervisor passes an [`MmUserRequest`] containing the CPU context, status, and
    /// internal communication buffer address and capacity. For a synchronous MMI it has
    /// already copied the caller's communication buffer into that user-accessible buffer.
    ///
    /// If the status marks the communication buffer valid, the buffer is checked with the
    /// `MmIsCommBuffer` syscall, its `EfiMmCommunicateHeader` is parsed, and the handlers for
    /// its GUID are dispatched. The root handlers are then dispatched on every request
    /// (`mmi_manage(None, ...)`), and the updated status is written back.
    ///
    /// Follows the C `MmEntryPoint` in `StandaloneMmCore.c`.
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

        // The buffer must be large enough for at least the communicate header.
        if buffer_size < EfiMmCommunicateHeader::size() {
            log::error!(
                "Communication buffer too small for header: {buffer_size} < {}",
                EfiMmCommunicateHeader::size()
            );
            return efi::Status::BAD_BUFFER_SIZE;
        }

        // SAFETY: The buffer was checked to be large enough for the header.
        let header = unsafe { core::ptr::read_unaligned(comm_buffer_base as *const EfiMmCommunicateHeader) };

        // A V3 header is identified by its header GUID. Anything else is a legacy header. The
        // dispatch GUID is copied out of the buffer, so the value the handlers see is the one
        // checked here.
        let (comm_guid, comm_header_size, mut data_size) = if header.header_guid()
            == patina::Guid::from_ref(&patina::pi::protocol::communication3::COMMUNICATE_HEADER_V3_GUID)
        {
            let header_size = mem::size_of::<patina::pi::protocol::communication3::EfiMmCommunicateHeader>();

            // The V3 header is larger than the legacy one, so the size check above does not cover
            // it. The GUID that selects this branch lies in the part both layouts share and is
            // chosen by the caller, so the V3 header must be bounds-checked before it is read.
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
            // Legacy header. `message_length` comes from the buffer, so it is compared against the
            // space left after the header. Adding the header size to it instead could overflow
            // and wrap back into the accepted range.
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

        // Zero the rest of the buffer past the message, as the C core does.
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

        // The handler writes its response length back through `data_size`, and the non-MM caller
        // uses it as the size of the response. A length past the end of the communication buffer
        // describes a response that was never written, so report the failure and return no data.
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    use alloc::borrow::Cow;
    use core::sync::atomic::AtomicU64;

    use patina::{
        component::{
            MetaData, Storage, UnsafeStorageCell, component,
            hob::{FromHob, Hob as ComponentHob, HobParseError},
            params::{Config as ConfigParam, ConfigMut, Param},
        },
        management_mode::{
            MmCommBufferStatus, event::MM_DISPATCH_EVENT_GROUP_GUID, mm_services::MmServiceProvider,
            supervisor::MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
        },
        pi::{
            hob::{
                END_OF_HOB_LIST, GUID_EXTENSION, GuidHob, HANDOFF, HobHeader, MEMORY_ALLOCATION,
                MemoryAllocationHeader, MemoryAllocationModule,
            },
            mm_cis::EfiMmEntryContext,
        },
    };

    use crate::component_dispatcher::{Add, Component, Config};

    const HANDLER_GUID: efi::Guid = efi::Guid::from_fields(0x1111_0000, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 1]);

    static CORE: MmUserCore = MmUserCore::new();
    static AP_ARGUMENT: AtomicU64 = AtomicU64::new(0);
    static HANDLER_CALLS: AtomicU64 = AtomicU64::new(0);
    static DISPATCH_ORDER: Mutex<Vec<&str>> = Mutex::new(Vec::new());
    static NESTED_DISPATCH_STATUS: AtomicU64 = AtomicU64::new(0);
    static NESTED_PREPARATION_STATUS: AtomicU64 = AtomicU64::new(0);
    static PREPARATION_READY: AtomicBool = AtomicBool::new(false);
    static DRIVER_READY: AtomicBool = AtomicBool::new(false);

    struct PreparationProbe;

    // SAFETY: this parameter accesses no storage and has no access requirements.
    unsafe impl Param for PreparationProbe {
        type State = ();
        type Item<'storage, 'state> = Self;

        unsafe fn get_param<'storage, 'state>(
            _state: &'state Self::State,
            _storage: UnsafeStorageCell<'storage>,
        ) -> Self {
            Self
        }

        fn validate(_state: &Self::State, _storage: UnsafeStorageCell) -> bool {
            true
        }

        fn init_state(_storage: &mut Storage, _meta: &mut MetaData) -> Result<(), Cow<'static, str>> {
            DISPATCH_ORDER.lock().push("parameter initialization");
            Ok(())
        }
    }

    struct DispatchValue(u64);

    impl FromHob for DispatchValue {
        const HOB_GUID: patina::BinaryGuid = patina::BinaryGuid::from_string("00000000-0000-0000-0000-0000000000a1");

        fn parse(bytes: &[u8]) -> Result<Self, HobParseError> {
            DISPATCH_ORDER.lock().push("HOB parsing");
            Ok(Self(u64::from_le_bytes(bytes[..8].try_into().expect("test HOB contains a u64"))))
        }
    }

    struct ConfigureDispatch;

    impl ConfigureDispatch {
        fn new() -> Self {
            DISPATCH_ORDER.lock().push("construction");
            Self
        }
    }

    #[component]
    impl ConfigureDispatch {
        fn entry_point(
            self,
            mut config: ConfigMut<u64>,
            hob: ComponentHob<DispatchValue>,
            _probe: PreparationProbe,
        ) -> patina::error::Result<()> {
            *config = hob.0;
            DISPATCH_ORDER.lock().push("mutable-config entry");
            Ok(())
        }
    }

    struct PublishDispatchProtocol;

    #[component]
    impl PublishDispatchProtocol {
        fn entry_point(self, config: ConfigParam<u64>, services: MmServiceProvider) -> patina::error::Result<()> {
            assert_eq!(*config, 42, "the mutable-config pass consumed the startup HOB");
            DISPATCH_ORDER.lock().push("read-only-config entry");
            // SAFETY: the installed protocol is a null marker with no borrowed interface.
            unsafe {
                services.install_protocol_interface(None, &HANDLER_GUID, core::ptr::null_mut()).unwrap();
            }
            // SAFETY: the dispatch event has no context or payload.
            let nested = unsafe {
                services.mmi_manage(
                    Some(&MM_DISPATCH_EVENT_GROUP_GUID),
                    core::ptr::null(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                )
            };
            NESTED_DISPATCH_STATUS.store(nested.as_usize() as u64, Ordering::SeqCst);
            Ok(())
        }
    }

    struct DispatchPlatform;

    impl MmComponentInfo for DispatchPlatform {
        fn configs(mut add: Add<Config>) {
            // SAFETY: the core builds the MM System Table before any component callback.
            let table = unsafe { &*CORE.mm_system_table_ptr() };
            PREPARATION_READY.store(
                CORE.initialized.load(Ordering::Acquire)
                    && table.currently_executing_cpu == 1
                    && table.number_of_cpus == 4,
                Ordering::SeqCst,
            );
            DISPATCH_ORDER.lock().push("config registration");
            add.config(0u64);
        }

        fn services(_add: Add<crate::component_dispatcher::Service>) {
            DISPATCH_ORDER.lock().push("service registration");
        }

        fn components(mut add: Add<Component>) {
            DISPATCH_ORDER.lock().push("component registration");
            let nested = CORE.mmi_db.mmi_manage(
                Some(&MM_DISPATCH_EVENT_GROUP_GUID),
                core::ptr::null(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            );
            NESTED_PREPARATION_STATUS.store(nested.as_usize() as u64, Ordering::SeqCst);
            add.component(PublishDispatchProtocol);
            add.component(ConfigureDispatch::new());
        }
    }

    extern "efiapi" fn component_dependent_driver(handle: efi::Handle, table: *const c_void) -> efi::Status {
        // SAFETY: the dispatcher passes the initialized MM System Table.
        let table = unsafe { &*table.cast::<EfiMmSystemTable>() };
        DRIVER_READY.store(
            handle.is_null()
                && table.currently_executing_cpu == 1
                && table.number_of_cpus == 4
                && CORE.protocol_db.locate_protocol(&HANDLER_GUID).is_some(),
            Ordering::SeqCst,
        );
        DISPATCH_ORDER.lock().push("driver entry");
        efi::Status::SUCCESS
    }

    /// Publishes the process-wide core instance. nextest gives each test its own process, so the
    /// `Once` behind `set_instance` starts empty every time.
    fn init_core() -> &'static MmUserCore {
        assert!(CORE.set_instance(), "the core instance is set once per test process");
        MmUserCore::instance()
    }

    extern "efiapi" fn record_ap_argument(argument: *mut c_void) {
        AP_ARGUMENT.store(argument as u64, Ordering::SeqCst);
    }

    /// A platform that registers no components, configs or services.
    pub(super) struct BarePlatform;
    impl MmComponentInfo for BarePlatform {}

    fn counting_handler(_: &efi::Guid, _: *mut c_void, _: *mut usize) -> efi::Status {
        HANDLER_CALLS.fetch_add(1, Ordering::SeqCst);
        efi::Status::SUCCESS
    }

    /// Reports a response far larger than the communication buffer it was given.
    fn oversized_handler(_: &efi::Guid, _: *mut c_void, comm_buffer_size: *mut usize) -> efi::Status {
        // SAFETY: `mmi_manage` passes a pointer to the dispatcher's own `data_size`.
        unsafe { *comm_buffer_size = usize::MAX };
        efi::Status::SUCCESS
    }

    /// Builds a contiguous PI HOB list rooted at a PHIT, which is the shape the supervisor hands
    /// to `StartUserCore`.
    ///
    /// The HOB iterator walks raw memory from one header to the next, so the entries must be laid
    /// out consecutively with every length a multiple of eight to keep the next header aligned.
    pub(super) struct HobListBuffer {
        bytes: Vec<u8>,
        aligned: Vec<u64>,
    }

    impl HobListBuffer {
        pub(super) fn new() -> Self {
            let phit = PhaseHandoffInformationTable {
                header: HobHeader {
                    r#type: HANDOFF,
                    length: size_of::<PhaseHandoffInformationTable>() as u16,
                    reserved: 0,
                },
                version: 9,
                boot_mode: patina::pi::BootMode::BootWithFullConfiguration,
                memory_top: 0,
                memory_bottom: 0,
                free_memory_top: 0,
                free_memory_bottom: 0,
                end_of_hob_list: 0,
            };

            let mut this = Self { bytes: Vec::new(), aligned: Vec::new() };
            // SAFETY: `PhaseHandoffInformationTable` is `repr(C)` over integers and a `repr(u32)`
            // enum, so every byte is initialized.
            this.bytes.extend_from_slice(unsafe {
                core::slice::from_raw_parts(
                    core::ptr::from_ref(&phit).cast::<u8>(),
                    size_of::<PhaseHandoffInformationTable>(),
                )
            });
            this
        }

        fn guid_hob(mut self, guid: patina::BinaryGuid, payload: &[u8]) -> Self {
            let length = (size_of::<GuidHob>() + payload.len()).next_multiple_of(8);
            let header = GuidHob {
                header: HobHeader { r#type: GUID_EXTENSION, length: length as u16, reserved: 0 },
                name: guid,
            };

            // SAFETY: `GuidHob` is `repr(C)` and holds only integers and a GUID.
            self.bytes.extend_from_slice(unsafe {
                core::slice::from_raw_parts(core::ptr::from_ref(&header).cast::<u8>(), size_of::<GuidHob>())
            });
            self.bytes.extend_from_slice(payload);
            let padded = self.bytes.len() - payload.len() - size_of::<GuidHob>() + length;
            self.bytes.resize(padded, 0);
            self
        }

        fn driver(mut self) -> Self {
            static IMAGE: [u8; 64] = [0; 64];
            let module = MemoryAllocationModule {
                header: HobHeader {
                    r#type: MEMORY_ALLOCATION,
                    length: size_of::<MemoryAllocationModule>() as u16,
                    reserved: 0,
                },
                alloc_descriptor: MemoryAllocationHeader {
                    name: MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
                    memory_base_address: IMAGE.as_ptr() as u64,
                    memory_length: IMAGE.len() as u64,
                    memory_type: efi::BOOT_SERVICES_CODE,
                    reserved: [0; 4],
                },
                module_name: patina::BinaryGuid::from_string("00000000-0000-0000-0000-0000000000a2"),
                entry_point: component_dependent_driver as *const () as u64,
            };
            // SAFETY: `MemoryAllocationModule` is repr(C), with all bytes initialized.
            self.bytes.extend_from_slice(unsafe {
                core::slice::from_raw_parts(
                    core::ptr::from_ref(&module).cast::<u8>(),
                    size_of::<MemoryAllocationModule>(),
                )
            });
            self
        }

        pub(super) fn build(mut self) -> Self {
            let end = HobHeader { r#type: END_OF_HOB_LIST, length: size_of::<HobHeader>() as u16, reserved: 0 };
            // SAFETY: `HobHeader` is `repr(C)` over integers.
            self.bytes.extend_from_slice(unsafe {
                core::slice::from_raw_parts(core::ptr::from_ref(&end).cast::<u8>(), size_of::<HobHeader>())
            });

            self.aligned = vec![0u64; self.bytes.len().div_ceil(8)];
            // SAFETY: `aligned` owns at least `bytes.len()` bytes and is 8-byte aligned.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.bytes.as_ptr(),
                    self.aligned.as_mut_ptr().cast::<u8>(),
                    self.bytes.len(),
                );
            }
            self
        }

        pub(super) fn as_ptr(&self) -> *const c_void {
            self.aligned.as_ptr().cast::<c_void>()
        }
    }

    /// A legacy-format MM communication buffer of `total` bytes.
    fn legacy_comm_buffer(guid: efi::Guid, message: &[u8], total: usize) -> Vec<u64> {
        let mut storage = vec![0u64; total.div_ceil(8).max(1)];
        let header = EfiMmCommunicateHeader::new(patina::Guid::from_ref(&guid), message.len());
        // SAFETY: `storage` is 8-byte aligned and sized for the header plus the message.
        unsafe {
            let base = storage.as_mut_ptr().cast::<u8>();
            core::ptr::copy_nonoverlapping(header.as_bytes().as_ptr(), base, EfiMmCommunicateHeader::size());
            core::ptr::copy_nonoverlapping(message.as_ptr(), base.add(EfiMmCommunicateHeader::size()), message.len());
        }
        storage
    }

    /// Byte size of the V3 communicate header.
    fn v3_header_size() -> usize {
        mem::size_of::<patina::pi::protocol::communication3::EfiMmCommunicateHeader>()
    }

    /// A V3-format MM communication buffer of `total` bytes, whose `buffer_size` field claims
    /// `claimed_buffer_size`.
    fn v3_comm_buffer(message_guid: efi::Guid, total: usize, claimed_buffer_size: u64) -> Vec<u64> {
        let mut storage = vec![0u64; total.div_ceil(8).max(1)];
        // SAFETY: `storage` is 8-byte aligned; each field is written within `total` bytes, and
        // callers that build a short buffer only write the parts that fit.
        unsafe {
            let base = storage.as_mut_ptr().cast::<u8>();
            let v3_guid = patina::pi::protocol::communication3::COMMUNICATE_HEADER_V3_GUID;
            core::ptr::copy_nonoverlapping(v3_guid.as_bytes().as_ptr(), base, 16);
            if total >= 24 {
                core::ptr::write_unaligned(base.add(16).cast::<u64>(), claimed_buffer_size);
            }
            if total >= v3_header_size() {
                let guid_offset =
                    core::mem::offset_of!(patina::pi::protocol::communication3::EfiMmCommunicateHeader, message_guid);
                core::ptr::copy_nonoverlapping(
                    core::ptr::from_ref(&message_guid).cast::<u8>(),
                    base.add(guid_offset),
                    16,
                );
            }
        }
        storage
    }

    #[test]
    fn test_new_core_is_uninitialized() {
        let core = MmUserCore::default();

        assert!(MmUserCore::try_instance().is_none());
        assert!(core.mm_system_table_ptr().is_null());
        assert!(!core.initialized.load(Ordering::Acquire));
    }

    #[test]
    fn test_set_instance_publishes_the_core_once() {
        static OTHER: MmUserCore = MmUserCore::new();

        assert!(CORE.set_instance());
        assert!(core::ptr::eq(MmUserCore::instance(), &raw const CORE));

        // A second core cannot displace the published one.
        assert!(!OTHER.set_instance());
        assert!(core::ptr::eq(MmUserCore::try_instance().unwrap(), &raw const CORE));
    }

    #[test]
    fn test_entry_point_worker_rejects_an_unknown_command() {
        let core = init_core();

        let status = core.entry_point_worker::<BarePlatform>(99, 0, 0);

        assert_eq!(status, efi::Status::INVALID_PARAMETER.as_usize() as u64);
    }

    #[test]
    fn test_start_user_core_rejects_a_null_hob_list() {
        let core = init_core();

        let status = core.entry_point_worker::<BarePlatform>(UserCommandType::StartUserCore as u64, 0, 0);

        assert_eq!(status, efi::Status::INVALID_PARAMETER.as_usize() as u64);
    }

    #[test]
    fn test_start_user_core_initializes_and_refuses_to_run_twice() {
        let hobs = HobListBuffer::new().build();

        let status =
            CORE.entry_point_worker::<BarePlatform>(UserCommandType::StartUserCore as u64, hobs.as_ptr() as u64, 0);
        assert_eq!(status, efi::Status::SUCCESS.as_usize() as u64);

        let core = MmUserCore::instance();
        assert!(core.initialized.load(Ordering::Acquire));
        assert!(!core.mm_system_table_ptr().is_null(), "the MM System Table was built");
        // The HOB list is published so dispatched drivers can find it through the system table.
        assert_eq!(
            core.config_table_db.get_configuration_table(&patina::pi::guid::HOB_LIST_TABLE_GUID),
            Some(hobs.as_ptr().cast_mut())
        );

        // A repeat StartUserCore must not re-initialize.
        let repeat =
            CORE.entry_point_worker::<BarePlatform>(UserCommandType::StartUserCore as u64, hobs.as_ptr() as u64, 0);
        assert_eq!(repeat, efi::Status::ALREADY_STARTED.as_usize() as u64);
    }

    #[test]
    fn test_components_dispatch_with_drivers_only_on_the_dispatch_event() {
        let comm = legacy_comm_buffer(MM_DISPATCH_EVENT_GROUP_GUID.into_inner(), &[], patina::UEFI_PAGE_SIZE);
        let hobs = HobListBuffer::new().guid_hob(DispatchValue::HOB_GUID, &42u64.to_le_bytes()).driver().build();

        let status =
            CORE.entry_point_worker::<DispatchPlatform>(UserCommandType::StartUserCore as u64, hobs.as_ptr() as u64, 0);
        assert_eq!(status, efi::Status::SUCCESS.as_usize() as u64);
        assert!(DISPATCH_ORDER.lock().is_empty(), "startup must not execute any component-supplied code");
        assert!(CORE.protocol_db.locate_protocol(&HANDLER_GUID).is_none());
        mm_mem::mock::set_handler(|index, _, _, _| {
            assert_eq!(index, patina::management_mode::supervisor::SyscallIndex::MmIsCommBuffer.as_u64());
            1
        });

        let expected = [
            "config registration",
            "service registration",
            "component registration",
            "construction",
            "parameter initialization",
            "HOB parsing",
            "mutable-config entry",
            "read-only-config entry",
            "driver entry",
        ];
        for (valid, expected_order, expected_status) in [
            (0, &[][..], efi::Status::SUCCESS),
            (1, &expected[..], efi::Status::SUCCESS),
            (1, &expected[..], efi::Status::NOT_FOUND),
        ] {
            let mut request = MmUserRequest {
                entry_context: EfiMmEntryContext {
                    mm_startup_this_ap: 0,
                    currently_executing_cpu: 1,
                    number_of_cpus: 4,
                    cpu_save_state_size: 0,
                    cpu_save_state: 0,
                },
                status: MmCommBufferStatus {
                    is_comm_buffer_valid: valid,
                    _padding: [0; 7],
                    return_status: 0,
                    return_buffer_size: 0,
                },
                comm_buffer_address: comm.as_ptr() as u64,
                comm_buffer_size: patina::UEFI_PAGE_SIZE as u64,
            };
            let result = CORE.entry_point_worker::<DispatchPlatform>(
                UserCommandType::UserRequest as u64,
                core::ptr::from_mut(&mut request) as u64,
                size_of::<MmUserRequest>() as u64,
            );
            assert_eq!(result, efi::Status::SUCCESS.as_usize() as u64);
            assert_eq!(DISPATCH_ORDER.lock().as_slice(), expected_order);
            assert_eq!(request.status.return_status, expected_status.as_usize() as u64);
        }

        assert!(PREPARATION_READY.load(Ordering::SeqCst), "preparation must wait for the request's CPU context");
        assert!(DRIVER_READY.load(Ordering::SeqCst), "components and CPU context must be ready before drivers");
        assert_eq!(
            NESTED_PREPARATION_STATUS.load(Ordering::SeqCst),
            efi::Status::NOT_FOUND.as_usize() as u64,
            "the handler must retire before invoking registration callbacks"
        );
        assert_eq!(
            NESTED_DISPATCH_STATUS.load(Ordering::SeqCst),
            efi::Status::NOT_FOUND.as_usize() as u64,
            "the handler must retire before calling components to prevent recursive dispatch"
        );
    }

    #[test]
    fn test_init_mm_system_table_builds_the_table_once() {
        let core = init_core();

        let first = core.init_mm_system_table();
        let second = core.init_mm_system_table();

        assert!(!first.is_null());
        assert_eq!(first, second, "the table is allocated once and reused");
        assert_eq!(core.mm_system_table_ptr(), first);
    }

    #[test]
    fn test_update_cpu_context_is_a_no_op_without_a_system_table() {
        let core = init_core();

        // Must not dereference the null table pointer.
        core.update_cpu_context(1, 4);

        assert!(core.mm_system_table_ptr().is_null());
    }

    #[test]
    fn test_update_cpu_context_reflects_the_executing_processor() {
        let core = init_core();
        let table = core.init_mm_system_table();

        core.update_cpu_context(3, 8);

        // SAFETY: the table is heap-allocated and lives for the process.
        unsafe {
            assert_eq!((*table).currently_executing_cpu, 3);
            assert_eq!((*table).number_of_cpus, 8);
        }
    }

    #[test]
    fn test_ap_procedure_rejects_a_null_pointer() {
        let core = init_core();

        let status = core.entry_point_worker::<BarePlatform>(UserCommandType::UserApProcedure as u64, 0, 0);

        assert_eq!(status, efi::Status::INVALID_PARAMETER.as_usize() as u64);
    }

    #[test]
    fn test_ap_procedure_invokes_the_supplied_routine() {
        let core = init_core();
        let procedure = (record_ap_argument as extern "efiapi" fn(*mut c_void)) as usize as u64;

        let status =
            core.entry_point_worker::<BarePlatform>(UserCommandType::UserApProcedure as u64, procedure, 0xabcd);

        assert_eq!(status, efi::Status::SUCCESS.as_usize() as u64);
        assert_eq!(AP_ARGUMENT.load(Ordering::SeqCst), 0xabcd);
    }

    #[test]
    fn test_user_request_rejects_a_null_buffer() {
        let core = init_core();

        let status = core.entry_point_worker::<BarePlatform>(
            UserCommandType::UserRequest as u64,
            0,
            size_of::<MmUserRequest>() as u64,
        );

        assert_eq!(status, efi::Status::INVALID_PARAMETER.as_usize() as u64);
    }

    #[test]
    fn test_user_request_rejects_an_invalid_handoff_size_before_reading_it() {
        let core = init_core();
        let expected = size_of::<MmUserRequest>() as u64;

        for size in [0, size_of::<EfiMmEntryContext>() as u64, expected - 1, expected + 1, u64::MAX] {
            let status = core.entry_point_worker::<BarePlatform>(UserCommandType::UserRequest as u64, 1, size);

            assert_eq!(status, efi::Status::INVALID_PARAMETER.as_usize() as u64);
        }
    }

    #[test]
    fn test_user_request_uses_each_handoff_without_a_comm_buffer_hob() {
        let hobs = HobListBuffer::new().build();
        assert_eq!(
            CORE.entry_point_worker::<BarePlatform>(UserCommandType::StartUserCore as u64, hobs.as_ptr() as u64, 0),
            efi::Status::SUCCESS.as_usize() as u64
        );
        CORE.mmi_db.register_internal_handler(counting_handler, Some(&HANDLER_GUID)).expect("handler registers");

        let legacy = legacy_comm_buffer(HANDLER_GUID, &[0xAA; 8], 64);
        let v3 = v3_comm_buffer(HANDLER_GUID, 96, 96);
        for (index, (address, capacity, response_size)) in
            [(legacy.as_ptr() as u64, 64, EfiMmCommunicateHeader::size() as u64 + 8), (v3.as_ptr() as u64, 96, 96)]
                .into_iter()
                .enumerate()
        {
            mm_mem::mock::set_handler(move |call_index, supplied_address, supplied_size, arg3| {
                assert_eq!(call_index, patina::management_mode::supervisor::SyscallIndex::MmIsCommBuffer.as_u64());
                assert_eq!(supplied_address, address);
                assert_eq!(supplied_size, capacity);
                assert_eq!(arg3, 0);
                1
            });
            let mut request = MmUserRequest {
                entry_context: EfiMmEntryContext {
                    mm_startup_this_ap: 0,
                    currently_executing_cpu: 0,
                    number_of_cpus: 1,
                    cpu_save_state_size: 0,
                    cpu_save_state: 0,
                },
                status: MmCommBufferStatus { is_comm_buffer_valid: 1, ..MmCommBufferStatus::new() },
                comm_buffer_address: address,
                comm_buffer_size: capacity,
            };

            let status = CORE.entry_point_worker::<BarePlatform>(
                UserCommandType::UserRequest as u64,
                core::ptr::from_mut(&mut request) as u64,
                size_of::<MmUserRequest>() as u64,
            );

            assert_eq!(status, efi::Status::SUCCESS.as_usize() as u64);
            assert_eq!(request.status.is_comm_buffer_valid, 0);
            assert_eq!(request.status.return_status, efi::Status::SUCCESS.as_usize() as u64);
            assert_eq!(request.status.return_buffer_size, response_size);
            assert_eq!(request.comm_buffer_address, address);
            assert_eq!(request.comm_buffer_size, capacity);
            assert_eq!(HANDLER_CALLS.load(Ordering::SeqCst), index as u64 + 1);
        }
    }

    #[test]
    fn test_user_request_rejects_an_unapproved_buffer_before_reading_it() {
        let core = init_core();
        core.mmi_db.register_internal_handler(counting_handler, None).expect("root handler registers");

        for address in [0, 0xDEAD_0000] {
            mm_mem::mock::set_handler(move |call_index, supplied_address, supplied_size, _| {
                assert_eq!(call_index, patina::management_mode::supervisor::SyscallIndex::MmIsCommBuffer.as_u64());
                assert_eq!(supplied_address, address);
                assert_eq!(supplied_size, 64);
                0
            });
            let mut request = MmUserRequest {
                entry_context: EfiMmEntryContext {
                    mm_startup_this_ap: 0,
                    currently_executing_cpu: 0,
                    number_of_cpus: 1,
                    cpu_save_state_size: 0,
                    cpu_save_state: 0,
                },
                status: MmCommBufferStatus { is_comm_buffer_valid: 1, ..MmCommBufferStatus::new() },
                comm_buffer_address: address,
                comm_buffer_size: 64,
            };

            let status = core.entry_point_worker::<BarePlatform>(
                UserCommandType::UserRequest as u64,
                core::ptr::from_mut(&mut request) as u64,
                size_of::<MmUserRequest>() as u64,
            );

            assert_eq!(status, efi::Status::SUCCESS.as_usize() as u64);
            assert_eq!(request.status.is_comm_buffer_valid, 0);
            assert_eq!(request.status.return_status, efi::Status::NOT_FOUND.as_usize() as u64);
            assert_eq!(request.status.return_buffer_size, 0);
        }
        assert_eq!(HANDLER_CALLS.load(Ordering::SeqCst), 2, "root handlers still run for each rejected request");
    }

    #[test]
    fn test_user_request_dispatches_root_handlers_and_preserves_the_status() {
        let core = init_core();
        core.mmi_db.register_internal_handler(counting_handler, None).expect("root handler registers");

        let context = EfiMmEntryContext {
            mm_startup_this_ap: 0,
            currently_executing_cpu: 2,
            number_of_cpus: 6,
            cpu_save_state_size: 0,
            cpu_save_state: 0,
        };
        let status =
            MmCommBufferStatus { is_comm_buffer_valid: 0, _padding: [0; 7], return_status: 7, return_buffer_size: 9 };
        let mut request = MmUserRequest { entry_context: context, status, comm_buffer_address: 0, comm_buffer_size: 0 };
        mm_mem::mock::set_handler(|_, _, _, _| panic!("an asynchronous request must not validate a comm buffer"));
        let table = core.init_mm_system_table();

        let result = core.entry_point_worker::<BarePlatform>(
            UserCommandType::UserRequest as u64,
            core::ptr::from_mut(&mut request) as u64,
            size_of::<MmUserRequest>() as u64,
        );

        assert_eq!(result, efi::Status::SUCCESS.as_usize() as u64);
        assert_eq!(HANDLER_CALLS.load(Ordering::SeqCst), 1, "the async root dispatch always runs");
        // The entry context is reflected into the system table for dispatched drivers.
        // SAFETY: the table is heap-allocated and lives for the process.
        unsafe {
            assert_eq!((*table).currently_executing_cpu, 2);
            assert_eq!((*table).number_of_cpus, 6);
        }
        // With no valid comm buffer the status block is written back untouched.
        let written = request.status;
        assert_eq!(written.return_status, 7);
        assert_eq!(written.return_buffer_size, 9);
    }

    #[test]
    fn test_synchronous_mmi_rejects_a_buffer_too_small_for_a_header() {
        let core = init_core();
        let buffer = legacy_comm_buffer(HANDLER_GUID, &[], EfiMmCommunicateHeader::size());
        let mut returned = 0;

        let status = core.dispatch_synchronous_mmi(
            buffer.as_ptr() as u64,
            (EfiMmCommunicateHeader::size() - 1) as u64,
            &mut returned,
        );

        assert_eq!(status, efi::Status::BAD_BUFFER_SIZE);
    }

    #[test]
    fn test_synchronous_mmi_rejects_a_message_longer_than_the_buffer() {
        let core = init_core();
        let total = EfiMmCommunicateHeader::size() + 8;
        // Claim a longer message than the buffer can hold.
        let buffer = legacy_comm_buffer(HANDLER_GUID, &[0xAA; 8], total);
        // SAFETY: the header occupies the first bytes of the buffer.
        unsafe {
            (*(buffer.as_ptr() as *mut EfiMmCommunicateHeader)).message_length = 0x1000;
        }
        let mut returned = 0;

        let status = core.dispatch_synchronous_mmi(buffer.as_ptr() as u64, total as u64, &mut returned);

        assert_eq!(status, efi::Status::BAD_BUFFER_SIZE);
    }

    #[test]
    fn test_synchronous_mmi_dispatches_the_message_guid() {
        let core = init_core();
        core.mmi_db.register_internal_handler(counting_handler, Some(&HANDLER_GUID)).expect("handler registers");

        let message = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let total = EfiMmCommunicateHeader::size() + 16;
        let buffer = legacy_comm_buffer(HANDLER_GUID, &message, total);
        let mut returned = 0;

        let status = core.dispatch_synchronous_mmi(buffer.as_ptr() as u64, total as u64, &mut returned);

        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(HANDLER_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(returned, (EfiMmCommunicateHeader::size() + message.len()) as u64);

        // The tail past the message is zeroed, matching the C implementation.
        // SAFETY: the buffer owns `total` bytes.
        let tail = unsafe {
            core::slice::from_raw_parts(
                buffer.as_ptr().cast::<u8>().add(EfiMmCommunicateHeader::size() + message.len()),
                total - EfiMmCommunicateHeader::size() - message.len(),
            )
        };
        assert!(tail.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn test_synchronous_mmi_reports_not_found_for_an_unhandled_guid() {
        let core = init_core();
        let total = EfiMmCommunicateHeader::size() + 8;
        let buffer = legacy_comm_buffer(HANDLER_GUID, &[0; 8], total);
        let mut returned = 0;

        let status = core.dispatch_synchronous_mmi(buffer.as_ptr() as u64, total as u64, &mut returned);

        assert_eq!(status, efi::Status::NOT_FOUND);
    }

    #[test]
    fn test_synchronous_mmi_rejects_a_response_larger_than_the_buffer() {
        let core = init_core();
        core.mmi_db.register_internal_handler(oversized_handler, Some(&HANDLER_GUID)).expect("handler registers");

        let total = EfiMmCommunicateHeader::size() + 16;
        let buffer = legacy_comm_buffer(HANDLER_GUID, &[0xAA; 8], total);
        let mut returned = 0;

        let status = core.dispatch_synchronous_mmi(buffer.as_ptr() as u64, total as u64, &mut returned);

        // The size travels to the non-MM caller, which uses it to read the response out of the
        // communication buffer, so a length it cannot hold is refused rather than shortened.
        assert_eq!(status, efi::Status::BAD_BUFFER_SIZE);
        assert_eq!(returned, 0);
    }

    #[test]
    fn test_synchronous_mmi_rejects_a_v3_header_that_does_not_fit() {
        let core = init_core();

        // A caller picks the V3 layout through a GUID that sits in the part both layouts share,
        // so a buffer holding only a legacy header can still select the larger V3 header. Reading
        // it would run past the end of the communication buffer.
        for total in [EfiMmCommunicateHeader::size(), v3_header_size() - 1] {
            let buffer = v3_comm_buffer(HANDLER_GUID, total, total as u64);
            let mut returned = 0;

            let status = core.dispatch_synchronous_mmi(buffer.as_ptr() as u64, total as u64, &mut returned);

            assert_eq!(status, efi::Status::BAD_BUFFER_SIZE, "a {total}-byte buffer was accepted as V3");
        }
    }

    #[test]
    fn test_synchronous_mmi_rejects_a_v3_total_outside_the_buffer() {
        let core = init_core();
        let total = v3_header_size() + 16;

        // Larger than the buffer, and smaller than the header it must account for.
        for claimed in [total as u64 + 1, u64::MAX, 0, v3_header_size() as u64 - 1] {
            let buffer = v3_comm_buffer(HANDLER_GUID, total, claimed);
            let mut returned = 0;

            let status = core.dispatch_synchronous_mmi(buffer.as_ptr() as u64, total as u64, &mut returned);

            assert_eq!(status, efi::Status::BAD_BUFFER_SIZE, "a claimed size of 0x{claimed:x} was accepted");
        }
    }

    #[test]
    fn test_synchronous_mmi_dispatches_a_valid_v3_message_guid() {
        let core = init_core();
        core.mmi_db.register_internal_handler(counting_handler, Some(&HANDLER_GUID)).expect("handler registers");

        let total = v3_header_size() + 16;
        let buffer = v3_comm_buffer(HANDLER_GUID, total, total as u64);
        let mut returned = 0;

        let status = core.dispatch_synchronous_mmi(buffer.as_ptr() as u64, total as u64, &mut returned);

        // The V3 branch dispatches on `message_guid`, not the header GUID that selected it.
        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(HANDLER_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(returned, total as u64);
    }

    #[test]
    fn test_synchronous_mmi_rejects_a_message_length_that_would_overflow() {
        let core = init_core();
        let total = EfiMmCommunicateHeader::size() + 8;
        let buffer = legacy_comm_buffer(HANDLER_GUID, &[0xAA; 8], total);

        // Adding the header size to this wraps, landing back inside the buffer and passing a
        // check written as `header + message > buffer`.
        // SAFETY: the header occupies the first bytes of the buffer.
        unsafe {
            (*(buffer.as_ptr() as *mut EfiMmCommunicateHeader)).message_length = usize::MAX;
        }
        let mut returned = 0;

        let status = core.dispatch_synchronous_mmi(buffer.as_ptr() as u64, total as u64, &mut returned);

        assert_eq!(status, efi::Status::BAD_BUFFER_SIZE);
    }

    #[test]
    fn test_dispatch_drivers_reports_no_work_when_nothing_was_discovered() {
        let core = init_core();

        assert_eq!(core.dispatch_drivers(), Ok(0));
    }
}
