//! MM Driver Dispatcher
//!
//! This module is responsible for discovering MM drivers from HOBs and dispatching them
//! in dependency order. It follows the same pattern as the C `StandaloneMmCore` dispatcher
//! in `FwVol.c` and `Dispatcher.c`, and the Rust DXE Core's `pi_dispatcher.rs`.
//!
//! ## Driver Discovery
//!
//! MM drivers are discovered from `MemoryAllocationModule` HOBs in the HOB list. Each driver
//! HOB is identified by having `alloc_descriptor.name == MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID`.
//! The HOB's `module_name` provides the driver GUID, and `entry_point` provides the address to call.
//!
//! Drivers that are the supervisor core or user core themselves are skipped.
//!
//! ## Depex Evaluation
//!
//! Each driver's `MemoryAllocationModule` HOB is followed by a `GuidHob` with
//! `name == MM_SUPERVISOR_DEPEX_HOB_GUID` containing the raw dependency expression bytes.
//! The depex is parsed and evaluated against the protocol database.
//!
//! ## Dispatch Order
//!
//! Drivers with satisfied dependencies (or `TRUE`/empty depex) are dispatched first.
//! `BEFORE`/`AFTER` associations are respected: if driver A has `BEFORE(B)`, A is
//! dispatched immediately before B.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::{collections::BTreeMap, format, string::String, vec::Vec};
use core::{cmp::Ordering, ffi::c_void};

use patina::standard::efi;
use patina::{
    c_ptr::CPtr,
    management_mode::supervisor::{
        MM_SUPERVISOR_CORE_GUID, MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_USER_GUID,
    },
    pi::hob::Hob,
};
use patina_internal_core::depex::{AssociatedDependency, Depex};
use spin::Mutex;

use crate::{DepexHobData, MM_SUPERVISOR_DEPEX_HOB_GUID, image_name, protocol_db::ProtocolDatabase};

/// Represents a discovered MM driver pending dispatch.
#[derive(Debug)]
struct DriverEntry {
    /// The GUID identifying this driver (from `MemoryAllocationModule.module_name`).
    file_name: efi::Guid,
    /// The module name recovered from the loaded image's debug metadata, if present.
    module_name: Option<String>,
    /// The entry point address of the driver.
    entry_point: u64,
    /// The base address of the driver image in memory.
    image_base: u64,
    /// The size of the driver image in memory.
    image_size: u64,
    /// The parsed dependency expression, if any.
    depex: Option<Depex>,
}

impl DriverEntry {
    /// Formats the driver for logging as `<Name.efi> [<GUID>]`, falling back to
    /// `[<GUID>]` when the image carries no debug metadata.
    fn describe(&self) -> String {
        let guid = patina::Guid::from_ref(&self.file_name);
        match self.module_name {
            Some(ref name) => format!("{name} [{guid}]"),
            None => format!("[{guid}]"),
        }
    }
}

/// Wrapper for `efi::Guid` that implements `Ord` for use in `BTreeMap`.
#[derive(Debug, Eq, PartialEq)]
struct OrdGuid(efi::Guid);

impl PartialOrd for OrdGuid {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrdGuid {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.as_bytes().cmp(other.0.as_bytes())
    }
}

/// The MM Driver Dispatcher.
///
/// Discovers drivers from HOBs at initialization time, evaluates their dependency
/// expressions, and dispatches them by calling their entry points.
pub struct MmDispatcher {
    /// Tracks whether the dispatcher is currently executing (prevents re-entrance).
    executing: Mutex<bool>,
    /// Drivers discovered from HOBs during `StartUserCore`, awaiting dispatch.
    pending: Mutex<Vec<DriverEntry>>,
}

impl Default for MmDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl MmDispatcher {
    /// Creates a new `MmDispatcher`.
    pub const fn new() -> Self {
        Self { executing: Mutex::new(false), pending: Mutex::new(Vec::new()) }
    }

