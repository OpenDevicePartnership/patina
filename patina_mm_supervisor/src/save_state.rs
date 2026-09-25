//! Save State Read Operations for the MM Supervisor Syscall Dispatcher
//!
//! Implements the two-phase save state read protocol used by the
//! `EFI_MM_CPU_PROTOCOL.ReadSaveState()` user-space API.
//!
//! **Phase 1** (`SyscallIndex::SaveStateRead`): stores the requested register
//! and CPU index in a per-BSP holder.
//!
//! **Phase 2** (`SyscallIndex::SaveStateRead2`): validates the request against
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
//! ## Vendor Selection
//!
//! The SMRAM save state layout (Intel vs AMD) is selected **at build time**
//! via Cargo features on the `patina` crate (`save_state_intel` or
//! `save_state_amd`).  All vendor-specific register offsets and I/O field
//! parsing live in the SDK; this module is vendor-agnostic.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::mm_policy::{SaveStateCondition, SaveStateField, gate::PolicyGate};
use patina::standard::efi::Status;
use patina_internal_cpu::save_state::{
    self, IA32_EFER_LMA, IO_INFO_SIZE, IO_TYPE_INPUT, LMA_32BIT, LMA_64BIT, MmSaveStateIoInfo, MmSaveStateRegister,
    RegisterInfo,
};
use zerocopy::IntoBytes;

use crate::{
    PageOwnership,
    intrinsics::current_apic_id,
    privilege_mgmt::SyscallResult,
    query_address_ownership,
    runtime::with_user_access,
    state::{init_state, security_state},
};

/// Size in bytes of one `SMRAM_SAVE_STATE_MAP` region.
///
/// The relocation code sets every CPU's save-state size to
/// `sizeof(SMRAM_SAVE_STATE_MAP)` — a fixed 0x400-byte region spanning
/// SMBASE+0x7C00..SMBASE+0x8000 (Intel SDM Vol 3C, §34.4). Because it is
/// identical for every CPU, it is a constant here rather than a per-CPU array
/// passed through the HOB.
const SMRAM_SAVE_STATE_MAP_SIZE: u64 = 0x400;

/// Offset of the `SMRAM_SAVE_STATE_MAP` from a CPU's SMBASE.
///
/// A fixed architectural offset (Intel SDM Vol 3C, §34.4;
/// `SMRAM_SAVE_STATE_MAP_OFFSET` in `MdePkg`). The per-CPU save-state region base
/// is `sm_base[i] + SMRAM_SAVE_STATE_MAP_OFFSET`, derived here so the loader only
/// has to pass the raw SMBASE array.
const SMRAM_SAVE_STATE_MAP_OFFSET: u64 = 0xfc00;

/// Per-CPU save-state metadata needed by the save-state read syscall.
///
/// Assembled at initialization from two public sources instead of the private
/// `SMM_CPU_PRIVATE_DATA` layout:
///
/// - `number_of_cpus` comes from the MP Information HOB (`gMpInformationHobGuid`).
/// - `sm_base` (the per-CPU SMBASE array) is passed through the MM Supervisor
///   `PassDown` HOB. The save-state region base is derived as
///   `sm_base[i] + SMRAM_SAVE_STATE_MAP_OFFSET` with the fixed
///   [`SMRAM_SAVE_STATE_MAP_SIZE`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct SaveStateInfo {
    /// Number of CPUs (from `MP_INFORMATION_HOB_DATA.NumberOfProcessors`).
    pub(crate) number_of_cpus: u64,
    /// Pointer to the per-CPU SMBASE array (`u64[number_of_cpus]`).
    pub(crate) sm_base: u64,
}

