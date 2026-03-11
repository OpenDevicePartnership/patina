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
