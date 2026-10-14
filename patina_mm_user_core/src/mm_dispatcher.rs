//! MM Driver Dispatcher
//!
//! Discovers MM drivers from HOBs and dispatches them in dependency order. It follows the
//! C `StandaloneMmCore` dispatcher (`FwVol.c` and `Dispatcher.c`) and the Rust DXE Core's
//! `pi_dispatcher.rs`.
//!
//! ## Driver Discovery
//!
//! Drivers come from `MemoryAllocationModule` HOBs whose
//! `alloc_descriptor.name` is `MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID`. The HOB's
//! `module_name` is the driver GUID and `entry_point` is the address to call. The
//! supervisor core and the user core are skipped.
//!
//! ## Depex Evaluation
//!
//! A driver HOB may be followed by a `GuidHob` named `MM_SUPERVISOR_DEPEX_HOB_GUID` that holds
//! the raw dependency expression. The depex is evaluated against the protocol database.
//!
//! ## Dispatch Order
//!
//! Drivers whose depex is satisfied (or absent) are dispatched first. `BEFORE` and `AFTER`
//! associations are honored: a driver with `BEFORE(B)` runs immediately before B.
//! Discovery happens during `StartUserCore`; dispatch waits for the
//! `MM_DISPATCH_EVENT_GROUP_GUID` MMI.
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
    management_mode::supervisor::{
        MM_SUPERVISOR_CORE_GUID, MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_USER_GUID,
    },
    pi::hob::Hob,
};
use patina_internal_core::depex::{AssociatedDependency, Depex};
use spin::Mutex;

use crate::{image_name, protocol_db::ProtocolDatabase};

/// GUID of the depex HOB that follows each driver `MemoryAllocationModule` HOB
/// (`gMmSupervisorDepexHobGuid`).
pub const MM_SUPERVISOR_DEPEX_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("b17f0049-affd-4530-acd6-e245e19deaf1");

/// Mirrors the `MM_SUPV_DEPEX_HOB_DATA` structure defined in the supervisor.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DepexHobData {
    /// File GUID of the driver the dependency expression belongs to.
    pub name: patina::BinaryGuid,
    /// Size in bytes of the dependency expression that follows.
    pub depex_expression_size: u64,
    /// Variable-length dependency expression bytes (flexible array member).
    pub depex_expression: [u8; 0],
}

/// A discovered MM driver waiting for dispatch.
#[derive(Debug)]
struct DriverEntry {
    /// The driver GUID (`MemoryAllocationModule.module_name`).
    file_name: efi::Guid,
    /// The module name from the image's debug metadata, if present.
    module_name: Option<String>,
    /// The driver's entry point address.
    entry_point: u64,
    /// The base address of the driver image in memory.
    image_base: u64,
    /// The size of the driver image in memory.
    image_size: u64,
    /// The parsed dependency expression, if any.
    depex: Option<Depex>,
}

impl DriverEntry {
    /// Formats the driver for logging as `<Name.efi> [<GUID>]`, or `[<GUID>]` when the image
    /// has no debug metadata.
    fn describe(&self) -> String {
        let guid = patina::Guid::from_ref(&self.file_name);
        match self.module_name {
            Some(ref name) => format!("{name} [{guid}]"),
            None => format!("[{guid}]"),
        }
    }
}

/// Wraps `efi::Guid` with a byte-wise `Ord` so it can key a `BTreeMap`.
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

/// Why a depex HOB payload could not be used.
#[derive(Debug, PartialEq, Eq)]
enum DepexHobError {
    /// The payload is shorter than its header, or than the expression size it records.
    Truncated,
    /// The payload names a different driver than the module HOB before it.
    WrongDriver(patina::BinaryGuid),
}

/// Returns the dependency expression in a `DepexHobData` payload that belongs to `module_name`.
///
/// An empty payload means the driver has no dependency expression. The payload is read byte
/// by byte, so it needs no particular alignment, and every length is checked against `data`.
fn depex_bytes_from_hob(data: &[u8], module_name: patina::BinaryGuid) -> Result<Option<&[u8]>, DepexHobError> {
    const NAME_OFFSET: usize = core::mem::offset_of!(DepexHobData, name);
    const SIZE_OFFSET: usize = core::mem::offset_of!(DepexHobData, depex_expression_size);
    const EXPRESSION_OFFSET: usize = core::mem::offset_of!(DepexHobData, depex_expression);

    if data.is_empty() {
        return Ok(None);
    }

    let name_bytes = data.get(NAME_OFFSET..SIZE_OFFSET).and_then(|bytes| bytes.try_into().ok());
    let name = patina::BinaryGuid::from_bytes(name_bytes.ok_or(DepexHobError::Truncated)?);
    if name != module_name {
        return Err(DepexHobError::WrongDriver(name));
    }

    let size_bytes = data.get(SIZE_OFFSET..EXPRESSION_OFFSET).and_then(|bytes| bytes.try_into().ok());
    let size = u64::from_le_bytes(size_bytes.ok_or(DepexHobError::Truncated)?);
    let end = usize::try_from(size)
        .ok()
        .and_then(|size| EXPRESSION_OFFSET.checked_add(size))
        .ok_or(DepexHobError::Truncated)?;

    data.get(EXPRESSION_OFFSET..end).map(Some).ok_or(DepexHobError::Truncated)
}

