//! Save State Read Operations for the MM Supervisor Syscall Dispatcher
//!
//! Implements the two-phase save state read protocol used by the
//! `EFI_MM_CPU_PROTOCOL.ReadSaveState()` user-space API.
//!
//! Phase 1 (`SyscallIndex::SaveStateRead`): stores the requested register
//! and CPU index in a per-BSP holder.
//!
//! Phase 2 (`SyscallIndex::SaveStateRead2`): validates the request against
//! the MM security policy, reads the register value from the CPU's SMRAM save
//! state area, and copies the result into the caller-supplied buffer.
//!
//! ## Security Model
//!
//! - User buffer addresses are validated via page-table ownership queries.
//! - Policy-gated registers (RAX, IO) are checked through
//!   [`PolicyGate::is_save_state_read_allowed`](patina_mm_policy::PolicyGate::is_save_state_read_allowed).
//! - `PROCESSOR_ID` is always allowed (informational, not security-sensitive).
//! - Other architectural registers pass through without policy gating, matching
//!   the C reference implementation's allow-list semantics.
//!
//! ## Save State Regions
//!
//! The MM IPL hands the supervisor the per-CPU SMBASE array in the `PassDown` HOB.
//! [`SaveStateInfo`] records that array and the CPU count. During initialization,
//! [`validate_save_state_regions`] proves once that the array and every CPU's save
//! state map lie in supervisor-owned MMRAM, and [`log_save_state_map`] prints the
//! resulting layout. Every later read goes through that validated [`SaveStateInfo`].
//!
//! ## Vendor Selection
//!
//! The SMRAM save state vendor (Intel vs AMD) is chosen at build time by the
//! `save_state_amd` Cargo feature, and only that layout is compiled in. Each
//! read copies the target CPU's map out of the [`SaveStateInfo`] that
//! initialization validated, as a [`SaveState`], which does every offset lookup
//! and field decode and reports failures as [`SaveStateError`]. This module
//! applies the policy and turns those errors into the syscall's EFI status.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use patina::MemoryRange;
use patina::standard::efi::Status;
use patina_internal_cpu::save_state::{
    IO_INFO_SIZE, IO_TYPE_INPUT, MmSaveStateRegister, SMRAM_SAVE_STATE_MAP_SIZE, SaveState, SaveStateError,
};

use crate::{
    error::MmSupervisorResult,
    intrinsics::current_apic_id,
    memory::page_ownership::{PageOwnership, query_address_ownership},
    policy::{SaveStateCondition, SaveStateField, gate::PolicyGate},
    privilege_mgmt::SyscallResult,
    state::{init_state, security_state},
    user_access_guard::with_user_access,
};

/// Errors produced while validating the per-CPU save-state regions the `PassDown` HOB describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveStateValidationError {
    /// The SMBASE array is null, empty, misaligned, overflowing, or not entirely inside MMRAM.
    UnusableSmBaseArray {
        /// Base address of the array.
        base: u64,
        /// Number of entries the array was said to hold.
        count: usize,
    },
    /// The SMBASE array is inside MMRAM but is not mapped supervisor-only.
    SmBaseArrayNotSupervisorOwned {
        /// Base address of the array.
        base: u64,
        /// Number of entries the array was said to hold.
        count: usize,
    },
    /// A CPU's save-state region is null, overflows, or is not entirely inside MMRAM.
    UnusableSaveStateRegion {
        /// Index of the offending CPU.
        cpu_index: usize,
        /// The SMBASE the array reported for it.
        smbase: u64,
    },
    /// A CPU's save-state region is inside MMRAM but is not mapped supervisor-only.
    SaveStateRegionNotSupervisorOwned {
        /// Index of the offending CPU.
        cpu_index: usize,
        /// The SMBASE the array reported for it.
        smbase: u64,
    },
}

impl core::error::Error for SaveStateValidationError {}

impl core::fmt::Display for SaveStateValidationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnusableSmBaseArray { base, count } => write!(
                f,
                "the SMBASE array at 0x{base:016x} with {count} entries is null, misaligned, overflowing, \
                 or not entirely inside MMRAM"
            ),
            Self::SmBaseArrayNotSupervisorOwned { base, count } => write!(
                f,
                "the SMBASE array at 0x{base:016x} with {count} entries is inside MMRAM but is not mapped \
                 supervisor-only"
            ),
            Self::UnusableSaveStateRegion { cpu_index, smbase } => write!(
                f,
                "the save-state region for CPU {cpu_index} at SMBASE 0x{smbase:016x} is null, overflowing, \
                 or not entirely inside MMRAM"
            ),
            Self::SaveStateRegionNotSupervisorOwned { cpu_index, smbase } => write!(
                f,
                "the save-state region for CPU {cpu_index} at SMBASE 0x{smbase:016x} is inside MMRAM but is not \
                 mapped supervisor-only"
            ),
        }
    }
}

/// Size of one save-state map, as a `u64` for address arithmetic.
const MAP_SIZE: u64 = SMRAM_SAVE_STATE_MAP_SIZE as u64;

/// The per-CPU SMBASE array the MM IPL hands over in the `PassDown` HOB, and the CPU count.
///
/// The array holds one `u64` SMBASE per CPU. [`get_save_state`](Self::get_save_state)
/// copies one CPU's save state map out of SMRAM as a [`SaveState`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct SaveStateInfo {
    /// Number of CPUs (from `MP_INFORMATION_HOB_DATA.NumberOfProcessors`).
    number_of_cpus: usize,
    /// Address of the SMBASE array (`u64[number_of_cpus]`).
    smbase_array: u64,
}

impl SaveStateInfo {
    /// Records where the SMBASE array lives.
    ///
    /// ## Errors
    ///
    /// Returns [`SaveStateValidationError::UnusableSmBaseArray`] when `number_of_cpus` is 0,
    /// when `smbase_array` is null or not aligned for `u64`, or when the end of the array
    /// does not fit in a `u64`.
    ///
    /// ## Safety
    ///
    /// This function checks the shape of the array but cannot check that its memory is
    /// readable. [`smbase`](Self::smbase), [`save_state_map_base`](Self::save_state_map_base),
    /// and [`get_save_state`](Self::get_save_state) read through `smbase_array`, so before
    /// the first such call:
    ///
    /// - The `number_of_cpus` `u64` SMBASE values at `smbase_array` must be initialized and
    ///   readable.
    /// - For every CPU for which `save_state_map_base` returns an address,
    ///   `SMRAM_SAVE_STATE_MAP_SIZE` bytes at that address must be readable. Only
    ///   `get_save_state` reads them.
    ///
    /// Both must stay true for as long as those methods are called. The value may be built
    /// before [`validate_save_state_regions`] checks the regions, as long as each check
    /// finishes before the first read it covers.
    pub(crate) unsafe fn new(number_of_cpus: usize, smbase_array: u64) -> Result<Self, SaveStateValidationError> {
        let invalid = SaveStateValidationError::UnusableSmBaseArray { base: smbase_array, count: number_of_cpus };
        let array_size = number_of_cpus
            .checked_mul(core::mem::size_of::<u64>())
            .and_then(|size| u64::try_from(size).ok())
            .filter(|size| *size != 0)
            .ok_or(invalid)?;
        if smbase_array == 0
            || !smbase_array.is_multiple_of(core::mem::align_of::<u64>() as u64)
            || smbase_array.checked_add(array_size).is_none()
        {
            return Err(invalid);
        }

        Ok(Self { number_of_cpus, smbase_array })
    }

    /// Returns the number of CPUs.
    pub(crate) fn number_of_cpus(&self) -> usize {
        self.number_of_cpus
    }

    /// Returns the address of the SMBASE array.
    pub(crate) fn smbase_array(&self) -> u64 {
        self.smbase_array
    }

    /// Returns the size of the SMBASE array in bytes.
    pub(crate) fn smbase_array_size(&self) -> u64 {
        // `new` checked that this product fits in a `u64`.
        (self.number_of_cpus * core::mem::size_of::<u64>()) as u64
    }

    /// Returns the SMBASE the array records for CPU `cpu_index`.
    ///
    /// ## Errors
    ///
    /// Returns [`SaveStateError::InvalidParameter`] when `cpu_index` is not below the CPU
    /// count.
    pub(crate) fn smbase(&self, cpu_index: usize) -> Result<u64, SaveStateError> {
        if cpu_index >= self.number_of_cpus {
            return Err(SaveStateError::InvalidParameter);
        }

        // SAFETY: `SaveStateInfo::new` checked that `smbase_array` is a non-null, aligned array of
        // `number_of_cpus` `u64` entries, its contract makes them initialized and readable, and
        // `cpu_index` is below that count.
        Ok(unsafe { (self.smbase_array as *const u64).add(cpu_index).read() })
    }

