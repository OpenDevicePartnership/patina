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
    /// The SMBASE array (`u64[number_of_cpus]`).
    smbase_array: MemoryRange,
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
        if smbase_array == 0 || !smbase_array.is_multiple_of(core::mem::align_of::<u64>() as u64) {
            return Err(invalid);
        }
        let array = number_of_cpus
            .checked_mul(core::mem::size_of::<u64>())
            .and_then(|size| u64::try_from(size).ok())
            .and_then(|size| MemoryRange::new(smbase_array, size).ok())
            .ok_or(invalid)?;

        Ok(Self { number_of_cpus, smbase_array: array })
    }

    /// Returns the number of CPUs.
    pub(crate) fn number_of_cpus(&self) -> usize {
        self.number_of_cpus
    }

    /// Returns the address of the SMBASE array.
    pub(crate) fn smbase_array(&self) -> u64 {
        self.smbase_array.base()
    }

    /// Returns the range the SMBASE array occupies.
    pub(crate) fn smbase_array_range(&self) -> MemoryRange {
        self.smbase_array
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
    // `SaveStateInfo::new` already rejected a null, empty, misaligned, or overflowing array.
    let array = info.smbase_array_range();
    if !is_inside_mmram(&array) {
        return Err(SaveStateValidationError::UnusableSmBaseArray { base: smbase_array, count: number_of_cpus }.into());
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
        if let Some(holder) = access.take() {
            holder
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
        if let Err(err) = gate.is_save_state_read_allowed(field, width as usize, condition) {
            log::error!("SAVE_STATE_READ2: Policy denied read of {register:?} (field {field:?}): {err:?}");
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
