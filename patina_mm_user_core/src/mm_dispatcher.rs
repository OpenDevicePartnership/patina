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

use alloc::{collections::BTreeMap, vec::Vec};
use core::{cmp::Ordering, ffi::c_void};

use patina::{
    boot_services::c_ptr::CPtr,
    management_mode::supervisor::{MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_USER_GUID},
    pi::hob::Hob,
};
use patina_internal_core::depex::{AssociatedDependency, Depex};
use r_efi::efi;
use spin::Mutex;

use crate::{DepexHobData, MM_SUPERVISOR_CORE_GUID, MM_SUPERVISOR_DEPEX_HOB_GUID, protocol_db::ProtocolDatabase};

/// Represents a discovered MM driver pending dispatch.
#[derive(Debug)]
struct DriverEntry {
    /// The GUID identifying this driver (from `MemoryAllocationModule.module_name`).
    file_name: efi::Guid,
    /// The entry point address of the driver.
    entry_point: u64,
    /// The base address of the driver image in memory.
    _image_base: u64,
    /// The size of the driver image in memory.
    _image_size: u64,
    /// The parsed dependency expression, if any.
    depex: Option<Depex>,
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
                    log::info!("Skipping core module: {}", module_name);
                    continue;
                }

                log::info!(
                    "Found MM driver: name={}, entry=0x{:016x}, base=0x{:016x}, size=0x{:x}",
                    module_name,
                    mem_alloc_mod.entry_point,
                    mem_alloc_mod.alloc_descriptor.memory_base_address,
                    mem_alloc_mod.alloc_descriptor.memory_length,
                );

                // Look for a paired depex GuidHob in the next HOB
                let depex: Option<Depex> = if let Some(next_hob) = all_hobs.get(index + 1) {
                    if let Hob::GuidHob(guid_hob, data) = next_hob {
                        if guid_hob.name == MM_SUPERVISOR_DEPEX_HOB_GUID {
                            log::debug!("  Found depex HOB ({} bytes)", data.len());
                            if data.is_empty() {
                                None
                            } else {
                                // Check the name matches the expected depex HOB GUID before parsing.
                                let depex_hob_data = <[u8]>::as_ptr(data) as *const DepexHobData;
                                // SAFETY: We trust that the supervisor correctly formats the depex HOB data
                                let depex_hob_data = unsafe { &*depex_hob_data };
                                if depex_hob_data.name != module_name {
                                    panic!(
                                        "Depex HOB module name {} does not match driver module name {}",
                                        depex_hob_data.name, module_name
                                    );
                                }
                                // print depex_hob_data.depex_expression pointer and length
                                log::info!(
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

                log::info!("  Driver {} has depex: {:?}", module_name, depex);

                drivers.push(DriverEntry {
                    file_name: module_name.into_inner(),
                    entry_point: mem_alloc_mod.entry_point,
                    _image_base: mem_alloc_mod.alloc_descriptor.memory_base_address,
                    _image_size: mem_alloc_mod.alloc_descriptor.memory_length,
                    depex,
                });
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
                    match driver.depex.as_ref().map(|d| d.is_associated()) {
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
                log::info!(
                    "Dispatching MM driver {} at entry 0x{:016x}",
                    patina::Guid::from_ref(&driver.file_name),
                    driver.entry_point,
                );

                // Call the driver's entry point.
                // MM driver entry signature: EFI_STATUS EFIAPI DriverEntry(EFI_HANDLE ImageHandle, EFI_MM_SYSTEM_TABLE *MmSystemTable)
                // We pass a null image handle and the system table pointer.
                type MmDriverEntryPoint = unsafe extern "efiapi" fn(efi::Handle, *const c_void) -> efi::Status;
                let entry_fn: MmDriverEntryPoint = unsafe { core::mem::transmute(driver.entry_point) };

                let status = unsafe { entry_fn(core::ptr::null_mut(), mm_system_table) };

                if status == efi::Status::SUCCESS {
                    log::info!("  Driver {} returned SUCCESS.", patina::Guid::from_ref(&driver.file_name));
                    total_dispatched += 1;
                } else {
                    log::warn!(
                        "  Driver {} returned status: 0x{:x}",
                        patina::Guid::from_ref(&driver.file_name),
                        status.as_usize(),
                    );
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
                "Driver {} discovered but not dispatched (unsatisfied depex).",
                patina::Guid::from_ref(&driver.file_name),
            );
        }

        total_dispatched
    }
}