    /// Discover drivers from HOBs and dispatch them.
    ///
    /// This is the main entry point called during `StartUserCore`. It:
    /// 1. Walks the HOB list to find `MemoryAllocationModule` HOBs with the supervisor alloc GUID
    /// 2. Skips the supervisor core and user core modules
    /// 3. Reads the paired depex `GuidHob` that follows each driver HOB
    /// 4. Evaluates dependencies and dispatches in order
    ///
    /// The discovered drivers are dispatched later by [`dispatch`](Self::dispatch)
    /// when the `MM_DISPATCH_EVENT` MMI is delivered.
    pub fn discover(&self, hob: &Hob<'_>) {
        let drivers = self.discover_drivers(hob);
        log::info!("Discovered {} MM driver(s) from HOBs.", drivers.len());
        *self.pending.lock() = drivers;
    }

    /// Dispatch the drivers recorded by [`discover`](Self::discover) in dependency order.
    ///
    /// Evaluates each pending driver's depex against `protocol_db` and calls the
    /// entry points of drivers whose dependencies are satisfied.
    ///
    /// Returns the number of drivers successfully dispatched, or an error status.
    pub fn dispatch(
        &self,
        protocol_db: &ProtocolDatabase,
        mm_system_table: *const c_void,
    ) -> Result<usize, efi::Status> {
        let mut is_executing = self.executing.lock();
        if *is_executing {
            log::error!("MM driver dispatch rejected: a dispatch pass is already running");
            return Err(efi::Status::ALREADY_STARTED);
        }
        *is_executing = true;
        drop(is_executing);

        let pending = core::mem::take(&mut *self.pending.lock());
        let dispatched = self.dispatch_drivers(pending, protocol_db, mm_system_table);

        *self.executing.lock() = false;
        Ok(dispatched)
    }

    /// Walk the HOB list and collect driver entries.
    ///
    /// For each `MemoryAllocationModule` HOB with the supervisor allocation GUID:
    /// - Skip if the module is the supervisor core or user core
    /// - Look at the next HOB for a depex `GuidHob` with `MM_SUPERVISOR_DEPEX_HOB_GUID`
    /// - Create a `DriverEntry` with the parsed depex
    fn discover_drivers(&self, hob: &Hob<'_>) -> Vec<DriverEntry> {
        let mut drivers = Vec::new();

        // Collect all HOBs into a vec for indexed access (we need to look ahead for depex)
        let all_hobs: Vec<Hob<'_>> = hob.into_iter().collect();

        for (index, current_hob) in all_hobs.iter().enumerate() {
            if let Hob::MemoryAllocationModule(mem_alloc_mod) = current_hob {
                // Check if this is an MM Supervisor module allocation
                if mem_alloc_mod.alloc_descriptor.name != MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID {
                    continue;
                }

                let module_name = mem_alloc_mod.module_name;

                // Skip the supervisor core and user core modules
                if module_name == MM_SUPERVISOR_CORE_GUID || module_name == MM_SUPERVISOR_USER_GUID {
                    log::debug!("Skipping core module: {module_name}");
                    continue;
                }

                let image_base = mem_alloc_mod.alloc_descriptor.memory_base_address;
                let image_size = mem_alloc_mod.alloc_descriptor.memory_length;

                // SAFETY: The supervisor loaded this image into MMRAM at `image_base` for
                // `image_size` bytes and keeps it mapped for the lifetime of MM.
                let image_module_name = unsafe { image_name::loaded_image_name(image_base, image_size) };

                // Look for a paired depex GuidHob in the next HOB
                let depex: Option<Depex> = if let Some(next_hob) = all_hobs.get(index + 1) {
                    if let Hob::GuidHob(guid_hob, data) = next_hob {
                        if guid_hob.name == MM_SUPERVISOR_DEPEX_HOB_GUID {
                            log::debug!("  Found depex HOB ({} bytes)", data.len());
                            if data.is_empty() {
                                None
                            } else {
                                // Check the name matches the expected depex HOB GUID before parsing.
                                let depex_hob_data = <[u8]>::as_ptr(data).cast::<DepexHobData>();
                                // SAFETY: We trust that the supervisor correctly formats the depex HOB data
                                let depex_hob_data = unsafe { &*depex_hob_data };
                                assert!(
                                    depex_hob_data.name == module_name,
                                    "Depex HOB module name {} does not match driver module name {module_name}",
                                    depex_hob_data.name
                                );
                                log::debug!(
                                    "  Parsed depex HOB {:p} for driver {} at {:p}: expression length = {}",
                                    depex_hob_data.as_ptr(),
                                    module_name,
                                    depex_hob_data.depex_expression.as_ptr(),
                                    depex_hob_data.depex_expression_size
                                );
                                // SAFETY: depex_expression is a zero-length array (flexible array member).
                                // The actual bytes follow the struct in memory; use from_raw_parts with the real size.
                                let depex_bytes = unsafe {
                                    core::slice::from_raw_parts(
                                        depex_hob_data.depex_expression.as_ptr(),
                                        depex_hob_data.depex_expression_size as usize,
                                    )
                                };
                                Some(Depex::from(depex_bytes))
                            }
                        } else {
                            log::debug!("  No depex HOB (next HOB has different GUID)");
                            None
                        }
                    } else {
                        log::debug!("  No depex HOB (next HOB is not GuidHob)");
                        None
                    }
                } else {
                    log::debug!("  No depex HOB (no next HOB)");
                    None
                };

                log::debug!("  Driver {module_name} has depex: {depex:?}");

                let driver = DriverEntry {
                    file_name: module_name.into_inner(),
                    module_name: image_module_name,
                    entry_point: mem_alloc_mod.entry_point,
                    image_base,
                    image_size,
                    depex,
                };

                log::info!(
                    "Found MM driver {}: entry=0x{:016x}, base=0x{:016x}, size=0x{:x}",
                    driver.describe(),
                    driver.entry_point,
                    driver.image_base,
                    driver.image_size,
                );

                drivers.push(driver);
            }
        }

        drivers
    }