    /// Returns the address of the save state map of CPU `cpu_index`.
    ///
    /// ## Errors
    ///
    /// Returns the errors of [`smbase`](Self::smbase) and of [`SaveState::map_base`].
    pub(crate) fn save_state_map_base(&self, cpu_index: usize) -> Result<u64, SaveStateError> {
        SaveState::map_base(self.smbase(cpu_index)?)
    }

    /// Copies the save state map of CPU `cpu_index` out of SMRAM.
    ///
    /// ## Errors
    ///
    /// Returns the errors of [`smbase`](Self::smbase) and of [`SaveState::from_smbase`].
    pub(crate) fn get_save_state(&self, cpu_index: u64) -> Result<SaveState, SaveStateError> {
        usize::try_from(cpu_index)
            .map_err(|_| SaveStateError::InvalidParameter)
            .and_then(|cpu_index| self.smbase(cpu_index))
            .and_then(|smbase| {
                // SAFETY: the contract of `SaveStateInfo::new` makes the map of every SMBASE in
                // the array readable.
                unsafe { SaveState::from_smbase(smbase) }
            })
            .inspect_err(|error| log::error!("Save state read for CPU {cpu_index}: {error}"))
    }
}

/// Requires every per-CPU save-state region the `PassDown` HOB describes to lie inside MMRAM and
/// to be mapped supervisor-only.
///
/// The save-state syscall turns `smbase_array[i] + SMRAM_SAVE_STATE_MAP_OFFSET` into a slice and hands
/// selected bytes of it back to Ring 3. The MM IPL supplies those SMBASEs and sits outside the
/// supervisor's trust boundary, so an entry that is never checked is an arbitrary read primitive:
/// proving the array itself is in MMRAM says nothing about where its contents point. Checking all
/// of them once, here, is what lets [`SaveStateInfo::get_save_state`] treat them as sound later.
///
/// Being inside MMRAM is not on its own enough. A save-state map holds a CPU's saved context, and
/// the syscall exists so Ring 3 can read parts of it under policy; a page Ring 3 can reach
/// directly hands it the whole map and lets it rewrite what the supervisor is about to read.
/// Supervisor-only mapping is what keeps the syscall the only way in.
pub(crate) fn validate_save_state_regions(
    info: &SaveStateInfo,
    is_inside_mmram: impl Fn(&MemoryRange) -> bool,
    is_supervisor_owned: impl Fn(&MemoryRange) -> bool,
) -> MmSupervisorResult<()> {
    let (smbase_array, number_of_cpus) = (info.smbase_array(), info.number_of_cpus());
    let unusable_array = || SaveStateValidationError::UnusableSmBaseArray { base: smbase_array, count: number_of_cpus };
    // `SaveStateInfo::new` already rejected a null, empty, misaligned, or overflowing array.
    let array = MemoryRange::new(smbase_array, info.smbase_array_size()).map_err(|_| unusable_array())?;
    if !is_inside_mmram(&array) {
        return Err(unusable_array().into());
    }
    if !is_supervisor_owned(&array) {
        return Err(SaveStateValidationError::SmBaseArrayNotSupervisorOwned {
            base: smbase_array,
            count: number_of_cpus,
        }
        .into());
    }

    // The checks above prove the SMBASE array, so the entries can be read through `info`.
    for cpu_index in 0..number_of_cpus {
        let smbase = info
            .smbase(cpu_index)
            .map_err(|_| SaveStateValidationError::UnusableSmBaseArray { base: smbase_array, count: number_of_cpus })?;
        let map =
            info.save_state_map_base(cpu_index).ok().and_then(|map_base| MemoryRange::new(map_base, MAP_SIZE).ok());
        match map {
            Some(map) if is_inside_mmram(&map) => {
                if !is_supervisor_owned(&map) {
                    return Err(
                        SaveStateValidationError::SaveStateRegionNotSupervisorOwned { cpu_index, smbase }.into()
                    );
                }
            }
            _ => return Err(SaveStateValidationError::UnusableSaveStateRegion { cpu_index, smbase }.into()),
        }
    }

    Ok(())
}

/// Logs the per-CPU SMBASE and the save-state window derived from it.
///
/// This mirrors the CPU relocation table the C `PiSmmCpuDxeSmm` driver emits and
/// gives the same at-a-glance view when diagnosing save-state or SMBASE issues.
pub(crate) fn log_save_state_map(info: SaveStateInfo) {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }

    for cpu_index in 0..info.number_of_cpus() {
        let smbase = match info.smbase(cpu_index) {
            Ok(smbase) => smbase,
            Err(error) => {
                log::error!("CPU[{cpu_index:03}]  has no SMBASE entry: {error}");
                continue;
            }
        };
        match info.save_state_map_base(cpu_index) {
            Ok(save_state) => log::debug!(
                "CPU[{cpu_index:03}]  SMBASE=0x{smbase:016x}  SaveState=0x{save_state:016x}  Size=0x{SMRAM_SAVE_STATE_MAP_SIZE:08x}"
            ),
            Err(error) => {
                log::error!("CPU[{cpu_index:03}]  SMBASE=0x{smbase:016x} has no usable save-state map: {error}");
            }
        }
    }
}

/// Holds the parameters from Phase 1 until Phase 2 completes the read.
pub(crate) struct SaveStateAccessHolder {
    /// APIC ID of the CPU that staged the request (must match in Phase 2).
    pub(crate) caller: u32,
    /// User protocol pointer (must match across both phases).
    pub(crate) user_protocol: u64,
    /// Register to read.
    pub(crate) register: MmSaveStateRegister,
    /// CPU index to read from.
    pub(crate) cpu_index: u64,
}

/// Returns the ordered sequence of policy checks a read of `reg` must clear.
///
fn policy_checks_for_register(reg: MmSaveStateRegister) -> &'static [SaveStateField] {
    match reg {
        MmSaveStateRegister::Io => &[SaveStateField::IoTrap, SaveStateField::Rax],
        MmSaveStateRegister::Rax => &[SaveStateField::Rax],
        _ => &[],
    }
}

/// Processes Phase 1 of the save state read syscall.
///
/// Validates and stores the register and CPU index for the subsequent Phase 2
/// call. The `protocol` pointer is retained for a consistency check in Phase 2,
/// and `register_raw` is the raw `EFI_MM_SAVE_STATE_REGISTER` value.
pub fn save_state_read_phase1(protocol: u64, register_raw: u64, cpu_index: u64) -> SyscallResult {
    let num_cpus = get_number_of_cpus()
        .inspect_err(|status| log::error!("SAVE_STATE_READ: Unable to get number of CPUs: {:#x}", status.as_usize()))?;

    stage_read_request(current_apic_id(), protocol, register_raw, cpu_index, num_cpus)
}

/// Validates a Phase 1 request against `num_cpus` and stages it for Phase 2 on behalf of `caller`.
fn stage_read_request(caller: u32, protocol: u64, register_raw: u64, cpu_index: u64, num_cpus: usize) -> SyscallResult {
    let Some(register) = MmSaveStateRegister::from_u64(register_raw) else {
        log::error!("SAVE_STATE_READ: Unknown register value: {register_raw}");
        return Err(Status::INVALID_PARAMETER);
    };

    let Ok(index) = usize::try_from(cpu_index) else {
        log::error!("SAVE_STATE_READ: CPU index {cpu_index} does not fit the target architecture");
        return Err(Status::INVALID_PARAMETER);
    };
    if index >= num_cpus {
        log::error!("SAVE_STATE_READ: CPU index {cpu_index} >= NumberOfCpus {num_cpus}");
        return Err(Status::INVALID_PARAMETER);
    }

    let mut access = security_state().lock_save_state_access();
    *access = Some(SaveStateAccessHolder { caller, user_protocol: protocol, register, cpu_index });

    Ok(0)
}