/// The MM Driver Dispatcher.
///
/// Discovers drivers from HOBs, evaluates their dependency expressions, and calls their
/// entry points.
pub struct MmDispatcher {
    /// Set while a dispatch pass runs, to reject re-entry.
    executing: Mutex<bool>,
    /// Drivers discovered during `StartUserCore` and not yet dispatched.
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

    /// Discovers the drivers in the HOB list and records them for [`dispatch`](Self::dispatch).
    ///
    /// Called during `StartUserCore`. Replaces any drivers recorded earlier. They are
    /// dispatched when the `MM_DISPATCH_EVENT_GROUP_GUID` MMI arrives.
    ///
    /// ## Panics
    ///
    /// Panics if a depex HOB names a different driver than the module HOB before it.
    pub fn discover(&self, hob: &Hob<'_>) {
        let drivers = self.discover_drivers(hob);
        log::info!("Discovered {} MM driver(s) from HOBs.", drivers.len());
        *self.pending.lock() = drivers;
    }

    /// Dispatches the drivers recorded by [`discover`](Self::discover) in dependency order.
    ///
    /// Each pending driver's depex is evaluated against `protocol_db`, and the drivers whose
    /// dependencies are met are called. Returns the number of drivers that returned success.
    /// Drivers whose dependencies are never met are logged and dropped.
    ///
    /// ## Errors
    ///
    /// Returns `ALREADY_STARTED` if a dispatch pass is already running, for example when a
    /// driver entry point calls back into this function.
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