    /// Dispatch drivers in dependency order.
    ///
    /// This implements a multi-pass dispatch loop similar to the DXE Core's `PiDispatcher`:
    /// 1. Evaluate each pending driver's depex against the current protocol database
    /// 2. Drivers with satisfied (or absent) depexes are scheduled
    /// 3. Before/After associations are handled by reordering the scheduled list
    /// 4. Each scheduled driver's entry point is called
    /// 5. Repeat until no more drivers can be dispatched
    fn dispatch_drivers(
        &self,
        mut pending: Vec<DriverEntry>,
        protocol_db: &ProtocolDatabase,
        mm_system_table: *const c_void,
    ) -> usize {
        let mut total_dispatched = 0;

        loop {
            // The protocol DB is shared with the MM System Table thunks, so it already
            // reflects every protocol installed by previously dispatched drivers.
            let registered_protocols = protocol_db.registered_protocols();
            let mut scheduled = Vec::new();
            let mut still_pending = Vec::new();
            let mut associated_before: BTreeMap<OrdGuid, Vec<DriverEntry>> = BTreeMap::new();
            let mut associated_after: BTreeMap<OrdGuid, Vec<DriverEntry>> = BTreeMap::new();

            for mut driver in pending.drain(..) {
                let depex_satisfied = match driver.depex {
                    Some(ref mut depex) => depex.eval(&registered_protocols),
                    // No depex means the driver can be dispatched immediately
                    None => true,
                };

                if depex_satisfied {
                    scheduled.push(driver);
                } else {
                    // Check for Before/After associations
                    match driver.depex.as_ref().map(patina_internal_core::depex::Depex::is_associated) {
                        Some(Some(AssociatedDependency::Before(guid))) => {
                            associated_before.entry(OrdGuid(guid)).or_default().push(driver);
                        }
                        Some(Some(AssociatedDependency::After(guid))) => {
                            associated_after.entry(OrdGuid(guid)).or_default().push(driver);
                        }
                        _ => {
                            still_pending.push(driver);
                        }
                    }
                }
            }

            if scheduled.is_empty() {
                // No more drivers can be dispatched; move remaining to pending for logging
                pending = still_pending;
                break;
            }

            // Build the final dispatch order respecting Before/After associations
            let ordered: Vec<DriverEntry> = scheduled
                .into_iter()
                .flat_map(|driver| {
                    let filename = OrdGuid(driver.file_name);
                    let mut list = associated_before.remove(&filename).unwrap_or_default();
                    let mut after_list = associated_after.remove(&filename).unwrap_or_default();
                    list.push(driver);
                    list.append(&mut after_list);
                    list
                })
                .collect();

            // Dispatch each scheduled driver
            for driver in ordered {
                let description = driver.describe();
                log::info!("Dispatching MM driver {description} at entry 0x{:016x}", driver.entry_point);

                // Call the driver's entry point.
                // MM driver entry signature: EFI_STATUS EFIAPI DriverEntry(EFI_HANDLE ImageHandle, EFI_MM_SYSTEM_TABLE *MmSystemTable)
                // We pass a null image handle and the system table pointer.
                type MmDriverEntryPoint = unsafe extern "efiapi" fn(efi::Handle, *const c_void) -> efi::Status;
                // SAFETY: The entry point address came from the driver's `MemoryAllocationModule`
                // HOB, which the supervisor produced, and matches the MM driver ABI.
                let entry_fn: MmDriverEntryPoint = unsafe { core::mem::transmute(driver.entry_point) };

                // SAFETY: As above; the driver owns the contract for its own entry point.
                let status = unsafe { entry_fn(core::ptr::null_mut(), mm_system_table) };

                if status == efi::Status::SUCCESS {
                    log::info!("  Driver {description} returned SUCCESS.");
                    total_dispatched += 1;
                } else {
                    log::warn!("  Driver {description} returned status: {status:?} (0x{:x})", status.as_usize());
                }
            }

            // Remaining unmatched Before/After drivers go back to pending
            for (_guid, drivers) in associated_before {
                still_pending.extend(drivers);
            }

            for (_guid, drivers) in associated_after {
                still_pending.extend(drivers);
            }

            pending = still_pending;
        }

        // Log any remaining drivers
        for driver in &pending {
            log::warn!(
                "Driver {} discovered but not dispatched (unsatisfied depex: {:?}).",
                driver.describe(),
                driver.depex,
            );
        }

        total_dispatched
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};