/// Processes Phase 2 of the save state read syscall.
///
/// Validates the request against the MM security policy, reads the register
/// from the CPU's SMRAM save state area, and copies the result into the user
/// buffer. The `protocol` pointer must match the one supplied in Phase 1.
pub fn save_state_read_phase2(protocol: u64, width: u64, buffer: u64) -> SyscallResult {
    // Retrieve and consume the Phase 1 state
    let holder = {
        let mut access = security_state().lock_save_state_access();
        if let Some(h) = access.take() {
            h
        } else {
            log::error!("SAVE_STATE_READ2: Phase 1 not completed");
            return Err(Status::INVALID_PARAMETER);
        }
    };

    // Phase 1 only stages a request once the metadata exists, so this does not fail in practice.
    let info = save_state_info()?;
    let write_size = validate_read_request(&holder, current_apic_id(), protocol, width, buffer)?;

    let mut out = [0u8; IO_INFO_SIZE];
    let out = out.get_mut(..write_size).ok_or_else(|| {
        log::error!("SAVE_STATE_READ2: write size {write_size} exceeds the {IO_INFO_SIZE}-byte staging buffer");
        Status::BUFFER_TOO_SMALL
    })?;

    if holder.register == MmSaveStateRegister::ProcessorId {
        // Special case: PROCESSOR_ID - always allowed, no policy check.
        read_processor_id(holder.cpu_index, out)?;
    } else {
        let state = info.get_save_state(holder.cpu_index).map_err(|error| {
            log::error!("SAVE_STATE_READ2: Unable to read the save state of CPU {}: {error}", holder.cpu_index);
            save_state_error_status(error)
        })?;

        let Some(gate) = security_state().policy_gate() else {
            log::error!("SAVE_STATE_READ2: Policy gate not initialized");
            return Err(Status::NOT_READY);
        };

        read_gated_register(&state, gate, holder.register, width, out)?;
    }

    // SAFETY: `validate_read_request` confirmed `buffer` is a user-owned region of at least
    // `out.len()` bytes.
    unsafe { copy_to_user(buffer as *mut u8, out) };

    Ok(0)
}

/// Validates a Phase 2 request against the staged Phase 1 hand-off.
///
/// Returns the number of bytes that will be written to `buffer`.
fn validate_read_request(
    holder: &SaveStateAccessHolder,
    caller: u32,
    protocol: u64,
    width: u64,
    buffer: u64,
) -> Result<usize, Status> {
    // The hand-off lives in one slot shared by every core, so a request staged by another CPU
    // must not be completed here. The protocol pointer cannot catch this: every caller passes
    // the same user protocol. Reading another core's staged request would let a caller obtain a
    // register under the trap condition of a CPU other than its own.
    if holder.caller != caller {
        log::error!("SAVE_STATE_READ2: staged by CPU {} but completed on CPU {caller}", holder.caller);
        return Err(Status::ACCESS_DENIED);
    }

    // Verify protocol matches Phase 1
    if holder.user_protocol != protocol {
        log::error!("SAVE_STATE_READ2: Protocol mismatch: expected 0x{:x}, got 0x{:x}", holder.user_protocol, protocol);
        return Err(Status::INVALID_PARAMETER);
    }

    if width == 0 || buffer == 0 {
        log::error!("SAVE_STATE_READ2: Invalid width ({width}) or null buffer");
        return Err(Status::INVALID_PARAMETER);
    }

    let write_size = actual_write_size(holder.register, width);
    if write_size == 0 {
        log::error!("SAVE_STATE_READ2: Unsupported width {width} for register {:?}", holder.register);
        return Err(Status::UNSUPPORTED);
    }

    match MemoryRange::new(buffer, write_size as u64).ok().and_then(|range| query_address_ownership(&range)) {
        Some(PageOwnership::User) => Ok(write_size),
        Some(owner) => {
            log::error!("SAVE_STATE_READ2: Buffer 0x{buffer:x} owned by {owner:?}, expected User");
            Err(Status::ACCESS_DENIED)
        }
        None => {
            log::error!("SAVE_STATE_READ2: Buffer 0x{buffer:x} not in mapped memory");
            Err(Status::ACCESS_DENIED)
        }
    }
}

/// Applies the MM security policy to a save-state read and, when allowed, extracts the
/// register value from `state` into `out`.
fn read_gated_register(
    state: &SaveState,
    gate: &PolicyGate,
    register: MmSaveStateRegister,
    width: u64,
    out: &mut [u8],
) -> SyscallResult {
    let policy_checks = policy_checks_for_register(register);
    let condition = if policy_checks.is_empty() { None } else { inspect_io_condition(state) };

    // An IO read needs the trap condition; if it can't be determined the CPU did
    // not trap an I/O instruction, which is NOT_FOUND rather than a policy denial.
    if register == MmSaveStateRegister::Io && condition.is_none() {
        log::trace!("SAVE_STATE_READ2: Unable to determine I/O condition from save state");
        return Err(Status::NOT_FOUND);
    }

    // Each required field must independently clear the policy under the same trap
    // condition.
    for &field in policy_checks {
        if let Err(e) = gate.is_save_state_read_allowed(field, width as usize, condition) {
            log::error!("SAVE_STATE_READ2: Policy denied read of {register:?} (field {field:?}): {e:?}");
            return Err(Status::ACCESS_DENIED);
        }
    }

    // Dispatch to the matching reader. Each one decodes `state` and writes into
    // the validated `out` buffer, reporting failures as `SaveStateError`.
    let width = usize::try_from(width).map_err(|_| Status::INVALID_PARAMETER)?;
    let result = match register {
        MmSaveStateRegister::Io => state.read_io_register(out),
        MmSaveStateRegister::Lma => state.read_lma_register(width, out),
        _ => state.read_architectural_register(register, width, out),
    };
    result.map(|()| 0).map_err(|error| {
        log::error!("SAVE_STATE_READ2: Reading {register:?} failed: {error}");
        save_state_error_status(error)
    })
}

/// Returns the per-CPU save-state metadata captured at initialization.
fn save_state_info() -> Result<SaveStateInfo, Status> {
    if let Some(info) = security_state().save_state_info() {
        Ok(info)
    } else {
        log::error!("Save-state metadata not initialized");
        Err(Status::NOT_READY)
    }
}

/// Returns the number of CPUs from the save-state metadata.
fn get_number_of_cpus() -> Result<usize, Status> {
    Ok(save_state_info()?.number_of_cpus())
}

/// Determines the actual number of bytes that will be written to the user buffer.
///
/// Returns 0 if the width is not supported for the given register.
fn actual_write_size(register: MmSaveStateRegister, width: u64) -> usize {
    match register {
        MmSaveStateRegister::Io => IO_INFO_SIZE,
        MmSaveStateRegister::ProcessorId => 8,
        MmSaveStateRegister::Lma => {
            if width == 4 || width == 8 {
                width as usize
            } else {
                0
            }
        }
        _ => {
            if let Some(info) = SaveState::register_info(register) {
                if width == 2 && info.native_width >= 2 {
                    2
                } else if width == 4 && info.native_width >= 4 {
                    4
                } else if width == 8 && info.native_width == 8 {
                    8
                } else {
                    0
                }
            } else {
                0
            }
        }
    }
}

/// Copies a staged save-state result into a validated user buffer while SMAP is disabled.
///
/// ## Safety
///
/// `buffer` must reference a writable user-owned region of at least `out.len()` bytes and
/// must not overlap `out`.
unsafe fn copy_to_user(buffer: *mut u8, out: &[u8]) {
    // SAFETY: the caller's contract guarantees `buffer` is a writable user-owned region of at
    // least `out.len()` bytes that does not overlap `out`, so the only access made while SMAP is
    // lifted targets that validated user range.
    unsafe {
        with_user_access(|| core::ptr::copy_nonoverlapping(out.as_ptr(), buffer, out.len()));
    }
}

/// Reads the `PROCESSOR_ID` for a given CPU and writes it to the user buffer.
///
/// The `ProcessorId` (APIC ID) is read from the supervisor-owned [`CpuManager`](crate::mmcore::cpu::CpuManager).
fn read_processor_id(cpu_index: u64, out: &mut [u8]) -> SyscallResult {
    let lookup = init_state().processor_id_lookup_fn().ok_or(Status::NOT_READY)?;
    let processor_id = lookup(cpu_index as usize).ok_or_else(|| {
        log::error!("PROCESSOR_ID: CPU index {cpu_index} has no registered processor ID");
        Status::NOT_FOUND
    })?;

    // Write the 8-byte ProcessorId to the user buffer.
    let out_len = out.len();
    out.get_mut(..8)
        .ok_or_else(|| {
            log::error!("PROCESSOR_ID: output buffer holds {out_len} bytes, need 8");
            Status::BUFFER_TOO_SMALL
        })?
        .copy_from_slice(&processor_id.to_le_bytes());

    log::debug!("PROCESSOR_ID: CPU {cpu_index} = 0x{processor_id:x}");
    Ok(0)
}