    /// Walks the HOB list and collects the driver entries.
    ///
    /// For each `MemoryAllocationModule` HOB with the supervisor allocation GUID, other than
    /// the supervisor and user cores, the next HOB is read as the driver's depex if it is a
    /// `GuidHob` named `MM_SUPERVISOR_DEPEX_HOB_GUID`.
    fn discover_drivers(&self, hob: &Hob<'_>) -> Vec<DriverEntry> {
        let mut drivers = Vec::new();

        // Collect the HOBs so the depex HOB after each driver can be read by index.
        let all_hobs: Vec<Hob<'_>> = hob.into_iter().collect();

        for (index, current_hob) in all_hobs.iter().enumerate() {
            if let Hob::MemoryAllocationModule(mem_alloc_mod) = current_hob {
                if mem_alloc_mod.alloc_descriptor.name != MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID {
                    continue;
                }

                let module_name = mem_alloc_mod.module_name;

                if module_name == MM_SUPERVISOR_CORE_GUID || module_name == MM_SUPERVISOR_USER_GUID {
                    log::debug!("Skipping core module: {module_name}");
                    continue;
                }

                let image_base = mem_alloc_mod.alloc_descriptor.memory_base_address;
                let image_size = mem_alloc_mod.alloc_descriptor.memory_length;

                // SAFETY: The supervisor loaded this image into MMRAM at `image_base` for
                // `image_size` bytes and keeps it mapped for the lifetime of MM.
                let image_module_name = unsafe { image_name::loaded_image_name(image_base, image_size) };

                let depex: Option<Depex> = if let Some(next_hob) = all_hobs.get(index + 1) {
                    if let Hob::GuidHob(guid_hob, data) = next_hob {
                        if guid_hob.name == MM_SUPERVISOR_DEPEX_HOB_GUID {
                            log::debug!("  Found depex HOB ({} bytes)", data.len());
                            match depex_bytes_from_hob(data, module_name) {
                                Ok(Some(depex_bytes)) => {
                                    log::debug!(
                                        "  Parsed depex HOB for driver {module_name}: expression length = {}",
                                        depex_bytes.len()
                                    );
                                    Some(Depex::from(depex_bytes))
                                }
                                Ok(None) => None,
                                Err(err) => {
                                    log::error!("Skipping driver {module_name}: invalid depex HOB: {err:?}");
                                    continue;
                                }
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

    /// Dispatches drivers in dependency order.
    ///
    /// A multi-pass loop like the DXE Core's `PiDispatcher`. Each pass evaluates every
    /// pending depex against the current protocol database, orders the ready drivers around
    /// their `BEFORE` and `AFTER` associates, and calls their entry points. Passes repeat until
    /// one dispatches nothing.
    fn dispatch_drivers(
        &self,
        mut pending: Vec<DriverEntry>,
        protocol_db: &ProtocolDatabase,
        mm_system_table: *const c_void,
    ) -> usize {
        let mut total_dispatched = 0;

        loop {
            // The protocol database is shared with the MM System Table thunks, so it reflects
            // every protocol installed by drivers dispatched so far.
            let registered_protocols = protocol_db.registered_protocols();
            let mut scheduled = Vec::new();
            let mut still_pending = Vec::new();
            let mut associated_before: BTreeMap<OrdGuid, Vec<DriverEntry>> = BTreeMap::new();
            let mut associated_after: BTreeMap<OrdGuid, Vec<DriverEntry>> = BTreeMap::new();

            for mut driver in pending.drain(..) {
                let depex_satisfied = match driver.depex {
                    Some(ref mut depex) => depex.eval(&registered_protocols),
                    // No depex: the driver can run now.
                    None => true,
                };

                if depex_satisfied {
                    scheduled.push(driver);
                } else {
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
                // Nothing more can run. Keep the rest for the log below.
                pending = still_pending;
                break;
            }

            // Place BEFORE drivers ahead of their target and AFTER drivers behind it.
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

            for driver in ordered {
                let description = driver.describe();
                log::info!("Dispatching MM driver {description} at entry 0x{:016x}", driver.entry_point);

                // MM driver entry point:
                // EFI_STATUS EFIAPI DriverEntry(EFI_HANDLE ImageHandle, EFI_MM_SYSTEM_TABLE *MmSystemTable)
                // The image handle is passed as null.
                type MmDriverEntryPoint = unsafe extern "efiapi" fn(efi::Handle, *const c_void) -> efi::Status;
                // SAFETY: The entry point address came from the driver's `MemoryAllocationModule`
                // HOB, which the supervisor produced, and matches the MM driver ABI.
                let entry_fn: MmDriverEntryPoint = unsafe { core::mem::transmute(driver.entry_point) };

                // SAFETY: As above. The driver owns the contract for its own entry point.
                let status = unsafe { entry_fn(core::ptr::null_mut(), mm_system_table) };

                if status == efi::Status::SUCCESS {
                    log::info!("  Driver {description} returned SUCCESS.");
                    total_dispatched += 1;
                } else {
                    log::warn!("  Driver {description} returned status: {status:?} (0x{:x})", status.as_usize());
                }
            }

            // BEFORE and AFTER drivers whose target did not run go back to pending.
            for (_guid, drivers) in associated_before {
                still_pending.extend(drivers);
            }

            for (_guid, drivers) in associated_after {
                still_pending.extend(drivers);
            }

            pending = still_pending;
        }

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

    /// Depex bytes for `BEFORE(guid)` (opcode 0x00) or `AFTER(guid)` (opcode 0x01).
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
    fn test_a_depex_hob_shorter_than_its_header_skips_the_driver() {
        let hobs = HobListBuffer::new()
            .module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, DRIVER_A, entry_of(driver_a))
            .guid_hob(MM_SUPERVISOR_DEPEX_HOB_GUID, &DRIVER_A.into_inner().as_bytes()[..8])
            .build();
        let dispatcher = MmDispatcher::new();

        dispatcher.discover(&hobs.hob());

        assert_eq!(dispatcher.dispatch(&ProtocolDatabase::new(), core::ptr::null()), Ok(0));
    }

    #[test]
    fn test_depex_bytes_from_hob_rejects_bad_payloads() {
        let expression = depex_true();
        let valid = depex_payload(DRIVER_A, &expression);
        assert_eq!(depex_bytes_from_hob(&valid, DRIVER_A), Ok(Some(expression.as_slice())));
        assert_eq!(depex_bytes_from_hob(&[], DRIVER_A), Ok(None));

        // The recorded size runs past the end of the payload.
        let mut too_long = valid.clone();
        too_long[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(depex_bytes_from_hob(&too_long, DRIVER_A), Err(DepexHobError::Truncated));
        assert_eq!(depex_bytes_from_hob(&valid[..20], DRIVER_A), Err(DepexHobError::Truncated));
        assert_eq!(depex_bytes_from_hob(&valid[..valid.len() - 1], DRIVER_A), Err(DepexHobError::Truncated));

        assert_eq!(depex_bytes_from_hob(&valid, DRIVER_B), Err(DepexHobError::WrongDriver(DRIVER_A)));
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
        let guid_a = OrdGuid(DRIVER_A.into_inner());
        let guid_b = OrdGuid(DRIVER_B.into_inner());

        assert_eq!(guid_a.cmp(&guid_b), guid_a.0.as_bytes().cmp(guid_b.0.as_bytes()));
        assert_eq!(guid_a.partial_cmp(&guid_b), Some(guid_a.cmp(&guid_b)));
        assert_eq!(OrdGuid(DRIVER_A.into_inner()), guid_a);
    }
}