    use patina::pi::hob::{
        END_OF_HOB_LIST, GUID_EXTENSION, GuidHob, HANDOFF, HobHeader, MEMORY_ALLOCATION, MemoryAllocationHeader,
        MemoryAllocationModule, PhaseHandoffInformationTable,
    };

    const DRIVER_A: patina::BinaryGuid = patina::BinaryGuid::from_string("0a000000-0000-0000-0000-00000000000a");
    const DRIVER_B: patina::BinaryGuid = patina::BinaryGuid::from_string("0b000000-0000-0000-0000-00000000000b");
    const OTHER_ALLOC: patina::BinaryGuid = patina::BinaryGuid::from_string("0c000000-0000-0000-0000-00000000000c");
    const MISSING_PROTOCOL: patina::BinaryGuid =
        patina::BinaryGuid::from_string("0d000000-0000-0000-0000-00000000000d");

    /// Records the order in which driver entry points ran, keyed by the value each one reports.
    static DISPATCH_LOG: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    static FAILING_CALLS: AtomicUsize = AtomicUsize::new(0);
    static REENTRANT_RESULT: AtomicU64 = AtomicU64::new(0);

    extern "efiapi" fn driver_a(_handle: efi::Handle, _mmst: *const c_void) -> efi::Status {
        DISPATCH_LOG.lock().push(0xA);
        efi::Status::SUCCESS
    }

    extern "efiapi" fn driver_b(_handle: efi::Handle, _mmst: *const c_void) -> efi::Status {
        DISPATCH_LOG.lock().push(0xB);
        efi::Status::SUCCESS
    }

    extern "efiapi" fn failing_driver(_handle: efi::Handle, _mmst: *const c_void) -> efi::Status {
        FAILING_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
        efi::Status::UNSUPPORTED
    }

    /// Calls back into the dispatcher that is currently running it.
    extern "efiapi" fn reentrant_driver(_handle: efi::Handle, _mmst: *const c_void) -> efi::Status {
        let status = match REENTRANT_DISPATCHER.dispatch(&ProtocolDatabase::new(), core::ptr::null()) {
            Ok(_) => efi::Status::SUCCESS,
            Err(status) => status,
        };
        REENTRANT_RESULT.store(status.as_usize() as u64, AtomicOrdering::SeqCst);
        efi::Status::SUCCESS
    }

    static REENTRANT_DISPATCHER: MmDispatcher = MmDispatcher::new();

    fn entry_of(driver: extern "efiapi" fn(efi::Handle, *const c_void) -> efi::Status) -> u64 {
        driver as usize as u64
    }

    /// Depex bytes for an unconditional `TRUE`.
    fn depex_true() -> Vec<u8> {
        vec![0x06, 0x08]
    }