/// Why the per-CPU save-state regions the `PassDown` HOB describes cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveStateValidationError {
    /// The SMBASE array is null, empty, misaligned, overflowing, or not entirely inside MMRAM.
    UnusableSmBaseArray {
        /// Base address of the array.
        base: u64,
        /// Number of entries the array was said to hold.
        count: u64,
    },
    /// The SMBASE array is inside MMRAM but is not mapped supervisor-only.
    SmBaseArrayNotSupervisorOwned {
        /// Base address of the array.
        base: u64,
        /// Number of entries the array was said to hold.
        count: u64,
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

/// Requires every per-CPU save-state region the `PassDown` HOB describes to lie inside MMRAM and
/// to be mapped supervisor-only.
///
/// The save-state syscall turns `sm_base[i] + SMRAM_SAVE_STATE_MAP_OFFSET` into a slice and hands
/// selected bytes of it back to Ring 3. The MM IPL supplies those SMBASEs and sits outside the
/// supervisor's trust boundary, so an entry that is never checked is an arbitrary read primitive:
/// proving the array itself is in MMRAM says nothing about where its contents point. Checking all
/// of them once, here, is what lets [`get_save_state_view`] treat them as sound later.
///
/// Being inside MMRAM is not on its own enough. A save-state map holds a CPU's saved context, and
/// the syscall exists so Ring 3 can read parts of it under policy; a page Ring 3 can reach
/// directly hands it the whole map and lets it rewrite what the supervisor is about to read.
/// Supervisor-only mapping is what keeps the syscall the only way in.
pub(crate) fn validate_save_state_regions(
    sm_base: u64,
    number_of_cpus: u64,
    is_inside_mmram: impl Fn(u64, u64) -> bool,
    is_supervisor_owned: impl Fn(u64, u64) -> bool,
) -> Result<(), SaveStateValidationError> {
    let unusable_array = SaveStateValidationError::UnusableSmBaseArray { base: sm_base, count: number_of_cpus };

    let count = usize::try_from(number_of_cpus).map_err(|_| unusable_array)?;
    let array_size = number_of_cpus
        .checked_mul(core::mem::size_of::<u64>() as u64)
        .filter(|size| *size != 0)
        .ok_or(unusable_array)?;
    // Alignment is required as well as containment, because the array is read back as `&[u64]`.
    if sm_base == 0
        || !sm_base.is_multiple_of(core::mem::align_of::<u64>() as u64)
        || sm_base.checked_add(array_size).is_none()
        || !is_inside_mmram(sm_base, array_size)
    {
        return Err(unusable_array);
    }
    if !is_supervisor_owned(sm_base, array_size) {
        return Err(SaveStateValidationError::SmBaseArrayNotSupervisorOwned { base: sm_base, count: number_of_cpus });
    }

    // SAFETY: the checks above establish that `sm_base` is a non-null, aligned array of `count`
    // initialized `u64` entries lying entirely inside supervisor-owned MMRAM. The MM IPL
    // populates it before launching the supervisor and it stays resident for the supervisor's
    // lifetime.
    let sm_bases = unsafe { core::slice::from_raw_parts(sm_base as *const u64, count) };

    for (cpu_index, &smbase) in sm_bases.iter().enumerate() {
        let Some(map_base) = smbase
            .checked_add(SMRAM_SAVE_STATE_MAP_OFFSET)
            .filter(|_| smbase != 0)
            .filter(|base| base.checked_add(SMRAM_SAVE_STATE_MAP_SIZE).is_some())
            .filter(|base| is_inside_mmram(*base, SMRAM_SAVE_STATE_MAP_SIZE))
        else {
            return Err(SaveStateValidationError::UnusableSaveStateRegion { cpu_index, smbase });
        };
        if !is_supervisor_owned(map_base, SMRAM_SAVE_STATE_MAP_SIZE) {
            return Err(SaveStateValidationError::SaveStateRegionNotSupervisorOwned { cpu_index, smbase });
        }
    }

    Ok(())
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
fn stage_read_request(caller: u32, protocol: u64, register_raw: u64, cpu_index: u64, num_cpus: u64) -> SyscallResult {
    let Some(register) = MmSaveStateRegister::from_u64(register_raw) else {
        log::error!("SAVE_STATE_READ: Unknown register value: {register_raw}");
        return Err(Status::INVALID_PARAMETER);
    };

    if cpu_index >= num_cpus {
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

    let write_size = validate_read_request(&holder, current_apic_id(), protocol, width, buffer)?;

    let mut out = [0u8; IO_INFO_SIZE];
    let out = out.get_mut(..write_size).ok_or(Status::BUFFER_TOO_SMALL)?;

    if holder.register == MmSaveStateRegister::ProcessorId {
        // Special case: PROCESSOR_ID — always allowed, no policy check.
        read_processor_id(holder.cpu_index, out)?;
    } else {
        let view = get_save_state_view(save_state_info()?, holder.cpu_index).inspect_err(|status| {
            log::error!(
                "SAVE_STATE_READ2: Unable to get save state view for CPU {}: {:#x}",
                holder.cpu_index,
                status.as_usize()
            );
        })?;

        let Some(gate) = security_state().policy_gate() else {
            log::error!("SAVE_STATE_READ2: Policy gate not initialized");
            return Err(Status::NOT_READY);
        };

        read_gated_register(&view, gate, holder.register, width, out)?;
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

    match query_address_ownership(buffer, write_size as u64) {
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
/// register value from `view` into `out`.
fn read_gated_register(
    view: &SaveStateView,
    gate: &PolicyGate,
    register: MmSaveStateRegister,
    width: u64,
    out: &mut [u8],
) -> SyscallResult {
    let policy_checks = policy_checks_for_register(register);
    let condition = if policy_checks.is_empty() { None } else { inspect_io_condition(view) };

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

    // Dispatch to the appropriate read handler.  Each handler reads from the
    // save state `view` and writes into the validated `out` buffer using only
    // safe slice operations.
    match register {
        MmSaveStateRegister::Io => read_io_register(view, out),
        MmSaveStateRegister::Lma => read_lma_register(view, width, out),
        _ => read_architectural_register(view, register, width, out),
    }
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
fn get_number_of_cpus() -> Result<u64, Status> {
    Ok(save_state_info()?.number_of_cpus)
}

/// A read-only byte view over a CPU's SMRAM save state region.
struct SaveStateView {
    bytes: &'static [u8],
}

impl SaveStateView {
    /// Creates a view over `size` bytes of the save state region at `base`.
    ///
    /// ## Safety
    ///
    /// `base` must point to a readable SMRAM save state region of at least
    /// `size` bytes that is not mutated for the lifetime of the view and lives
    /// for the duration of the program.
    unsafe fn new(base: *const u8, size: usize) -> Self {
        // SAFETY: guaranteed by the caller's contract.
        Self { bytes: unsafe { core::slice::from_raw_parts(base, size) } }
    }

    /// Reads the byte at `offset`. Panics if `offset` is out of range.
    fn read_u8(&self, offset: usize) -> u8 {
        *self.bytes.get(offset).expect("save state offset within region")
    }

    /// Reads a little-endian `u16` at `offset`. Panics on an out-of-range offset (see `read_u8`).
    fn read_u16(&self, offset: usize) -> u16 {
        let bytes = self.bytes.get(offset..offset + 2).expect("save state offset within region");
        u16::from_le_bytes(bytes.try_into().expect("slice length is 2"))
    }

    /// Reads a little-endian `u32` at `offset`. Panics on an out-of-range offset (see `read_u8`).
    fn read_u32(&self, offset: usize) -> u32 {
        let bytes = self.bytes.get(offset..offset + 4).expect("save state offset within region");
        u32::from_le_bytes(bytes.try_into().expect("slice length is 4"))
    }

    /// Reads a little-endian `u64` at `offset`. Panics on an out-of-range offset (see `read_u8`).
    fn read_u64(&self, offset: usize) -> u64 {
        let bytes = self.bytes.get(offset..offset + 8).expect("save state offset within region");
        u64::from_le_bytes(bytes.try_into().expect("slice length is 8"))
    }
}

/// Builds a [`SaveStateView`] for the given CPU index.
///
/// The region base is derived from the CPU's SMBASE as
/// `sm_base[cpu_index] + SMRAM_SAVE_STATE_MAP_OFFSET`, with the SMBASE array
/// passed through the MM Supervisor `PassDown` HOB. The region length is the fixed
/// [`SMRAM_SAVE_STATE_MAP_SIZE`].
fn get_save_state_view(info: SaveStateInfo, cpu_index: u64) -> Result<SaveStateView, Status> {
    let num_cpus = info.number_of_cpus;
    if cpu_index >= num_cpus {
        log::error!("Save state read: CPU index {cpu_index} >= NumberOfCpus {num_cpus}");
        return Err(Status::INVALID_PARAMETER);
    }

    if info.sm_base == 0 {
        log::error!("SmBase array pointer is null");
        return Err(Status::NOT_READY);
    }

    // The SMBASE array holds `num_cpus` per-CPU SMBASE values set up by the
    // relocation code. The save-state region base is `SmBase + 0xfc00` and every
    // region is the fixed `SMRAM_SAVE_STATE_MAP_SIZE`.
    //
    // SAFETY: `validate_save_state_regions` proved during initialization that `sm_base` is a
    // non-null, aligned array of at least `num_cpus` initialized `u64` entries inside MMRAM, and
    // MMRAM is not writable from outside MM, so it still describes that array here.
    let sm_bases = unsafe { core::slice::from_raw_parts(info.sm_base as *const u64, num_cpus as usize) };

    let smbase = *sm_bases.get(cpu_index as usize).ok_or(Status::INVALID_PARAMETER)?;
    if smbase == 0 {
        log::error!("SmBase[{cpu_index}] is null");
        return Err(Status::INVALID_PARAMETER);
    }
    let base = smbase.checked_add(SMRAM_SAVE_STATE_MAP_OFFSET).ok_or(Status::INVALID_PARAMETER)?;

    // SAFETY: `validate_save_state_regions` proved during initialization that this entry's
    // `SMRAM_SAVE_STATE_MAP_SIZE` region lies inside MMRAM, which is stable while this SMI is
    // serviced and lives for the program's duration.
    Ok(unsafe { SaveStateView::new(base as *const u8, SMRAM_SAVE_STATE_MAP_SIZE as usize) })
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
            if let Some(info) = save_state::register_info(register) {
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
/// The `ProcessorId` (APIC ID) is read from the supervisor-owned [`CpuManager`](crate::cpu::CpuManager).
fn read_processor_id(cpu_index: u64, out: &mut [u8]) -> SyscallResult {
    let lookup = init_state().processor_id_lookup_fn().ok_or(Status::NOT_READY)?;
    let processor_id = lookup(cpu_index as usize).ok_or(Status::NOT_FOUND)?;

    // Write the 8-byte ProcessorId to the user buffer.
    out.get_mut(..8).ok_or(Status::BUFFER_TOO_SMALL)?.copy_from_slice(&processor_id.to_le_bytes());

    log::debug!("PROCESSOR_ID: CPU {cpu_index} = 0x{processor_id:x}");
    Ok(0)
}

/// Inspects the I/O condition (IN vs OUT) from the save state for policy checking.
///
/// Reads the vendor-specific IO field from the CPU's save state and uses the
/// SDK's [`save_state::parse_io_field`] to determine whether the I/O trap was
/// caused by an IN or OUT instruction.
fn inspect_io_condition(view: &SaveStateView) -> Option<SaveStateCondition> {
    let vc = save_state::vendor_constants();

    // Verify the save state revision supports IO info before reading the field.
    let smm_rev_id = view.read_u32(vc.smmrevid_offset as usize);
    assert!(
        save_state::io_info_supported(smm_rev_id),
        "SMMRevId {smm_rev_id:#x} does not expose I/O info; legacy hardware is not supported"
    );

    // Read the vendor-specific IO information field.
    let io_field = view.read_u32(vc.io_info_offset as usize);
    // Intentionally commented out to avoid info leakage.
    // log::info!("Inspecting IO condition: IO field = 0x{:x}", io_field);

    // Use the SDK's vendor-specific parser.
    let parsed = save_state::parse_io_field(io_field)?;
    match parsed.io_type {
        IO_TYPE_INPUT => Some(SaveStateCondition::IoRead),
        _ => Some(SaveStateCondition::IoWrite),
    }
}

/// Reads an architectural register from the vendor save state map into `out`.
fn read_architectural_register(
    view: &SaveStateView,
    register: MmSaveStateRegister,
    width: u64,
    out: &mut [u8],
) -> SyscallResult {
    let Some(info) = save_state::register_info(register) else {
        log::error!("Register {register:?} not found in save state map");
        return Err(Status::NOT_FOUND);
    };

    read_register_field(view, register, info, width, out)
}

/// Copies `width` bytes of the field described by `info` out of `view` into `out`.
///
/// `register` is carried through for diagnostics only.
fn read_register_field(
    view: &SaveStateView,
    register: MmSaveStateRegister,
    info: RegisterInfo,
    width: u64,
    out: &mut [u8],
) -> SyscallResult {
    let lo = info.lo_offset as usize;
    if width == 0 {
        log::error!("Register {register:?} does not support 0-byte read");
        return Err(Status::NOT_FOUND);
    } else if width == 2 {
        if info.native_width < 2 {
            log::error!("Register {register:?} does not support 2-byte read");
            return Err(Status::INVALID_PARAMETER);
        }
        // Read the low 2 bytes (AMD segment selectors, DT limits).
        out.get_mut(..2).ok_or(Status::BUFFER_TOO_SMALL)?.copy_from_slice(&view.read_u16(lo).to_le_bytes());
    } else if width == 4 {
        if info.native_width < 4 {
            log::error!("Register {register:?} does not support 4-byte read");
            return Err(Status::INVALID_PARAMETER);
        }
        // Read the low 4 bytes.
        out.get_mut(..4).ok_or(Status::BUFFER_TOO_SMALL)?.copy_from_slice(&view.read_u32(lo).to_le_bytes());
    } else if width == 8 {
        if info.native_width != 8 {
            log::error!("Register {register:?} does not support 8-byte read");
            return Err(Status::INVALID_PARAMETER);
        }
        // Read lo u32 then hi u32 (handles both contiguous and split
        // layouts) and write them as two adjacent u32 (matching C
        // split-register behaviour).
        out.get_mut(..4).ok_or(Status::BUFFER_TOO_SMALL)?.copy_from_slice(&view.read_u32(lo).to_le_bytes());
        out.get_mut(4..8)
            .ok_or(Status::BUFFER_TOO_SMALL)?
            .copy_from_slice(&view.read_u32(info.hi_offset as usize).to_le_bytes());
    } else {
        log::error!("Register {register:?} does not support {width}-byte read");
        return Err(Status::INVALID_PARAMETER);
    }

    Ok(0)
}

/// Reads the IO pseudo-register and writes an `EFI_MM_SAVE_STATE_IO_INFO`
/// structure to `out`.
///
/// The IO pseudo-register provides information about the I/O instruction that
/// triggered the SMI, including the port, width, direction, and data value.
fn read_io_register(view: &SaveStateView, out: &mut [u8]) -> SyscallResult {
    let vc = save_state::vendor_constants();

    // 1. Read SMMRevId to verify IO info is available.
    let smm_rev_id = view.read_u32(vc.smmrevid_offset as usize);
    assert!(
        save_state::io_info_supported(smm_rev_id),
        "SMMRevId {smm_rev_id:#x} does not expose I/O info; legacy hardware is not supported"
    );

    // 2. Read the vendor-specific IO information field and parse it.
    let io_field = view.read_u32(vc.io_info_offset as usize);
    let parsed = if let Some(p) = save_state::parse_io_field(io_field) {
        p
    } else {
        log::error!("IO_READ: IO field 0x{io_field:x} did not indicate a valid I/O trap");
        return Err(Status::NOT_FOUND);
    };

    // 3. Read I/O data from RAX (only the significant bytes).
    let rax = vc.rax_offset as usize;
    let io_data: u64 = match parsed.byte_count {
        1 => u64::from(view.read_u8(rax)),
        2 => u64::from(view.read_u16(rax)),
        4 => u64::from(view.read_u32(rax)),
        _ => {
            log::error!("IO_READ: Unsupported byte count: {}", parsed.byte_count);
            0
        }
    };

    // 4. Serialize the EFI_MM_SAVE_STATE_IO_INFO structure into the output buffer.
    let io_info = MmSaveStateIoInfo {
        io_data,
        io_port: parsed.io_port,
        _pad0: [0; 2],
        io_width: parsed.io_width,
        io_type: parsed.io_type,
        _pad1: [0; 4],
    };
    let out = out.get_mut(..IO_INFO_SIZE).ok_or(Status::BUFFER_TOO_SMALL)?;
    out.copy_from_slice(io_info.as_bytes());

    Ok(0)
}

/// Reads the LMA pseudo-register (processor Long Mode Active state) into `out`.
///
/// Returns `LMA_32BIT` (32) or `LMA_64BIT` (64) depending on the `IA32_EFER.LMA`
/// bit in the save state.
fn read_lma_register(view: &SaveStateView, width: u64, out: &mut [u8]) -> SyscallResult {
    let vc = save_state::vendor_constants();

    // AMD64 always operates in 64-bit mode during SMM.
    let lma_value = if vc.lma_always_64 {
        LMA_64BIT
    } else {
        // Read IA32_EFER from the save state.
        let efer = view.read_u64(vc.efer_offset as usize);
        if (efer & IA32_EFER_LMA) != 0 { LMA_64BIT } else { LMA_32BIT }
    };

    if width == 4 {
        out.get_mut(..4).ok_or(Status::BUFFER_TOO_SMALL)?.copy_from_slice(&(lma_value as u32).to_le_bytes());
    } else if width == 8 {
        out.get_mut(..8).ok_or(Status::BUFFER_TOO_SMALL)?.copy_from_slice(&lma_value.to_le_bytes());
    } else {
        return Err(Status::INVALID_PARAMETER);
    }

    Ok(0)
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::mm_policy::{
        ACCESS_ATTR_ALLOW, ACCESS_ATTR_DENY, RESOURCE_ATTR_COND_READ, RESOURCE_ATTR_READ, SaveStateDescriptorV1_0,
        TYPE_SAVE_STATE,
    };
    use patina_internal_cpu::save_state::IO_TYPE_OUTPUT;
    use serial_test::serial;

    struct TestPlatform;

    impl crate::PlatformInfo for TestPlatform {}

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
            let region_size = (SMRAM_SAVE_STATE_MAP_OFFSET + SMRAM_SAVE_STATE_MAP_SIZE) as usize;
            let region = vec![0u8; region_size].into_boxed_slice();
            let smbase = region.as_ptr() as u64;
            Self { region, sm_bases: vec![smbase; num_cpus].into_boxed_slice() }
        }

        /// Returns the metadata the save-state syscall would receive from the `PassDown` HOB.
        fn info(&self) -> SaveStateInfo {
            SaveStateInfo { number_of_cpus: self.sm_bases.len() as u64, sm_base: self.sm_bases.as_ptr() as u64 }
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
    fn inside_any(allowed: Vec<(u64, u64)>) -> impl Fn(u64, u64) -> bool {
        move |addr, size| {
            allowed
                .iter()
                .any(|&(base, len)| addr >= base && addr.checked_add(size).is_some_and(|end| end <= base + len))
        }
    }

    /// Answers "supervisor-owned" for every range, isolating the containment rules.
    fn all_supervisor_owned(_addr: u64, _size: u64) -> bool {
        true
    }

    #[test]
    fn test_validate_save_state_regions_accepts_regions_inside_mmram() {
        let smram = FakeSmram::new(2);
        let info = smram.info();

        assert_eq!(
            validate_save_state_regions(
                info.sm_base,
                info.number_of_cpus,
                inside_any(fake_smram_ranges(&smram)),
                all_supervisor_owned
            ),
            Ok(())
        );
    }

    #[test]
    fn test_validate_save_state_regions_rejects_an_array_that_ring_3_can_reach() {
        // An array Ring 3 can write lets it choose where every save-state read lands.
        let smram = FakeSmram::new(2);
        let info = smram.info();

        assert_eq!(
            validate_save_state_regions(
                info.sm_base,
                info.number_of_cpus,
                inside_any(fake_smram_ranges(&smram)),
                |_, _| false
            ),
            Err(SaveStateValidationError::SmBaseArrayNotSupervisorOwned { base: info.sm_base, count: 2 })
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
        let owned = move |addr: u64, size: u64| addr == info.sm_base && size == array_size;

        assert_eq!(
            validate_save_state_regions(
                info.sm_base,
                info.number_of_cpus,
                inside_any(fake_smram_ranges(&smram)),
                owned
            ),
            Err(SaveStateValidationError::SaveStateRegionNotSupervisorOwned {
                cpu_index: 0,
                smbase: smram.sm_bases[0]
            })
        );
    }

    #[test]
    fn test_validate_save_state_regions_rejects_an_array_outside_mmram_without_reading_it() {
        // The array has to be refused before it is dereferenced, so this points at an address
        // that would fault if the check were done in the wrong order.
        let result = validate_save_state_regions(0xdead_0000, 4, |_, _| false, all_supervisor_owned);

        assert_eq!(result, Err(SaveStateValidationError::UnusableSmBaseArray { base: 0xdead_0000, count: 4 }));
    }

    #[test]
    fn test_validate_save_state_regions_rejects_an_unusable_array() {
        let smram = FakeSmram::new(2);
        let sm_base = smram.info().sm_base;

        // Containment is answered yes throughout, so each case is rejected on its own ground:
        // a null base, an empty array, and an extent that overflows.
        for (base, count) in [(0, 2), (sm_base, 0), (u64::MAX - 7, 2)] {
            assert_eq!(
                validate_save_state_regions(base, count, |_, _| true, all_supervisor_owned),
                Err(SaveStateValidationError::UnusableSmBaseArray { base, count }),
                "array at 0x{base:x} with {count} entries should be rejected"
            );
        }
    }

    #[test]
    fn test_validate_save_state_regions_rejects_a_misaligned_array() {
        // The array is read back as `&[u64]`, so a misaligned base is refused even when its
        // extent is acceptable.
        let smram = FakeSmram::new(2);
        let base = smram.info().sm_base + 1;

        assert_eq!(
            validate_save_state_regions(base, 2, |_, _| true, all_supervisor_owned),
            Err(SaveStateValidationError::UnusableSmBaseArray { base, count: 2 })
        );
    }

    #[test]
    fn test_validate_save_state_regions_rejects_an_entry_outside_mmram() {
        // This is the arbitrary-read case: an in-MMRAM array whose entry points anywhere the
        // producer likes.
        let mut smram = FakeSmram::new(2);
        smram.set_smbase(1, 0x1000);

        assert_eq!(
            validate_save_state_regions(
                smram.info().sm_base,
                2,
                inside_any(fake_smram_ranges(&smram)),
                all_supervisor_owned
            ),
            Err(SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 1, smbase: 0x1000 })
        );
    }

    #[test]
    fn test_validate_save_state_regions_rejects_a_null_entry() {
        let mut smram = FakeSmram::new(2);
        smram.set_smbase(0, 0);

        assert_eq!(
            validate_save_state_regions(
                smram.info().sm_base,
                2,
                inside_any(fake_smram_ranges(&smram)),
                all_supervisor_owned
            ),
            Err(SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 0, smbase: 0 })
        );
    }

    #[test]
    fn test_validate_save_state_regions_rejects_an_entry_whose_region_overflows() {
        let mut smram = FakeSmram::new(1);
        smram.set_smbase(0, u64::MAX);

        assert_eq!(
            validate_save_state_regions(smram.info().sm_base, 1, |_, _| true, all_supervisor_owned),
            Err(SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 0, smbase: u64::MAX })
        );
    }

    /// Builds a zeroed save-state map whose `SMMRevId` advertises I/O trap support.
    fn new_save_state_map() -> Box<[u8; SMRAM_SAVE_STATE_MAP_SIZE as usize]> {
        let mut map = Box::new([0u8; SMRAM_SAVE_STATE_MAP_SIZE as usize]);
        let constants = save_state::vendor_constants();
        write_u32(&mut map[..], constants.smmrevid_offset as usize, constants.min_rev_id_io);
        map
    }

    /// Creates a view over `map`, which the caller must keep alive and unmodified.
    fn view_over(map: &[u8]) -> SaveStateView {
        // SAFETY: the caller keeps `map` alive and immutable for the lifetime of the view.
        unsafe { SaveStateView::new(map.as_ptr(), map.len()) }
    }

    fn write_u32(buffer: &mut [u8], offset: usize, value: u32) {
        buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(buffer: &mut [u8], offset: usize, value: u64) {
        buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// Encodes an I/O trap field that the active vendor decodes with the requested direction
    /// and transfer size.
    ///
    /// Intel (`IOMisc`) and AMD (`IO_DWord`) disagree on the meaning of bit 0, so an encoding
    /// for each layout is probed against the active parser.
    fn io_trap_field(port: u16, input: bool, byte_count: usize) -> u32 {
        let port_bits = u32::from(port) << 16;
        // Intel: SmiFlag | Length (bits 3:1) | Type IN(1)/OUT(0) (bits 7:4).
        let intel = port_bits | (u32::from(input) << 4) | ((byte_count as u32) << 1) | 1;
        // AMD: Valid (bit 1) | SZ8 (bit 4) / SZ16 (bit 5) / SZ32 (default) | Direction (bit 0).
        let amd_size = match byte_count {
            1 => 1 << 4,
            2 => 1 << 5,
            _ => 0,
        };
        let amd = port_bits | amd_size | (1 << 1) | u32::from(input);

        let expected = if input { IO_TYPE_INPUT } else { IO_TYPE_OUTPUT };
        [intel, amd]
            .into_iter()
            .find(|field| {
                save_state::parse_io_field(*field).is_some_and(|p| p.io_type == expected && p.byte_count == byte_count)
            })
            .expect("an I/O trap encoding exists for the active vendor")
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
        unsafe { PolicyGate::new(policy.as_ptr() as *const u8, core::mem::size_of_val(policy)) }
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
        static SUPERVISOR: crate::MmSupervisorCore<TestPlatform, 4> = crate::MmSupervisorCore::new();

        assert_eq!(SUPERVISOR.cpu_manager().register_cpu(0x20, 2, false), Some(2));

        let mut out = [0u8; 8];
        assert_eq!(read_processor_id(2, &mut out), Err(Status::NOT_READY));

        assert!(SUPERVISOR.set_instance());

        assert_eq!(read_processor_id(2, &mut out), Ok(0));
        assert_eq!(u64::from_le_bytes(out), 0x20);
        assert_eq!(read_processor_id(1, &mut out), Err(Status::NOT_FOUND));
        assert_eq!(read_processor_id(2, &mut out[..4]), Err(Status::BUFFER_TOO_SMALL));
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
    fn test_save_state_io_register_serialization() {
        const IO_PORT: u16 = 0xB2;
        const IO_DATA: u8 = 0x5A;

        let constants = save_state::vendor_constants();
        let mut save_state_bytes = Box::new([0u8; SMRAM_SAVE_STATE_MAP_SIZE as usize]);
        let revision = constants.min_rev_id_io;
        save_state_bytes[constants.smmrevid_offset as usize..constants.smmrevid_offset as usize + 4]
            .copy_from_slice(&revision.to_le_bytes());

        // This encodes a valid one-byte IN for both the Intel and AMD save-state layouts.
        let io_field = (u32::from(IO_PORT) << 16) | (1 << 4) | (1 << 1) | 1;
        save_state_bytes[constants.io_info_offset as usize..constants.io_info_offset as usize + 4]
            .copy_from_slice(&io_field.to_le_bytes());
        save_state_bytes[constants.rax_offset as usize] = IO_DATA;

        // SAFETY: the boxed save-state map remains alive and immutable while `view` is used.
        let view = unsafe { SaveStateView::new(save_state_bytes.as_ptr(), save_state_bytes.len()) };
        let mut out = [0xFF; IO_INFO_SIZE];

        assert_eq!(read_io_register(&view, &mut out), Ok(0));

        let parsed = save_state::parse_io_field(io_field).unwrap();
        let expected = MmSaveStateIoInfo {
            io_data: u64::from(IO_DATA),
            io_port: IO_PORT,
            _pad0: [0; 2],
            io_width: parsed.io_width,
            io_type: parsed.io_type,
            _pad1: [0; 4],
        };
        assert_eq!(out, expected.as_bytes());
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
    fn test_save_state_view_reads_little_endian_fields() {
        let mut map = new_save_state_map();
        map[0x10] = 0xA5;
        map[0x20..0x22].copy_from_slice(&0xBEEFu16.to_le_bytes());
        write_u32(&mut map[..], 0x30, 0xDEAD_BEEF);
        write_u64(&mut map[..], 0x40, 0x0123_4567_89AB_CDEF);

        let view = view_over(map.as_slice());

        assert_eq!(view.read_u8(0x10), 0xA5);
        assert_eq!(view.read_u16(0x20), 0xBEEF);
        assert_eq!(view.read_u32(0x30), 0xDEAD_BEEF);
        assert_eq!(view.read_u64(0x40), 0x0123_4567_89AB_CDEF);
    }

    #[test]
    #[should_panic(expected = "save state offset within region")]
    fn test_save_state_view_rejects_offset_past_region() {
        let map = new_save_state_map();
        view_over(map.as_slice()).read_u8(SMRAM_SAVE_STATE_MAP_SIZE as usize);
    }

    #[test]
    fn test_save_state_view_derived_from_smbase_array() {
        let mut smram = FakeSmram::new(2);
        smram.map_mut()[0x10] = 0x77;
        let info = smram.info();

        let view = get_save_state_view(info, 1).expect("view for a valid CPU index");
        assert_eq!(view.read_u8(0x10), 0x77);
    }

    #[test]
    fn test_save_state_view_rejects_invalid_metadata() {
        let mut smram = FakeSmram::new(2);
        let info = smram.info();

        // CPU index beyond the reported CPU count.
        assert_eq!(get_save_state_view(info, 2).err(), Some(Status::INVALID_PARAMETER));

        // A null SMBASE entry means the CPU was never relocated.
        smram.set_smbase(0, 0);
        assert_eq!(get_save_state_view(smram.info(), 0).err(), Some(Status::INVALID_PARAMETER));

        // A null SMBASE array means the PassDown HOB was never processed.
        let no_array = SaveStateInfo { number_of_cpus: 2, sm_base: 0 };
        assert_eq!(get_save_state_view(no_array, 0).err(), Some(Status::NOT_READY));
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
    fn test_read_architectural_register_widths() {
        let rax = save_state::register_info(MmSaveStateRegister::Rax).expect("RAX is mapped on both vendors");
        let mut map = new_save_state_map();
        write_u32(&mut map[..], rax.lo_offset as usize, 0x1122_3344);
        write_u32(&mut map[..], rax.hi_offset as usize, 0x5566_7788);
        let view = view_over(map.as_slice());

        let mut out = [0u8; 8];
        assert_eq!(read_architectural_register(&view, MmSaveStateRegister::Rax, 2, &mut out[..2]), Ok(0));
        assert_eq!(u16::from_le_bytes(out[..2].try_into().unwrap()), 0x3344);

        assert_eq!(read_architectural_register(&view, MmSaveStateRegister::Rax, 4, &mut out[..4]), Ok(0));
        assert_eq!(u32::from_le_bytes(out[..4].try_into().unwrap()), 0x1122_3344);

        assert_eq!(read_architectural_register(&view, MmSaveStateRegister::Rax, 8, &mut out), Ok(0));
        assert_eq!(u64::from_le_bytes(out), 0x5566_7788_1122_3344);
    }

    #[test]
    fn test_read_architectural_register_rejects_unmapped_register() {
        let map = new_save_state_map();
        let view = view_over(map.as_slice());
        let mut out = [0u8; 8];

        // `LdtInfo` is absent from both vendor maps, and pseudo-registers are handled elsewhere.
        assert_eq!(
            read_architectural_register(&view, MmSaveStateRegister::LdtInfo, 8, &mut out),
            Err(Status::NOT_FOUND)
        );
        assert_eq!(read_architectural_register(&view, MmSaveStateRegister::Io, 8, &mut out), Err(Status::NOT_FOUND));
    }

    #[test]
    fn test_read_register_field_enforces_native_width() {
        let map = new_save_state_map();
        let view = view_over(map.as_slice());
        let mut out = [0u8; 8];

        let byte = RegisterInfo { lo_offset: 0x10, hi_offset: 0, native_width: 1 };
        let word = RegisterInfo { lo_offset: 0x10, hi_offset: 0, native_width: 2 };
        let dword = RegisterInfo { lo_offset: 0x10, hi_offset: 0x14, native_width: 4 };
        let reg = MmSaveStateRegister::Rax;

        assert_eq!(read_register_field(&view, reg, word, 0, &mut out), Err(Status::NOT_FOUND));
        assert_eq!(read_register_field(&view, reg, byte, 2, &mut out), Err(Status::INVALID_PARAMETER));
        assert_eq!(read_register_field(&view, reg, word, 4, &mut out), Err(Status::INVALID_PARAMETER));
        assert_eq!(read_register_field(&view, reg, dword, 8, &mut out), Err(Status::INVALID_PARAMETER));
        assert_eq!(read_register_field(&view, reg, dword, 3, &mut out), Err(Status::INVALID_PARAMETER));

        // A buffer shorter than the requested width is caught before any copy.
        assert_eq!(read_register_field(&view, reg, word, 2, &mut out[..1]), Err(Status::BUFFER_TOO_SMALL));
        assert_eq!(read_register_field(&view, reg, dword, 4, &mut out[..2]), Err(Status::BUFFER_TOO_SMALL));
    }

    #[test]
    fn test_read_lma_register() {
        let constants = save_state::vendor_constants();
        let mut map = new_save_state_map();
        write_u64(&mut map[..], constants.efer_offset as usize, IA32_EFER_LMA);
        let view = view_over(map.as_slice());

        let mut out = [0u8; 8];
        assert_eq!(read_lma_register(&view, 8, &mut out), Ok(0));
        assert_eq!(u64::from_le_bytes(out), LMA_64BIT);

        assert_eq!(read_lma_register(&view, 4, &mut out[..4]), Ok(0));
        assert_eq!(u32::from_le_bytes(out[..4].try_into().unwrap()), LMA_64BIT as u32);

        assert_eq!(read_lma_register(&view, 2, &mut out), Err(Status::INVALID_PARAMETER));
        assert_eq!(read_lma_register(&view, 8, &mut out[..4]), Err(Status::BUFFER_TOO_SMALL));
    }

    #[test]
    fn test_read_lma_register_reports_32_bit_mode() {
        let constants = save_state::vendor_constants();
        let mut map = new_save_state_map();
        write_u64(&mut map[..], constants.efer_offset as usize, 0);
        let view = view_over(map.as_slice());

        // AMD64 is always in long mode during MM, so only Intel can report 32-bit.
        let expected = if constants.lma_always_64 { LMA_64BIT } else { LMA_32BIT };
        let mut out = [0u8; 8];
        assert_eq!(read_lma_register(&view, 8, &mut out), Ok(0));
        assert_eq!(u64::from_le_bytes(out), expected);
    }

    #[test]
    fn test_read_io_register_rejects_a_save_state_without_an_io_trap() {
        let map = new_save_state_map();
        let view = view_over(map.as_slice());
        let mut out = [0u8; IO_INFO_SIZE];

        // A zeroed I/O field means the SMI was not caused by an I/O instruction.
        assert_eq!(read_io_register(&view, &mut out), Err(Status::NOT_FOUND));
    }

    #[test]
    fn test_read_io_register_reports_wider_transfers() {
        const IO_PORT: u16 = 0x70;

        let constants = save_state::vendor_constants();
        let mut out = [0u8; IO_INFO_SIZE];

        for (byte_count, rax, expected_data) in [(2usize, 0xAABB_CCDDu32, 0xCCDDu64), (4, 0xAABB_CCDD, 0xAABB_CCDD)] {
            let mut map = new_save_state_map();
            let io_field = io_trap_field(IO_PORT, true, byte_count);
            write_u32(&mut map[..], constants.io_info_offset as usize, io_field);
            write_u32(&mut map[..], constants.rax_offset as usize, rax);

            assert_eq!(read_io_register(&view_over(map.as_slice()), &mut out), Ok(0));

            let parsed = save_state::parse_io_field(io_field).unwrap();
            let expected = MmSaveStateIoInfo {
                io_data: expected_data,
                io_port: IO_PORT,
                _pad0: [0; 2],
                io_width: parsed.io_width,
                io_type: parsed.io_type,
                _pad1: [0; 4],
            };
            assert_eq!(out, expected.as_bytes());
        }
    }

    #[test]
    fn test_read_io_register_rejects_a_short_buffer() {
        let constants = save_state::vendor_constants();
        let mut map = new_save_state_map();
        write_u32(&mut map[..], constants.io_info_offset as usize, io_trap_field(0x70, true, 1));

        let mut out = [0u8; IO_INFO_SIZE];
        assert_eq!(
            read_io_register(&view_over(map.as_slice()), &mut out[..IO_INFO_SIZE - 1]),
            Err(Status::BUFFER_TOO_SMALL)
        );
    }

    #[test]
    #[should_panic(expected = "does not expose I/O info")]
    fn test_read_io_register_panics_on_legacy_save_state_revision() {
        let constants = save_state::vendor_constants();
        // AMD always reports I/O info, so only Intel can observe an unsupported revision.
        assert!(!constants.lma_always_64, "vendor always exposes I/O info");

        let mut map = new_save_state_map();
        write_u32(&mut map[..], constants.smmrevid_offset as usize, constants.min_rev_id_io - 1);
        let mut out = [0u8; IO_INFO_SIZE];

        let _ = read_io_register(&view_over(map.as_slice()), &mut out);
    }

    #[test]
    fn test_inspect_io_condition_maps_direction_to_policy_condition() {
        let constants = save_state::vendor_constants();
        let mut map = new_save_state_map();

        write_u32(&mut map[..], constants.io_info_offset as usize, io_trap_field(0xB2, true, 1));
        assert_eq!(inspect_io_condition(&view_over(map.as_slice())), Some(SaveStateCondition::IoRead));

        write_u32(&mut map[..], constants.io_info_offset as usize, io_trap_field(0xB2, false, 1));
        assert_eq!(inspect_io_condition(&view_over(map.as_slice())), Some(SaveStateCondition::IoWrite));

        write_u32(&mut map[..], constants.io_info_offset as usize, 0);
        assert_eq!(inspect_io_condition(&view_over(map.as_slice())), None);
    }

    #[test]
    #[should_panic(expected = "does not expose I/O info")]
    fn test_inspect_io_condition_panics_on_legacy_save_state_revision() {
        let constants = save_state::vendor_constants();
        // AMD always reports I/O info, so only Intel can observe an unsupported revision.
        assert!(!constants.lma_always_64, "vendor always exposes I/O info");

        let mut map = new_save_state_map();
        write_u32(&mut map[..], constants.smmrevid_offset as usize, constants.min_rev_id_io - 1);

        let _ = inspect_io_condition(&view_over(map.as_slice()));
    }

    #[test]
    fn test_read_gated_register_allows_a_permitted_register() {
        let rbx = save_state::register_info(MmSaveStateRegister::Rbx).expect("RBX is mapped on both vendors");
        let mut map = new_save_state_map();
        write_u32(&mut map[..], rbx.lo_offset as usize, 0x0000_0042);
        write_u32(&mut map[..], rbx.hi_offset as usize, 0);

        // RBX is not policy-gated, so an empty allow-list still permits the read.
        let policy = build_policy(ACCESS_ATTR_ALLOW, &[]);
        let gate = gate_over(&policy);

        let mut out = [0u8; 8];
        assert_eq!(
            read_gated_register(&view_over(map.as_slice()), &gate, MmSaveStateRegister::Rbx, 8, &mut out),
            Ok(0)
        );
        assert_eq!(u64::from_le_bytes(out), 0x42);
    }

    #[test]
    fn test_read_gated_register_denies_rax_without_a_matching_descriptor() {
        let map = new_save_state_map();
        let policy = build_policy(ACCESS_ATTR_ALLOW, &[]);
        let gate = gate_over(&policy);

        let mut out = [0u8; 8];
        assert_eq!(
            read_gated_register(&view_over(map.as_slice()), &gate, MmSaveStateRegister::Rax, 8, &mut out),
            Err(Status::ACCESS_DENIED)
        );
    }

    #[test]
    fn test_read_gated_register_allows_rax_with_an_unconditional_descriptor() {
        let rax = save_state::register_info(MmSaveStateRegister::Rax).expect("RAX is mapped on both vendors");
        let mut map = new_save_state_map();
        write_u32(&mut map[..], rax.lo_offset as usize, 0x9999_0001);
        write_u32(&mut map[..], rax.hi_offset as usize, 0);

        let policy = build_policy(
            ACCESS_ATTR_ALLOW,
            &[descriptor(SaveStateField::Rax, RESOURCE_ATTR_READ, SaveStateCondition::Unconditional)],
        );
        let gate = gate_over(&policy);

        let mut out = [0u8; 8];
        assert_eq!(
            read_gated_register(&view_over(map.as_slice()), &gate, MmSaveStateRegister::Rax, 8, &mut out),
            Ok(0)
        );
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
            read_gated_register(&view_over(map.as_slice()), &gate, MmSaveStateRegister::Io, 4, &mut out),
            Err(Status::NOT_FOUND)
        );
    }

    #[test]
    fn test_read_gated_register_matches_the_io_trap_condition() {
        const IO_PORT: u16 = 0xB2;
        const IO_DATA: u8 = 0x5A;

        let constants = save_state::vendor_constants();
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
        assert_eq!(read_gated_register(&view_over(map.as_slice()), &gate, MmSaveStateRegister::Io, 4, &mut out), Ok(0));

        let parsed = save_state::parse_io_field(io_field).unwrap();
        let expected = MmSaveStateIoInfo {
            io_data: u64::from(IO_DATA),
            io_port: IO_PORT,
            _pad0: [0; 2],
            io_width: parsed.io_width,
            io_type: parsed.io_type,
            _pad1: [0; 4],
        };
        assert_eq!(out, expected.as_bytes());
    }

    #[test]
    fn test_read_gated_register_denies_an_io_read_with_the_wrong_condition() {
        let constants = save_state::vendor_constants();
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
            read_gated_register(&view_over(map.as_slice()), &gate, MmSaveStateRegister::Io, 4, &mut out),
            Err(Status::ACCESS_DENIED)
        );
    }

    #[test]
    fn test_read_gated_register_reads_lma_without_policy_gating() {
        let constants = save_state::vendor_constants();
        let mut map = new_save_state_map();
        write_u64(&mut map[..], constants.efer_offset as usize, IA32_EFER_LMA);

        let policy = build_policy(ACCESS_ATTR_ALLOW, &[]);
        let gate = gate_over(&policy);

        let mut out = [0u8; 8];
        assert_eq!(
            read_gated_register(&view_over(map.as_slice()), &gate, MmSaveStateRegister::Lma, 8, &mut out),
            Ok(0)
        );
        assert_eq!(u64::from_le_bytes(out), LMA_64BIT);
    }
}