/// Returns the I/O condition (IN vs OUT) that caused the SMI, for policy checking.
///
/// Returns `None` when the SMI was not an I/O trap, or when the save state revision predates
/// I/O trap information. Either way the policy check then runs without a trap condition, so
/// only unconditional descriptors can allow the read.
fn inspect_io_condition(state: &SaveState) -> Option<SaveStateCondition> {
    let io_info = match state.io_info() {
        Ok(io_info) => io_info?,
        Err(error) => {
            log::error!("SAVE_STATE_READ2: {error}; reading the save state without an I/O trap condition");
            return None;
        }
    };
    match io_info.io_type {
        IO_TYPE_INPUT => Some(SaveStateCondition::IoRead),
        _ => Some(SaveStateCondition::IoWrite),
    }
}

/// Maps a save state failure onto the status the syscall reports.
fn save_state_error_status(error: SaveStateError) -> Status {
    match error {
        SaveStateError::InvalidParameter | SaveStateError::UnsupportedWidth { .. } => Status::INVALID_PARAMETER,
        SaveStateError::RegisterNotMapped(_) | SaveStateError::NoIoTrap => Status::NOT_FOUND,
        SaveStateError::BufferTooSmall { .. } => Status::BUFFER_TOO_SMALL,
        SaveStateError::IoInfoUnsupported { .. } => Status::UNSUPPORTED,
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::{
        MmSupervisorCore,
        policy::{
            ACCESS_ATTR_ALLOW, ACCESS_ATTR_DENY, RESOURCE_ATTR_COND_READ, RESOURCE_ATTR_READ, SaveStateDescriptorV1_0,
            TYPE_SAVE_STATE,
        },
        test_support,
    };
    use patina_internal_cpu::save_state::{
        IA32_EFER_LMA, IO_TYPE_OUTPUT, IO_WIDTH_UINT8, IO_WIDTH_UINT16, IO_WIDTH_UINT32, LMA_64BIT, MmSaveStateIoInfo,
        SMRAM_SAVE_STATE_MAP_OFFSET, SaveStateBytes, vendor_constants,
    };
    use serial_test::serial;
    use zerocopy::IntoBytes;

    /// Backing store for a synthetic save-state map plus the SMBASE array that points at it.
    ///
    /// `region` is sized so that the `SMBASE + SMRAM_SAVE_STATE_MAP_OFFSET` window covers exactly
    /// one `SMRAM_SAVE_STATE_MAP_SIZE` map, mirroring the real SMBASE-relative layout.
    struct FakeSmram {
        region: Box<[u8]>,
        sm_bases: Box<[u64]>,
    }

    impl FakeSmram {
        /// Builds an SMBASE array of `num_cpus` entries all pointing at a single backing region.
        fn new(num_cpus: usize) -> Self {
            let region_size = SMRAM_SAVE_STATE_MAP_OFFSET as usize + SMRAM_SAVE_STATE_MAP_SIZE;
            let region = vec![0u8; region_size].into_boxed_slice();
            let smbase = region.as_ptr() as u64;
            Self { region, sm_bases: vec![smbase; num_cpus].into_boxed_slice() }
        }

        /// Returns the metadata the save-state syscall would receive from the `PassDown` HOB.
        fn info(&self) -> SaveStateInfo {
            // SAFETY: every SMBASE in `sm_bases` points at `region`, whose map window holds a full
            // `SMRAM_SAVE_STATE_MAP_SIZE` map, and both live as long as this `FakeSmram`.
            unsafe { SaveStateInfo::new(self.sm_bases.len(), self.sm_bases.as_ptr() as u64) }
                .expect("the fake SMBASE array is valid")
        }

        /// Returns the save-state map bytes (the `SMBASE + 0xfc00` window).
        fn map_mut(&mut self) -> &mut [u8] {
            &mut self.region[SMRAM_SAVE_STATE_MAP_OFFSET as usize..]
        }

        /// Overwrites the SMBASE recorded for `cpu_index`.
        fn set_smbase(&mut self, cpu_index: usize, smbase: u64) {
            self.sm_bases[cpu_index] = smbase;
        }
    }

    /// Returns the address ranges a `FakeSmram` legitimately occupies.
    fn fake_smram_ranges(smram: &FakeSmram) -> Vec<(u64, u64)> {
        vec![
            (smram.sm_bases.as_ptr() as u64, (smram.sm_bases.len() * core::mem::size_of::<u64>()) as u64),
            (smram.region.as_ptr() as u64, smram.region.len() as u64),
        ]
    }

    /// Accepts only ranges lying wholly inside one of `allowed`.
    fn inside_any(allowed: Vec<(u64, u64)>) -> impl Fn(&MemoryRange) -> bool {
        move |range| allowed.iter().any(|&(base, len)| range.base() >= base && range.end() <= base + len)
    }

    /// Wraps an SMBASE array that the validator must reject before it reads any entry.
    fn unchecked_info(smbase_array: u64, number_of_cpus: usize) -> SaveStateInfo {
        // SAFETY: each caller passes an array the validator rejects before reading through it.
        unsafe { SaveStateInfo::new(number_of_cpus, smbase_array) }.expect("the array has a valid shape")
    }

    /// Answers "supervisor-owned" for every range, isolating the containment rules.
    fn all_supervisor_owned(_range: &MemoryRange) -> bool {
        true
    }

    #[test]
    fn test_save_state_validation_errors_render_each_variant_distinctly() {
        // These reject the per-CPU save-state regions the PassDown HOB describes, which stops
        // initialization. The message carries the address and the CPU index, because that is what
        // identifies which descriptor the producer got wrong.
        assert_eq!(
            format!("{}", SaveStateValidationError::UnusableSmBaseArray { base: 0x7000, count: 4 }),
            "the SMBASE array at 0x0000000000007000 with 4 entries is null, misaligned, overflowing, \
             or not entirely inside MMRAM"
        );
        assert_eq!(
            format!("{}", SaveStateValidationError::SmBaseArrayNotSupervisorOwned { base: 0x7000, count: 4 }),
            "the SMBASE array at 0x0000000000007000 with 4 entries is inside MMRAM but is not mapped \
             supervisor-only"
        );
        assert_eq!(
            format!("{}", SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 2, smbase: 0x8000 }),
            "the save-state region for CPU 2 at SMBASE 0x0000000000008000 is null, overflowing, \
             or not entirely inside MMRAM"
        );
        assert_eq!(
            format!("{}", SaveStateValidationError::SaveStateRegionNotSupervisorOwned { cpu_index: 2, smbase: 0x8000 }),
            "the save-state region for CPU 2 at SMBASE 0x0000000000008000 is inside MMRAM but is not \
             mapped supervisor-only"
        );

        // Placement and ownership are separate findings for the same region, so the two must not
        // render the same way.
        assert_ne!(
            format!("{}", SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 0, smbase: 0x8000 }),
            format!("{}", SaveStateValidationError::SaveStateRegionNotSupervisorOwned { cpu_index: 0, smbase: 0x8000 })
        );
        // The CPU index distinguishes two cores failing the same check.
        assert_ne!(
            format!("{}", SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 0, smbase: 0x8000 }),
            format!("{}", SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 1, smbase: 0x8000 })
        );
    }

    #[test]
    fn test_save_state_info_new_rejects_an_unusable_array() {
        let smbase_entries = [0u64; 2];
        let array = smbase_entries.as_ptr() as u64;

        // A null array, an empty array, an extent that overflows, a misaligned array, and a
        // count whose byte size overflows.
        for (base, count) in [(0, 2), (array, 0), (u64::MAX - 7, 2), (array + 1, 2), (array, usize::MAX)] {
            // SAFETY: each array is rejected before it is read.
            let result = unsafe { SaveStateInfo::new(count, base) };
            assert_eq!(
                result.err(),
                Some(SaveStateValidationError::UnusableSmBaseArray { base, count }),
                "array at {base:#x} with {count} entries"
            );
        }
    }

    #[test]
    fn test_save_state_info_looks_up_smbase_and_map_base_by_cpu_index() {
        let smbase_entries = [0x3000_0000u64, u64::MAX - 0x1_0000, 0, u64::MAX];
        // SAFETY: `smbase_entries` holds four entries and outlives every call below, none of which
        // reads a map.
        let info = unsafe { SaveStateInfo::new(smbase_entries.len(), smbase_entries.as_ptr() as u64) }.unwrap();
        assert_eq!(info.smbase_array_size(), 32);

        assert_eq!(info.smbase(0), Ok(0x3000_0000));
        assert_eq!(info.smbase(2), Ok(0));
        assert_eq!(info.smbase(4), Err(SaveStateError::InvalidParameter));

        assert_eq!(info.save_state_map_base(0), Ok(0x3000_FC00));
        assert_eq!(info.save_state_map_base(1), Ok(u64::MAX - 0x400));
        // A null SMBASE, an SMBASE whose map ends past the address space, and an index past the
        // array.
        for cpu_index in [2, 3, 4] {
            assert_eq!(info.save_state_map_base(cpu_index), Err(SaveStateError::InvalidParameter));
        }
    }

    #[test]
    fn test_validate_save_state_regions_accepts_regions_inside_mmram() {
        let smram = FakeSmram::new(2);
        let info = smram.info();

        assert_eq!(
            validate_save_state_regions(&info, inside_any(fake_smram_ranges(&smram)), all_supervisor_owned),
            Ok(())
        );
    }

    #[test]
    fn test_validate_save_state_regions_rejects_an_array_that_ring_3_can_reach() {
        // An array Ring 3 can write lets it choose where every save-state read lands.
        let smram = FakeSmram::new(2);
        let info = smram.info();

        assert_eq!(
            validate_save_state_regions(&info, inside_any(fake_smram_ranges(&smram)), |_| false),
            Err(SaveStateValidationError::SmBaseArrayNotSupervisorOwned { base: info.smbase_array(), count: 2 }.into())
        );
    }

    #[test]
    fn test_validate_save_state_regions_rejects_a_region_that_ring_3_can_reach() {
        // The syscall is meant to be the only way into a save-state map under policy, so a map
        // Ring 3 can reach directly is refused even though it is inside MMRAM.
        let smram = FakeSmram::new(2);
        let info = smram.info();
        let array_size = (smram.sm_bases.len() * core::mem::size_of::<u64>()) as u64;

        // Only the array itself is supervisor-owned; the maps it points at are not.
        let owned = move |range: &MemoryRange| range.base() == info.smbase_array() && range.size() == array_size;

        assert_eq!(
            validate_save_state_regions(&info, inside_any(fake_smram_ranges(&smram)), owned),
            Err(SaveStateValidationError::SaveStateRegionNotSupervisorOwned {
                cpu_index: 0,
                smbase: smram.sm_bases[0]
            }
            .into())
        );
    }

    #[test]
    fn test_validate_save_state_regions_rejects_an_array_outside_mmram_without_reading_it() {
        // The array has to be refused before it is dereferenced, so this points at an address
        // that would fault if the check were done in the wrong order.
        let result = validate_save_state_regions(&unchecked_info(0xdead_0000, 4), |_| false, all_supervisor_owned);

        assert_eq!(result, Err(SaveStateValidationError::UnusableSmBaseArray { base: 0xdead_0000, count: 4 }.into()));
    }

    #[test]
    fn test_validate_save_state_regions_rejects_an_entry_outside_mmram() {
        // This is the arbitrary-read case: an in-MMRAM array whose entry points anywhere the
        // producer likes.
        let mut smram = FakeSmram::new(2);
        smram.set_smbase(1, 0x1000);

        assert_eq!(
            validate_save_state_regions(&smram.info(), inside_any(fake_smram_ranges(&smram)), all_supervisor_owned),
            Err(SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 1, smbase: 0x1000 }.into())
        );
    }

    #[test]
    fn test_validate_save_state_regions_rejects_a_null_entry() {
        let mut smram = FakeSmram::new(2);
        smram.set_smbase(0, 0);

        assert_eq!(
            validate_save_state_regions(&smram.info(), inside_any(fake_smram_ranges(&smram)), all_supervisor_owned),
            Err(SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 0, smbase: 0 }.into())
        );
    }

    #[test]
    fn test_validate_save_state_regions_rejects_an_entry_whose_region_overflows() {
        let mut smram = FakeSmram::new(1);
        smram.set_smbase(0, u64::MAX);

        assert_eq!(
            validate_save_state_regions(&smram.info(), |_| true, all_supervisor_owned),
            Err(SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 0, smbase: u64::MAX }.into())
        );
    }

    #[test]
    #[serial]
    fn test_log_save_state_map_does_nothing_when_debug_is_disabled() {
        test_support::init_test_logger();
        let smram = FakeSmram::new(2);
        log::set_max_level(log::LevelFilter::Info);

        log_save_state_map(smram.info());

        log::set_max_level(log::LevelFilter::Trace);
    }

    #[test]
    #[serial]
    fn test_log_save_state_map_walks_every_cpu() {
        test_support::init_test_logger();
        let smram = FakeSmram::new(3);

        log_save_state_map(smram.info());
    }

    #[test]
    #[serial]
    fn test_log_save_state_map_reports_an_smbase_that_overflows() {
        test_support::init_test_logger();
        let mut smram = FakeSmram::new(1);
        smram.set_smbase(0, u64::MAX);

        log_save_state_map(smram.info());
    }

    #[test]
    fn test_get_save_state_reads_with_the_layout_selected_by_the_build() {
        let mut smram = FakeSmram::new(2);
        // A distinct value at each vendor's RAX offset shows which layout the copied state reads.
        smram.map_mut()[0x035C..0x0364].copy_from_slice(&0x1111u64.to_le_bytes());
        smram.map_mut()[0x03F8..0x0400].copy_from_slice(&0x2222u64.to_le_bytes());

        let state = smram.info().get_save_state(1).expect("state for a valid CPU index");
        let mut out = [0u8; 8];
        assert_eq!(state.read_architectural_register(MmSaveStateRegister::Rax, 8, &mut out), Ok(()));
        let expected = if cfg!(feature = "save_state_amd") { 0x2222 } else { 0x1111 };
        assert_eq!(u64::from_le_bytes(out), expected);
    }

    #[test]
    fn test_get_save_state_rejects_invalid_metadata() {
        let mut smram = FakeSmram::new(2);

        // CPU index beyond the reported CPU count.
        assert_eq!(smram.info().get_save_state(2).err(), Some(SaveStateError::InvalidParameter));

        // A null SMBASE entry means the CPU was never relocated.
        smram.set_smbase(0, 0);
        assert_eq!(smram.info().get_save_state(0).err(), Some(SaveStateError::InvalidParameter));
    }

    struct TestPlatform;

    impl crate::PlatformInfo for TestPlatform {}

    /// Builds a zeroed save-state map whose `SMMRevId` advertises I/O trap support.
    fn new_save_state_map() -> Box<SaveStateBytes> {
        let mut map = Box::new([0u8; SMRAM_SAVE_STATE_MAP_SIZE]);
        let constants = vendor_constants();
        write_u32(&mut map[..], constants.smmrevid_offset as usize, constants.min_rev_id_io);
        map
    }

    fn write_u32(buffer: &mut [u8], offset: usize, value: u32) {
        buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(buffer: &mut [u8], offset: usize, value: u64) {
        buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// Encodes an Intel `IOMisc` field that decodes with the requested direction and size.
    #[cfg(not(feature = "save_state_amd"))]
    fn io_trap_field(port: u16, input: bool, byte_count: usize) -> u32 {
        // SmiFlag | Length (bits 3:1) | Type IN(1)/OUT(0) (bits 7:4).
        (u32::from(port) << 16) | (u32::from(input) << 4) | ((byte_count as u32) << 1) | 1
    }

    /// Encodes an AMD `IO_DWord` field that decodes with the requested direction and size.
    #[cfg(feature = "save_state_amd")]
    fn io_trap_field(port: u16, input: bool, byte_count: usize) -> u32 {
        // Valid (bit 1) | SZ8 (bit 4) / SZ16 (bit 5) / SZ32 (default) | Direction (bit 0).
        let size = match byte_count {
            1 => 1 << 4,
            2 => 1 << 5,
            _ => 0,
        };
        (u32::from(port) << 16) | size | (1 << 1) | u32::from(input)
    }

    /// Builds a policy buffer containing a single save-state policy root.
    ///
    /// The returned buffer owns the policy bytes and must outlive any gate created over it.
    fn build_policy(access_attr: u8, descriptors: &[SaveStateDescriptorV1_0]) -> Vec<u64> {
        const HEADER_SIZE: usize = 40;
        const ROOT_SIZE: usize = 24;
        const DESCRIPTOR_SIZE: usize = 16;
        const ROOT_OFFSET: usize = HEADER_SIZE;
        const DESCRIPTOR_OFFSET: usize = ROOT_OFFSET + ROOT_SIZE;

        let total = DESCRIPTOR_OFFSET + descriptors.len() * DESCRIPTOR_SIZE;
        // `Vec<u64>` guarantees the 8-byte alignment `SecurePolicyDataV1_0` requires.
        let mut policy = vec![0u64; total.div_ceil(8)];
        let bytes = policy.as_mut_slice().as_mut_bytes();

        // SecurePolicyDataV1_0 header.
        bytes[0..2].copy_from_slice(&0u16.to_le_bytes()); // version_minor
        bytes[2..4].copy_from_slice(&1u16.to_le_bytes()); // version_major
        write_u32(bytes, 4, total as u32); // size
        write_u32(bytes, 32, ROOT_OFFSET as u32); // policy_root_offset
        write_u32(bytes, 36, 1); // policy_root_count

        // PolicyRootV1 for the save-state descriptors.
        write_u32(bytes, ROOT_OFFSET, 1); // version
        write_u32(bytes, ROOT_OFFSET + 4, ROOT_SIZE as u32); // policy_root_size
        write_u32(bytes, ROOT_OFFSET + 8, TYPE_SAVE_STATE); // policy_type
        write_u32(bytes, ROOT_OFFSET + 12, DESCRIPTOR_OFFSET as u32); // offset
        write_u32(bytes, ROOT_OFFSET + 16, descriptors.len() as u32); // count
        bytes[ROOT_OFFSET + 20] = access_attr;

        for (i, descriptor) in descriptors.iter().enumerate() {
            let at = DESCRIPTOR_OFFSET + i * DESCRIPTOR_SIZE;
            write_u32(bytes, at, descriptor.map_field);
            write_u32(bytes, at + 4, descriptor.attributes);
            write_u32(bytes, at + 8, descriptor.access_condition);
        }

        policy
    }

    /// Creates a gate over `policy`, which the caller must keep alive and unmodified.
    fn gate_over(policy: &[u64]) -> PolicyGate {
        // SAFETY: `build_policy` produced a valid, aligned V1.0 policy buffer that the caller
        // keeps alive for the gate's lifetime.
        unsafe { PolicyGate::new(policy.as_ptr().cast::<u8>(), core::mem::size_of_val(policy)) }
            .expect("valid policy buffer")
    }

    fn descriptor(field: SaveStateField, attributes: u32, condition: SaveStateCondition) -> SaveStateDescriptorV1_0 {
        SaveStateDescriptorV1_0 {
            map_field: field.as_index(),
            attributes,
            access_condition: condition as u32,
            reserved: 0,
        }
    }

    #[test]
    fn test_save_state_processor_id_from_cpu_manager() {
        static SUPERVISOR: MmSupervisorCore<TestPlatform, 4> = MmSupervisorCore::new();

        assert_eq!(SUPERVISOR.cpu_manager().register_cpu(0x20, 2, false), Ok(2));

        let mut out = [0u8; 8];
        assert_eq!(read_processor_id(2, &mut out), Err(Status::NOT_READY));

        assert!(SUPERVISOR.set_instance());

        assert_eq!(read_processor_id(2, &mut out), Ok(0));
        assert_eq!(u64::from_le_bytes(out), 0x20);
        assert_eq!(read_processor_id(1, &mut out), Err(Status::NOT_FOUND));
        assert_eq!(read_processor_id(2, &mut out[..4]), Err(Status::BUFFER_TOO_SMALL));
    }

    #[test]
    fn test_save_state_error_status_maps_each_error() {
        let register = MmSaveStateRegister::Rax;
        let cases = [
            (SaveStateError::InvalidParameter, Status::INVALID_PARAMETER),
            (SaveStateError::RegisterNotMapped(register), Status::NOT_FOUND),
            (SaveStateError::UnsupportedWidth { register, width: 3 }, Status::INVALID_PARAMETER),
            (SaveStateError::BufferTooSmall { needed: 8, available: 4 }, Status::BUFFER_TOO_SMALL),
            (SaveStateError::NoIoTrap, Status::NOT_FOUND),
            (SaveStateError::IoInfoUnsupported { smm_rev_id: 0x30003 }, Status::UNSUPPORTED),
        ];
        for (error, status) in cases {
            assert_eq!(save_state_error_status(error), status, "{error:?}");
        }
    }

    #[test]
    fn test_read_gated_register_reports_reader_errors_as_statuses() {
        // Unrestricted policy, so each result below comes from the reader, not the gate.
        let policy = build_policy(ACCESS_ATTR_ALLOW, &[]);
        let gate = gate_over(&policy);
        let map = new_save_state_map();
        let state = SaveState::new(*map);
        let mut out = [0u8; 8];

        // A 16-byte read is wider than any register.
        assert_eq!(
            read_gated_register(&state, &gate, MmSaveStateRegister::Rbx, 16, &mut [0u8; 16]),
            Err(Status::INVALID_PARAMETER)
        );
        // `LdtInfo` has no entry in either vendor map.
        assert_eq!(
            read_gated_register(&state, &gate, MmSaveStateRegister::LdtInfo, 8, &mut out),
            Err(Status::NOT_FOUND)
        );
        // The buffer is shorter than the requested width.
        assert_eq!(
            read_gated_register(&state, &gate, MmSaveStateRegister::Rbx, 8, &mut out[..4]),
            Err(Status::BUFFER_TOO_SMALL)
        );
    }

    #[test]
    fn test_register_from_u64() {
        assert_eq!(MmSaveStateRegister::from_u64(38), Some(MmSaveStateRegister::Rax));
        assert_eq!(MmSaveStateRegister::from_u64(512), Some(MmSaveStateRegister::Io));
        assert_eq!(MmSaveStateRegister::from_u64(514), Some(MmSaveStateRegister::ProcessorId));
        assert_eq!(MmSaveStateRegister::from_u64(999), None);
        assert_eq!(MmSaveStateRegister::from_u64(0), None);
    }

    #[test]
    fn test_policy_checks_for_register() {
        // RAX maps to a single RAX field check.
        assert_eq!(policy_checks_for_register(MmSaveStateRegister::Rax), &[SaveStateField::Rax]);
        // IO is composite: it discloses the IO trap field and RAX, so both are checked.
        assert_eq!(policy_checks_for_register(MmSaveStateRegister::Io), &[SaveStateField::IoTrap, SaveStateField::Rax]);
        // Non-gated registers still run a single `None` check (root allow/deny default).
        assert_eq!(policy_checks_for_register(MmSaveStateRegister::Rbx), &[]);
        assert_eq!(policy_checks_for_register(MmSaveStateRegister::ProcessorId), &[]);
    }

    #[test]
    fn test_actual_write_size() {
        // IO always writes IO_INFO_SIZE
        assert_eq!(actual_write_size(MmSaveStateRegister::Io, 4), IO_INFO_SIZE);
        assert_eq!(actual_write_size(MmSaveStateRegister::Io, 24), IO_INFO_SIZE);

        // PROCESSOR_ID always writes 8
        assert_eq!(actual_write_size(MmSaveStateRegister::ProcessorId, 8), 8);

        // LMA supports 4 and 8
        assert_eq!(actual_write_size(MmSaveStateRegister::Lma, 4), 4);
        assert_eq!(actual_write_size(MmSaveStateRegister::Lma, 8), 8);
        assert_eq!(actual_write_size(MmSaveStateRegister::Lma, 3), 0);

        // RAX (native 8): supports Width=2, 4, and 8
        assert_eq!(actual_write_size(MmSaveStateRegister::Rax, 2), 2);
        assert_eq!(actual_write_size(MmSaveStateRegister::Rax, 4), 4);
        assert_eq!(actual_write_size(MmSaveStateRegister::Rax, 8), 8);
        assert_eq!(actual_write_size(MmSaveStateRegister::Rax, 16), 0);

        // The layouts disagree on register widths: CR4 is 8 bytes on AMD but only 4 on Intel.
        let cr4_width = if cfg!(feature = "save_state_amd") { 8 } else { 0 };
        assert_eq!(actual_write_size(MmSaveStateRegister::Cr4, 8), cr4_width);
    }

    #[test]
    fn test_save_state_copy_to_user() {
        let source = [0x12, 0x34, 0x56, 0x78];
        let mut destination = [0u8; 4];

        // SAFETY: `destination` is writable, exactly `source.len()` bytes, and does not overlap `source`.
        unsafe { copy_to_user(destination.as_mut_ptr(), &source) };

        assert_eq!(destination, source);
    }

    #[test]
    fn test_io_trap_field_round_trips() {
        // The helper encodes for the selected vendor; this pins that its encoding decodes as
        // intended, so the tests built on it are exercising the direction and size they claim to.
        for (input, expected_type) in [(true, IO_TYPE_INPUT), (false, IO_TYPE_OUTPUT)] {
            for (byte_count, expected_width) in [(1usize, IO_WIDTH_UINT8), (2, IO_WIDTH_UINT16), (4, IO_WIDTH_UINT32)] {
                let mut map = new_save_state_map();
                let field = io_trap_field(0x70, input, byte_count);
                write_u32(&mut map[..], vendor_constants().io_info_offset as usize, field);

                let io_info = SaveState::new(*map).io_info().unwrap().expect("the encoding records an I/O trap");
                assert_eq!((io_info.io_type, io_info.io_width, io_info.io_port), (expected_type, expected_width, 0x70));
            }
        }
    }

    #[test]
    #[serial]
    fn test_save_state_access_holder() {
        // Serialized because `FirmwareOps::save_state_read_phase2` consumes this same global
        // hand-off slot in `privilege_mgmt::syscall_ops`.
        // Test that the mutex works correctly for Phase 1/Phase 2.
        {
            let mut access = crate::state::security_state().lock_save_state_access();
            assert!(access.is_none());
            *access = Some(SaveStateAccessHolder {
                caller: 7,
                user_protocol: 0xDEAD,
                register: MmSaveStateRegister::Rax,
                cpu_index: 0,
            });
        }

        {
            let mut access = crate::state::security_state().lock_save_state_access();
            let holder = access.take().unwrap();
            assert_eq!(holder.caller, 7);
            assert_eq!(holder.user_protocol, 0xDEAD);
            assert_eq!(holder.register, MmSaveStateRegister::Rax);
            assert_eq!(holder.cpu_index, 0);
        }

        {
            let access = crate::state::security_state().lock_save_state_access();
            assert!(access.is_none());
        }
    }

    #[test]
    fn test_save_state_metadata_reports_not_ready_before_init() {
        // The global metadata is only populated from the PassDown HOB during initialization.
        assert_eq!(save_state_info().err(), Some(Status::NOT_READY));
        assert_eq!(get_number_of_cpus().err(), Some(Status::NOT_READY));
        assert_eq!(save_state_read_phase1(0x1000, 38, 0), Err(Status::NOT_READY));
    }

    #[test]
    #[serial]
    fn test_save_state_read_phase1_stages_against_the_initialized_cpu_count() {
        test_support::init_test_logger();
        let smram = FakeSmram::new(2);
        security_state().set_save_state_info(smram.info());

        // Phase 1 bounds the request against the CPU count the PassDown HOB supplied, so a CPU
        // index inside it stages and one past it is refused.
        assert_eq!(save_state_read_phase1(0x1000, 38, 0), Ok(0));
        assert!(security_state().lock_save_state_access().take().is_some());
        assert_eq!(save_state_read_phase1(0x1000, 38, 2), Err(Status::INVALID_PARAMETER));
    }

    #[test]
    #[serial]
    fn test_stage_read_request_validates_and_stores_request() {
        assert_eq!(stage_read_request(7, 0x1000, 38, 0, 4), Ok(0));

        let holder = security_state().lock_save_state_access().take().expect("request staged");
        assert_eq!(holder.caller, 7);
        assert_eq!(holder.user_protocol, 0x1000);
        assert_eq!(holder.register, MmSaveStateRegister::Rax);
        assert_eq!(holder.cpu_index, 0);
    }

    #[test]
    #[serial]
    fn test_stage_read_request_rejects_bad_register_and_cpu_index() {
        assert_eq!(stage_read_request(7, 0x1000, 999, 0, 4), Err(Status::INVALID_PARAMETER));
        assert_eq!(stage_read_request(7, 0x1000, 38, 4, 4), Err(Status::INVALID_PARAMETER));
        assert_eq!(stage_read_request(7, 0x1000, 38, 9, 4), Err(Status::INVALID_PARAMETER));

        assert!(security_state().lock_save_state_access().is_none());
    }

    #[test]
    fn test_validate_read_request_rejects_a_request_staged_by_another_cpu() {
        // The hand-off slot is shared by every core and every caller passes the same user
        // protocol, so the staging CPU is the only thing that can tell the requests apart.
        let holder = SaveStateAccessHolder {
            caller: 7,
            user_protocol: 0x1000,
            register: MmSaveStateRegister::Rax,
            cpu_index: 0,
        };

        assert_eq!(validate_read_request(&holder, 8, 0x1000, 8, 0x5000), Err(Status::ACCESS_DENIED));
    }

    #[test]
    fn test_validate_read_request_rejects_mismatched_or_malformed_requests() {
        let holder = SaveStateAccessHolder {
            caller: 7,
            user_protocol: 0x1000,
            register: MmSaveStateRegister::Rax,
            cpu_index: 0,
        };

        // Phase 2 must present the same protocol pointer Phase 1 recorded.
        assert_eq!(validate_read_request(&holder, 7, 0x2000, 8, 0x5000), Err(Status::INVALID_PARAMETER));
        // Zero width and null buffers are rejected before anything is read.
        assert_eq!(validate_read_request(&holder, 7, 0x1000, 0, 0x5000), Err(Status::INVALID_PARAMETER));
        assert_eq!(validate_read_request(&holder, 7, 0x1000, 8, 0), Err(Status::INVALID_PARAMETER));
        // A width the register cannot satisfy is unsupported rather than a policy failure.
        assert_eq!(validate_read_request(&holder, 7, 0x1000, 16, 0x5000), Err(Status::UNSUPPORTED));
    }

    #[test]
    #[serial]
    fn test_validate_read_request_denies_buffers_outside_user_memory() {
        let holder = SaveStateAccessHolder {
            caller: 7,
            user_protocol: 0x1000,
            register: MmSaveStateRegister::Rax,
            cpu_index: 0,
        };

        // Without a page table no address can be proven user-owned, so the read is refused.
        *security_state().lock_page_table() = None;
        assert_eq!(validate_read_request(&holder, 7, 0x1000, 8, 0x5000), Err(Status::ACCESS_DENIED));
    }

    #[test]
    #[serial]
    fn test_save_state_read_phase2_requires_phase1() {
        assert!(security_state().lock_save_state_access().is_none());
        assert_eq!(save_state_read_phase2(0x1000, 8, 0x5000), Err(Status::INVALID_PARAMETER));
    }

    #[test]
    #[serial]
    fn test_save_state_read_phase2_consumes_the_staged_request() {
        // Phase 2 needs the metadata, so publish it as initialization would.
        let smram = FakeSmram::new(4);
        security_state().set_save_state_info(smram.info());

        // Staged as this CPU so the hand-off reaches the checks the test is about.
        assert_eq!(stage_read_request(current_apic_id(), 0x1000, 38, 0, 4), Ok(0));

        // The buffer cannot be proven user-owned without a page table.
        *security_state().lock_page_table() = None;
        assert_eq!(save_state_read_phase2(0x1000, 8, 0x5000), Err(Status::ACCESS_DENIED));

        // A failed Phase 2 still clears the hand-off, so a replay is rejected.
        assert_eq!(save_state_read_phase2(0x1000, 8, 0x5000), Err(Status::INVALID_PARAMETER));
    }

    #[test]
    fn test_actual_write_size_rejects_registers_without_layout() {
        // `LdtInfo` has no entry in either vendor's save-state map.
        assert_eq!(actual_write_size(MmSaveStateRegister::LdtInfo, 8), 0);
        assert_eq!(actual_write_size(MmSaveStateRegister::Rax, 0), 0);
        assert_eq!(actual_write_size(MmSaveStateRegister::Lma, 0), 0);
    }

    #[test]
    fn test_inspect_io_condition_maps_direction_to_policy_condition() {
        let constants = vendor_constants();
        let mut map = new_save_state_map();

        write_u32(&mut map[..], constants.io_info_offset as usize, io_trap_field(0xB2, true, 1));
        assert_eq!(inspect_io_condition(&SaveState::new(*map)), Some(SaveStateCondition::IoRead));

        write_u32(&mut map[..], constants.io_info_offset as usize, io_trap_field(0xB2, false, 1));
        assert_eq!(inspect_io_condition(&SaveState::new(*map)), Some(SaveStateCondition::IoWrite));

        write_u32(&mut map[..], constants.io_info_offset as usize, 0);
        assert_eq!(inspect_io_condition(&SaveState::new(*map)), None);
    }

    #[test]
    fn test_inspect_io_condition_decodes_with_the_view_layout() {
        // Bit 0 is the IN/OUT direction on AMD but the "SMI was an I/O trap" flag on Intel. Here
        // Intel decodes length 1 (bits 3:1) and type OUT (bits 7:4 clear), while AMD decodes a
        // valid record (bit 1) for a 32-bit IN.
        let field = (0xB2 << 16) | (1 << 1) | 1;
        let expected =
            if cfg!(feature = "save_state_amd") { SaveStateCondition::IoRead } else { SaveStateCondition::IoWrite };

        let mut map = new_save_state_map();
        write_u32(&mut map[..], vendor_constants().io_info_offset as usize, field);
        assert_eq!(inspect_io_condition(&SaveState::new(*map)), Some(expected));
    }

    #[cfg(not(feature = "save_state_amd"))]
    #[test]
    fn test_inspect_io_condition_ignores_a_legacy_save_state_revision() {
        // AMD always reports I/O info, so only the Intel layout can observe an unsupported revision.
        let constants = vendor_constants();
        let mut map = new_save_state_map();
        write_u32(&mut map[..], constants.smmrevid_offset as usize, constants.min_rev_id_io - 1);
        // The field records an IN trap, so a `None` below comes from the revision alone.
        write_u32(&mut map[..], constants.io_info_offset as usize, io_trap_field(0xB2, true, 1));
        let state = SaveState::new(*map);

        assert_eq!(inspect_io_condition(&state), None);

        // Without a trap condition, a descriptor that allows RAX only on an I/O read does not
        // apply, and the IO pseudo-register has nothing to report.
        let policy = build_policy(
            ACCESS_ATTR_ALLOW,
            &[descriptor(SaveStateField::Rax, RESOURCE_ATTR_COND_READ, SaveStateCondition::IoRead)],
        );
        let gate = gate_over(&policy);
        let mut out = [0u8; IO_INFO_SIZE];
        assert_eq!(
            read_gated_register(&state, &gate, MmSaveStateRegister::Rax, 8, &mut out),
            Err(Status::ACCESS_DENIED)
        );
        assert_eq!(read_gated_register(&state, &gate, MmSaveStateRegister::Io, 4, &mut out), Err(Status::NOT_FOUND));
    }

    #[test]
    fn test_read_gated_register_allows_a_permitted_register() {
        let rbx = SaveState::register_info(MmSaveStateRegister::Rbx).expect("RBX is mapped on both vendors");
        let mut map = new_save_state_map();
        write_u32(&mut map[..], rbx.lo_offset as usize, 0x0000_0042);
        write_u32(&mut map[..], rbx.hi_offset as usize, 0);

        // RBX is not policy-gated, so an empty allow-list still permits the read.
        let policy = build_policy(ACCESS_ATTR_ALLOW, &[]);
        let gate = gate_over(&policy);

        let mut out = [0u8; 8];
        assert_eq!(read_gated_register(&SaveState::new(*map), &gate, MmSaveStateRegister::Rbx, 8, &mut out), Ok(0));
        assert_eq!(u64::from_le_bytes(out), 0x42);
    }

    #[test]
    fn test_read_gated_register_denies_rax_without_a_matching_descriptor() {
        let map = new_save_state_map();
        let policy = build_policy(ACCESS_ATTR_ALLOW, &[]);
        let gate = gate_over(&policy);

        let mut out = [0u8; 8];
        assert_eq!(
            read_gated_register(&SaveState::new(*map), &gate, MmSaveStateRegister::Rax, 8, &mut out),
            Err(Status::ACCESS_DENIED)
        );
    }

    #[test]
    fn test_read_gated_register_allows_rax_with_an_unconditional_descriptor() {
        let rax = SaveState::register_info(MmSaveStateRegister::Rax).expect("RAX is mapped on both vendors");
        let mut map = new_save_state_map();
        write_u32(&mut map[..], rax.lo_offset as usize, 0x9999_0001);
        write_u32(&mut map[..], rax.hi_offset as usize, 0);

        let policy = build_policy(
            ACCESS_ATTR_ALLOW,
            &[descriptor(SaveStateField::Rax, RESOURCE_ATTR_READ, SaveStateCondition::Unconditional)],
        );
        let gate = gate_over(&policy);

        let mut out = [0u8; 8];
        assert_eq!(read_gated_register(&SaveState::new(*map), &gate, MmSaveStateRegister::Rax, 8, &mut out), Ok(0));
        assert_eq!(u64::from_le_bytes(out), 0x9999_0001);
    }

    #[test]
    fn test_read_gated_register_requires_an_io_trap_condition() {
        let map = new_save_state_map();
        let policy = build_policy(ACCESS_ATTR_DENY, &[]);
        let gate = gate_over(&policy);

        // No I/O trap recorded: the register has no value to report, so this is NOT_FOUND
        // rather than a policy denial.
        let mut out = [0u8; IO_INFO_SIZE];
        assert_eq!(
            read_gated_register(&SaveState::new(*map), &gate, MmSaveStateRegister::Io, 4, &mut out),
            Err(Status::NOT_FOUND)
        );
    }

    #[test]
    fn test_read_gated_register_matches_the_io_trap_condition() {
        const IO_PORT: u16 = 0xB2;
        const IO_DATA: u8 = 0x5A;

        let constants = vendor_constants();
        let io_field = io_trap_field(IO_PORT, true, 1);
        let mut map = new_save_state_map();
        write_u32(&mut map[..], constants.io_info_offset as usize, io_field);
        map[constants.rax_offset as usize] = IO_DATA;

        // Both the I/O trap field and RAX must clear the policy for an IO read.
        let policy = build_policy(
            ACCESS_ATTR_ALLOW,
            &[
                descriptor(SaveStateField::IoTrap, RESOURCE_ATTR_COND_READ, SaveStateCondition::IoRead),
                descriptor(SaveStateField::Rax, RESOURCE_ATTR_COND_READ, SaveStateCondition::IoRead),
            ],
        );
        let gate = gate_over(&policy);

        let mut out = [0u8; IO_INFO_SIZE];
        assert_eq!(read_gated_register(&SaveState::new(*map), &gate, MmSaveStateRegister::Io, 4, &mut out), Ok(0));

        let expected = MmSaveStateIoInfo {
            io_data: u64::from(IO_DATA),
            io_port: IO_PORT,
            io_width: IO_WIDTH_UINT8,
            io_type: IO_TYPE_INPUT,
            ..Default::default()
        };
        assert_eq!(out, expected.as_bytes());
    }

    #[test]
    fn test_read_gated_register_denies_an_io_read_with_the_wrong_condition() {
        let constants = vendor_constants();
        let mut map = new_save_state_map();
        write_u32(&mut map[..], constants.io_info_offset as usize, io_trap_field(0xB2, true, 1));

        // The descriptor only permits the read when an I/O *write* trapped.
        let policy = build_policy(
            ACCESS_ATTR_ALLOW,
            &[descriptor(SaveStateField::IoTrap, RESOURCE_ATTR_COND_READ, SaveStateCondition::IoWrite)],
        );
        let gate = gate_over(&policy);

        let mut out = [0u8; IO_INFO_SIZE];
        assert_eq!(
            read_gated_register(&SaveState::new(*map), &gate, MmSaveStateRegister::Io, 4, &mut out),
            Err(Status::ACCESS_DENIED)
        );
    }

    #[test]
    fn test_read_gated_register_reads_lma_without_policy_gating() {
        let constants = vendor_constants();
        let mut map = new_save_state_map();
        write_u64(&mut map[..], constants.efer_offset as usize, IA32_EFER_LMA);

        let policy = build_policy(ACCESS_ATTR_ALLOW, &[]);
        let gate = gate_over(&policy);

        let mut out = [0u8; 8];
        assert_eq!(read_gated_register(&SaveState::new(*map), &gate, MmSaveStateRegister::Lma, 8, &mut out), Ok(0));
        assert_eq!(u64::from_le_bytes(out), LMA_64BIT);
    }
}