    /// Depex bytes requiring `guid` to be present in the protocol database.
    fn depex_push(guid: patina::BinaryGuid) -> Vec<u8> {
        let mut bytes = vec![0x02];
        bytes.extend_from_slice(guid.into_inner().as_bytes());
        bytes.push(0x08);
        bytes
    }

    /// Depex bytes for `BEFORE(guid)` (`0x00`) or `AFTER(guid)` (`0x01`).
    fn depex_association(opcode: u8, guid: patina::BinaryGuid) -> Vec<u8> {
        let mut bytes = vec![opcode];
        bytes.extend_from_slice(guid.into_inner().as_bytes());
        bytes.push(0x08);
        bytes
    }

    /// Builds the `MM_SUPV_DEPEX_HOB_DATA` payload: the owning module GUID, the expression
    /// length, then the expression bytes.
    fn depex_payload(module: patina::BinaryGuid, expression: &[u8]) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(module.into_inner().as_bytes());
        payload.extend_from_slice(&(expression.len() as u64).to_le_bytes());
        payload.extend_from_slice(expression);
        payload
    }

    /// Builds a contiguous PI HOB list rooted at a PHIT.
    ///
    /// The HOB iterator walks raw memory from one header to the next, so entries must be laid
    /// out consecutively with every length a multiple of eight to keep the next header aligned.
    struct HobListBuffer {
        bytes: Vec<u8>,
        aligned: Vec<u64>,
        /// Backing storage for the module images the HOBs point at, so that image-name
        /// resolution reads real memory instead of a fabricated address.
        images: Vec<Vec<u8>>,
    }

    impl HobListBuffer {
        fn new() -> Self {
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

            let mut this = Self { bytes: Vec::new(), aligned: Vec::new(), images: Vec::new() };
            this.push_struct(&phit);
            this
        }

        fn push_struct<T>(&mut self, value: &T) {
            // SAFETY: every HOB structure written here is `repr(C)` over integers, GUIDs and
            // `repr(u32)` enums, so all of its bytes are initialized.
            self.bytes.extend_from_slice(unsafe {
                core::slice::from_raw_parts(core::ptr::from_ref(value).cast::<u8>(), size_of::<T>())
            });
        }

        /// Appends a driver module HOB. `alloc_name` selects whether the supervisor recognizes it.
        fn module(mut self, alloc_name: patina::BinaryGuid, module_name: patina::BinaryGuid, entry_point: u64) -> Self {
            // Back the HOB with a real allocation. The heap buffer a `Vec` owns does not move
            // when the `Vec` itself is moved, so the recorded address stays valid.
            const IMAGE_SIZE: usize = 0x2000;
            self.images.push(vec![0u8; IMAGE_SIZE]);
            let image_base = self.images.last().expect("image was just pushed").as_ptr() as u64;

            let module = MemoryAllocationModule {
                header: HobHeader {
                    r#type: MEMORY_ALLOCATION,
                    length: size_of::<MemoryAllocationModule>() as u16,
                    reserved: 0,
                },
                alloc_descriptor: MemoryAllocationHeader {
                    name: alloc_name,
                    memory_base_address: image_base,
                    memory_length: IMAGE_SIZE as u64,
                    memory_type: efi::BOOT_SERVICES_CODE,
                    reserved: [0; 4],
                },
                module_name,
                entry_point,
            };
            self.push_struct(&module);
            self
        }

        fn guid_hob(mut self, guid: patina::BinaryGuid, payload: &[u8]) -> Self {
            let length = (size_of::<GuidHob>() + payload.len()).next_multiple_of(8);
            let header = GuidHob {
                header: HobHeader { r#type: GUID_EXTENSION, length: length as u16, reserved: 0 },
                name: guid,
            };
            self.push_struct(&header);
            self.bytes.extend_from_slice(payload);
            let padded = self.bytes.len() - payload.len() - size_of::<GuidHob>() + length;
            self.bytes.resize(padded, 0);
            self
        }

        fn build(mut self) -> Self {
            let end = HobHeader { r#type: END_OF_HOB_LIST, length: size_of::<HobHeader>() as u16, reserved: 0 };
            self.push_struct(&end);

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

        fn hob(&self) -> Hob<'_> {
            // SAFETY: `build` wrote a well-formed PHIT at the start of the aligned buffer.
            Hob::Handoff(unsafe { &*self.aligned.as_ptr().cast::<PhaseHandoffInformationTable>() })
        }
    }

    fn dispatch_order() -> Vec<u64> {
        DISPATCH_LOG.lock().clone()
    }

    #[test]
    fn test_dispatch_without_discovery_does_nothing() {
        let dispatcher = MmDispatcher::default();

        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(0));
    }

    #[test]
    fn test_discovered_driver_entry_point_is_called() {
        let hobs = HobListBuffer::new()
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_A, entry_of(driver_a))
            .build();
        let dispatcher = MmDispatcher::new();

        dispatcher.discover(&hobs.hob());

        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(1));
        assert_eq!(dispatch_order(), [0xA]);
    }

    #[test]
    fn test_discovery_ignores_allocations_from_another_producer() {
        let hobs = HobListBuffer::new().module(OTHER_ALLOC, DRIVER_A, entry_of(driver_a)).build();
        let dispatcher = MmDispatcher::new();

        dispatcher.discover(&hobs.hob());

        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(0));
        assert!(dispatch_order().is_empty());
    }

    #[test]
    fn test_discovery_skips_the_supervisor_and_user_cores() {
        let hobs = HobListBuffer::new()
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_CORE_GUID, entry_of(driver_a))
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_USER_GUID, entry_of(driver_a))
            .build();
        let dispatcher = MmDispatcher::new();

        dispatcher.discover(&hobs.hob());

        // The cores are already running; dispatching them again would re-enter the foundation.
        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(0));
        assert!(dispatch_order().is_empty());
    }

    #[test]
    fn test_a_satisfied_depex_allows_dispatch() {
        let hobs = HobListBuffer::new()
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_A, entry_of(driver_a))
            .guid_hob(MM_SUPERVISOR_DEPEX_HOB_GUID, &depex_payload(DRIVER_A, &depex_true()))
            .build();
        let dispatcher = MmDispatcher::new();

        dispatcher.discover(&hobs.hob());

        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(1));
        assert_eq!(dispatch_order(), [0xA]);
    }

    #[test]
    fn test_an_unsatisfied_depex_holds_the_driver_back() {
        crate::test_support::init_test_logger();
        let hobs = HobListBuffer::new()
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_A, entry_of(driver_a))
            .guid_hob(MM_SUPERVISOR_DEPEX_HOB_GUID, &depex_payload(DRIVER_A, &depex_push(MISSING_PROTOCOL)))
            .build();
        let dispatcher = MmDispatcher::new();

        dispatcher.discover(&hobs.hob());

        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(0));
        assert!(dispatch_order().is_empty(), "a driver must not run before its protocol exists");
    }

    #[test]
    fn test_a_depex_is_satisfied_once_the_protocol_is_installed() {
        let hobs = HobListBuffer::new()
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_A, entry_of(driver_a))
            .guid_hob(MM_SUPERVISOR_DEPEX_HOB_GUID, &depex_payload(DRIVER_A, &depex_push(MISSING_PROTOCOL)))
            .build();
        let dispatcher = MmDispatcher::new();
        let protocol_db = ProtocolDatabase::new();
        protocol_db
            .install_protocol(core::ptr::null_mut(), &MISSING_PROTOCOL.into_inner(), core::ptr::null_mut())
            .expect("protocol installs");

        dispatcher.discover(&hobs.hob());

        assert_eq!(dispatcher.dispatch(&protocol_db, core::ptr::null()), Ok(1));
        assert_eq!(dispatch_order(), [0xA]);
    }

    #[test]
    fn test_an_empty_depex_expression_is_treated_as_no_dependency() {
        let hobs = HobListBuffer::new()
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_A, entry_of(driver_a))
            .guid_hob(MM_SUPERVISOR_DEPEX_HOB_GUID, &[])
            .build();
        let dispatcher = MmDispatcher::new();

        dispatcher.discover(&hobs.hob());

        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(1));
    }

    #[test]
    fn test_a_trailing_hob_that_is_not_a_depex_is_ignored() {
        let unrelated = patina::BinaryGuid::from_string("0e000000-0000-0000-0000-00000000000e");
        let hobs = HobListBuffer::new()
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_A, entry_of(driver_a))
            .guid_hob(unrelated, &[0xFF; 8])
            .build();
        let dispatcher = MmDispatcher::new();

        dispatcher.discover(&hobs.hob());

        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(1));
    }

    #[test]
    fn test_a_module_followed_by_another_module_has_no_depex() {
        let hobs = HobListBuffer::new()
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_A, entry_of(driver_a))
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_B, entry_of(driver_b))
            .build();
        let dispatcher = MmDispatcher::new();

        dispatcher.discover(&hobs.hob());

        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(2));
        assert_eq!(dispatch_order(), [0xA, 0xB]);
    }

    #[test]
    fn test_before_association_runs_the_driver_ahead_of_its_target() {
        // B declares BEFORE(A), so it must run first even though A is discovered first.
        let hobs = HobListBuffer::new()
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_A, entry_of(driver_a))
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_B, entry_of(driver_b))
            .guid_hob(MM_SUPERVISOR_DEPEX_HOB_GUID, &depex_payload(DRIVER_B, &depex_association(0x00, DRIVER_A)))
            .build();
        let dispatcher = MmDispatcher::new();

        dispatcher.discover(&hobs.hob());

        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(2));
        assert_eq!(dispatch_order(), [0xB, 0xA]);
    }

    #[test]
    fn test_after_association_runs_the_driver_behind_its_target() {
        let hobs = HobListBuffer::new()
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_B, entry_of(driver_b))
            .guid_hob(MM_SUPERVISOR_DEPEX_HOB_GUID, &depex_payload(DRIVER_B, &depex_association(0x01, DRIVER_A)))
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_A, entry_of(driver_a))
            .build();
        let dispatcher = MmDispatcher::new();

        dispatcher.discover(&hobs.hob());

        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(2));
        assert_eq!(dispatch_order(), [0xA, 0xB]);
    }

    #[test]
    fn test_driver_entry_describe_uses_the_module_name_when_known() {
        let guid = DRIVER_A.into_inner();
        let named = DriverEntry {
            file_name: guid,
            module_name: Some(String::from("VariableStandaloneMm.efi")),
            entry_point: 0x1000,
            image_base: 0x2000,
            image_size: 0x3000,
            depex: None,
        };
        let anonymous = DriverEntry { module_name: None, ..named_clone(&named) };

        assert!(named.describe().starts_with("VariableStandaloneMm.efi ["));
        assert!(anonymous.describe().starts_with('['));
    }

    /// Clones the scalar fields of `entry`; `Depex` is not `Clone`, so it is dropped.
    fn named_clone(entry: &DriverEntry) -> DriverEntry {
        DriverEntry {
            file_name: entry.file_name,
            module_name: entry.module_name.clone(),
            entry_point: entry.entry_point,
            image_base: entry.image_base,
            image_size: entry.image_size,
            depex: None,
        }
    }

    #[test]
    fn test_a_driver_that_fails_is_still_consumed() {
        crate::test_support::init_test_logger();
        let hobs = HobListBuffer::new()
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_A, entry_of(failing_driver))
            .build();
        let dispatcher = MmDispatcher::new();

        dispatcher.discover(&hobs.hob());

        // The driver ran but reported failure, so it is not counted and not retried.
        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(0));
        assert_eq!(FAILING_CALLS.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(0));
        assert_eq!(FAILING_CALLS.load(AtomicOrdering::SeqCst), 1, "a dispatched driver is never re-run");
    }

    #[test]
    fn test_dispatch_refuses_to_re_enter_itself() {
        let hobs = HobListBuffer::new()
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_A, entry_of(reentrant_driver))
            .build();

        REENTRANT_DISPATCHER.discover(&hobs.hob());

        assert_eq!(REENTRANT_DISPATCHER.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(1));
        assert_eq!(
            REENTRANT_RESULT.load(AtomicOrdering::SeqCst),
            efi::Status::ALREADY_STARTED.as_usize() as u64,
            "a driver calling back into dispatch is rejected rather than recursing"
        );
    }

    #[test]
    fn test_ord_guid_orders_by_raw_bytes() {
        let a = OrdGuid(DRIVER_A.into_inner());
        let b = OrdGuid(DRIVER_B.into_inner());

        assert_eq!(a.cmp(&b), a.0.as_bytes().cmp(b.0.as_bytes()));
        assert_eq!(a.partial_cmp(&b), Some(a.cmp(&b)));
        assert_eq!(OrdGuid(DRIVER_A.into_inner()), a);
    }
}
