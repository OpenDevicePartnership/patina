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

use crate::mm_policy::{SaveStateCondition, SaveStateField};
use patina_internal_cpu::save_state::{
    self, IA32_EFER_LMA, IO_INFO_SIZE, IO_TYPE_INPUT, IO_TYPE_OUTPUT, LMA_32BIT, LMA_64BIT, MmSaveStateIoInfo,
    MmSaveStateRegister, PROCESSOR_INFO_ENTRY_SIZE,
};
use r_efi::efi::Status;

use crate::{PageOwnership, privilege_mgmt::SyscallResult, query_address_ownership, state::security_state};

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
/// `SMRAM_SAVE_STATE_MAP_OFFSET` in MdePkg). The per-CPU save-state region base
/// is `sm_base[i] + SMRAM_SAVE_STATE_MAP_OFFSET`, derived here so the loader only
/// has to pass the raw SMBASE array.
const SMRAM_SAVE_STATE_MAP_OFFSET: u64 = 0xfc00;

/// Per-CPU save-state metadata needed by the save-state read syscall.
///
/// Assembled at initialization from two public sources instead of the private
/// `SMM_CPU_PRIVATE_DATA` layout:
///
/// - `number_of_cpus` and `processor_info` come from the MP Information HOB
///   (`gMpInformationHobGuid`).
/// - `sm_base` (the per-CPU SMBASE array) is passed through the MM Supervisor
///   PassDown HOB. The save-state region base is derived as
///   `sm_base[i] + SMRAM_SAVE_STATE_MAP_OFFSET` with the fixed
///   [`SMRAM_SAVE_STATE_MAP_SIZE`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct SaveStateInfo {
    /// Number of CPUs (from `MP_INFORMATION_HOB_DATA.NumberOfProcessors`).
    pub(crate) number_of_cpus: u64,
    /// Pointer to the `EFI_PROCESSOR_INFORMATION[]` array (from the MP Information HOB).
    pub(crate) processor_info: u64,
    /// Pointer to the per-CPU SMBASE array (`u64[number_of_cpus]`).
    pub(crate) sm_base: u64,
}

/// Holds the parameters from Phase 1 until Phase 2 completes the read.
pub(crate) struct SaveStateAccessHolder {
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
    // Validate register
    let register = match MmSaveStateRegister::from_u64(register_raw) {
        Some(r) => r,
        None => {
            log::error!("SAVE_STATE_READ: Unknown register value: {}", register_raw);
            return Err(Status::INVALID_PARAMETER);
        }
    };

    // Validate CPU index against NumberOfCpus
    let num_cpus = match get_number_of_cpus() {
        Ok(n) => n,
        Err(status) => {
            log::error!("SAVE_STATE_READ: Unable to get number of CPUs: {:?}", status);
            return Err(status);
        }
    };

    if cpu_index >= num_cpus {
        log::error!("SAVE_STATE_READ: CPU index {} >= NumberOfCpus {}", cpu_index, num_cpus);
        return Err(Status::INVALID_PARAMETER);
    }

    // Store for Phase 2
    let mut access = security_state().lock_save_state_access();
    *access = Some(SaveStateAccessHolder { user_protocol: protocol, register, cpu_index });

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
        match access.take() {
            Some(h) => h,
            None => {
                log::error!("SAVE_STATE_READ2: Phase 1 not completed");
                return Err(Status::INVALID_PARAMETER);
            }
        }
    };

    // Verify protocol matches Phase 1
    if holder.user_protocol != protocol {
        log::error!("SAVE_STATE_READ2: Protocol mismatch: expected 0x{:x}, got 0x{:x}", holder.user_protocol, protocol);
        return Err(Status::INVALID_PARAMETER);
    }

    // Validate width and buffer
    if width == 0 || buffer == 0 {
        log::error!("SAVE_STATE_READ2: Invalid width ({}) or null buffer", width);
        return Err(Status::INVALID_PARAMETER);
    }

    let register = holder.register;
    let cpu_index = holder.cpu_index;

    // Determine the actual number of bytes we'll write
    let write_size = actual_write_size(register, width);
    if write_size == 0 {
        log::error!("SAVE_STATE_READ2: Unsupported width {} for register {:?}", width, register);
        return Err(Status::UNSUPPORTED);
    }

    // Validate buffer is in user-owned memory
    match query_address_ownership(buffer, write_size as u64) {
        Some(PageOwnership::User) => {}
        Some(owner) => {
            log::error!("SAVE_STATE_READ2: Buffer 0x{:x} owned by {:?}, expected User", buffer, owner);
            return Err(Status::ACCESS_DENIED);
        }
        None => {
            log::error!("SAVE_STATE_READ2: Buffer 0x{:x} not in mapped memory", buffer);
            return Err(Status::ACCESS_DENIED);
        }
    }

    // Create a single mutable slice over the validated user output buffer so the
    // individual read handlers write through safe slice operations.
    //
    // SAFETY: `buffer` was validated above as a user-owned, writable region of
    // at least `write_size` bytes.  User code is not executing concurrently
    // while the supervisor services this syscall, so there is no aliasing.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer as *mut u8, write_size) };

    // Special case: PROCESSOR_ID — always allowed, no policy check
    if register == MmSaveStateRegister::ProcessorId {
        return read_processor_id(cpu_index, out);
    }

    // Build a safe view over this CPU's save state region.
    let view = match get_save_state_view(cpu_index) {
        Ok(v) => v,
        Err(status) => {
            log::error!("SAVE_STATE_READ2: Unable to get save state view for CPU {}: {:?}", cpu_index, status);
            return Err(status);
        }
    };

    let policy_checks = policy_checks_for_register(register);
    let condition = if !policy_checks.is_empty() { inspect_io_condition(&view) } else { None };

    // An IO read needs the trap condition; if it can't be determined the CPU did
    // not trap an I/O instruction, which is NOT_FOUND rather than a policy denial.
    if register == MmSaveStateRegister::Io && condition.is_none() {
        log::trace!("SAVE_STATE_READ2: Unable to determine I/O condition from save state");
        return Err(Status::NOT_FOUND);
    }

    let gate = match security_state().policy_gate() {
        Some(g) => g,
        None => {
            log::error!("SAVE_STATE_READ2: Policy gate not initialized");
            return Err(Status::NOT_READY);
        }
    };

    // Each required field must independently clear the policy under the same trap
    // condition.
    for &field in policy_checks {
        if let Err(e) = gate.is_save_state_read_allowed(field, width as usize, condition) {
            log::error!("SAVE_STATE_READ2: Policy denied read of {:?} (field {:?}): {:?}", register, field, e);
            return Err(Status::ACCESS_DENIED);
        }
    }

    // Dispatch to the appropriate read handler.  Each handler reads from the
    // save state `view` and writes into the validated `out` buffer using only
    // safe slice operations.
    match register {
        MmSaveStateRegister::Io => read_io_register(&view, out),
        MmSaveStateRegister::Lma => read_lma_register(&view, width, out),
        _ => read_architectural_register(&view, register, width, out),
    }
}

/// Returns the per-CPU save-state metadata captured at initialization.
fn save_state_info() -> Result<SaveStateInfo, Status> {
    match security_state().save_state_info() {
        Some(info) => Ok(info),
        None => {
            log::error!("Save-state metadata not initialized");
            Err(Status::NOT_READY)
        }
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
/// passed through the MM Supervisor PassDown HOB. The region length is the fixed
/// [`SMRAM_SAVE_STATE_MAP_SIZE`].
fn get_save_state_view(cpu_index: u64) -> Result<SaveStateView, Status> {
    let info = save_state_info()?;

    let num_cpus = info.number_of_cpus;
    if cpu_index >= num_cpus {
        log::error!("Save state read: CPU index {} >= NumberOfCpus {}", cpu_index, num_cpus);
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
    // SAFETY: `sm_base` references a valid array of at least `num_cpus` `u64`
    // entries in SMRAM (from the PassDown HOB), so the slice covers only valid,
    // initialized memory.
    let sm_bases = unsafe { core::slice::from_raw_parts(info.sm_base as *const u64, num_cpus as usize) };

    let smbase = *sm_bases.get(cpu_index as usize).ok_or(Status::INVALID_PARAMETER)?;
    if smbase == 0 {
        log::error!("SmBase[{}] is null", cpu_index);
        return Err(Status::INVALID_PARAMETER);
    }
    let base = smbase + SMRAM_SAVE_STATE_MAP_OFFSET;

    // SAFETY: `base` points to a valid save state region of
    // `SMRAM_SAVE_STATE_MAP_SIZE` bytes in SMRAM that is stable while this SMI
    // is serviced and lives for the program's duration.
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

/// Reads the PROCESSOR_ID for a given CPU and writes it to the user buffer.
///
/// The ProcessorId (APIC ID) is read from the `EFI_PROCESSOR_INFORMATION` array
/// carried by the MP Information HOB (`gMpInformationHobGuid`).
fn read_processor_id(cpu_index: u64, out: &mut [u8]) -> SyscallResult {
    let info = save_state_info()?;

    if info.processor_info == 0 {
        log::error!("PROCESSOR_ID: ProcessorInfo array is null");
        return Err(Status::NOT_READY);
    }

    let num_cpus = info.number_of_cpus;

    // View the processor information array as bytes so the per-CPU entry can be
    // read through safe slice operations.
    //
    // SAFETY: `processor_info` points to a valid array of `num_cpus`
    // `EFI_PROCESSOR_INFORMATION` entries (PROCESSOR_INFO_ENTRY_SIZE bytes
    // each) in firmware memory, and `cpu_index` is < `num_cpus`.
    let entries = unsafe {
        core::slice::from_raw_parts(info.processor_info as *const u8, num_cpus as usize * PROCESSOR_INFO_ENTRY_SIZE)
    };

    // ProcessorId is the first field (u64) of EFI_PROCESSOR_INFORMATION.
    let offset = cpu_index as usize * PROCESSOR_INFO_ENTRY_SIZE;
    let processor_id = entries
        .get(offset..offset + 8)
        .and_then(|b| b.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(Status::INVALID_PARAMETER)?;

    // Write the 8-byte ProcessorId to the user buffer.
    out.get_mut(..8).ok_or(Status::BUFFER_TOO_SMALL)?.copy_from_slice(&processor_id.to_le_bytes());

    log::debug!("PROCESSOR_ID: CPU {} = 0x{:x}", cpu_index, processor_id);
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
    if !save_state::io_info_supported(smm_rev_id) {
        panic!("SMMRevId {:#x} does not expose I/O info; legacy hardware is not supported", smm_rev_id);
    }

    // Read the vendor-specific IO information field.
    let io_field = view.read_u32(vc.io_info_offset as usize);
    // Intentionally commented out to avoid info leakage.
    // log::info!("Inspecting IO condition: IO field = 0x{:x}", io_field);

    // Use the SDK's vendor-specific parser.
    let parsed = save_state::parse_io_field(io_field)?;
    match parsed.io_type {
        IO_TYPE_INPUT => Some(SaveStateCondition::IoRead),
        IO_TYPE_OUTPUT => Some(SaveStateCondition::IoWrite),
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
    let info = match save_state::register_info(register) {
        Some(i) => i,
        None => {
            log::error!("Register {:?} not found in save state map", register);
            return Err(Status::NOT_FOUND);
        }
    };

    let lo = info.lo_offset as usize;
    if width == 0 {
        log::error!("Register {:?} does not support 0-byte read", register);
        return Err(Status::NOT_FOUND);
    } else if width == 2 {
        if info.native_width < 2 {
            log::error!("Register {:?} does not support 2-byte read", register);
            return Err(Status::INVALID_PARAMETER);
        }
        // Read the low 2 bytes (AMD segment selectors, DT limits).
        out.get_mut(..2).ok_or(Status::BUFFER_TOO_SMALL)?.copy_from_slice(&view.read_u16(lo).to_le_bytes());
    } else if width == 4 {
        if info.native_width < 4 {
            log::error!("Register {:?} does not support 4-byte read", register);
            return Err(Status::INVALID_PARAMETER);
        }
        // Read the low 4 bytes.
        out.get_mut(..4).ok_or(Status::BUFFER_TOO_SMALL)?.copy_from_slice(&view.read_u32(lo).to_le_bytes());
    } else if width == 8 {
        if info.native_width != 8 {
            log::error!("Register {:?} does not support 8-byte read", register);
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
        log::error!("Register {:?} does not support {}-byte read", register, width);
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
    if !save_state::io_info_supported(smm_rev_id) {
        panic!("SMMRevId {:#x} does not expose I/O info; legacy hardware is not supported", smm_rev_id);
    }

    // 2. Read the vendor-specific IO information field and parse it.
    let io_field = view.read_u32(vc.io_info_offset as usize);
    let parsed = match save_state::parse_io_field(io_field) {
        Some(p) => p,
        None => {
            log::error!("IO_READ: IO field 0x{:x} did not indicate a valid I/O trap", io_field);
            return Err(Status::NOT_FOUND);
        }
    };

    // 3. Read I/O data from RAX (only the significant bytes).
    let rax = vc.rax_offset as usize;
    let io_data: u64 = match parsed.byte_count {
        1 => view.read_u8(rax) as u64,
        2 => view.read_u16(rax) as u64,
        4 => view.read_u32(rax) as u64,
        _ => {
            log::error!("IO_READ: Unsupported byte count: {}", parsed.byte_count);
            0
        }
    };

    // 4. Serialize the EFI_MM_SAVE_STATE_IO_INFO structure into the output
    //    buffer by writing the whole #[repr(C)] struct at once.
    let io_info =
        MmSaveStateIoInfo { io_data, io_port: parsed.io_port, io_width: parsed.io_width, io_type: parsed.io_type };

    // SAFETY: `out` was validated as a user owned, writable region of at least
    // `IO_INFO_SIZE` bytes, which equals `size_of::<MmSaveStateIoInfo>()`.
    // `write_unaligned` accounts for the buffer's unknown alignment.
    unsafe {
        core::ptr::write_unaligned(out.as_mut_ptr() as *mut MmSaveStateIoInfo, io_info);
    }

    Ok(0)
}

/// Reads the LMA pseudo-register (processor Long Mode Active state) into `out`.
///
/// Returns `LMA_32BIT` (32) or `LMA_64BIT` (64) depending on the IA32_EFER.LMA
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
mod tests {
    use super::*;

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
    fn test_save_state_access_holder() {
        // Test that the mutex works correctly for Phase 1/Phase 2.
        {
            let mut access = crate::state::security_state().lock_save_state_access();
            assert!(access.is_none());
            *access =
                Some(SaveStateAccessHolder { user_protocol: 0xDEAD, register: MmSaveStateRegister::Rax, cpu_index: 0 });
        }

        {
            let mut access = crate::state::security_state().lock_save_state_access();
            let holder = access.take().unwrap();
            assert_eq!(holder.user_protocol, 0xDEAD);
            assert_eq!(holder.register, MmSaveStateRegister::Rax);
            assert_eq!(holder.cpu_index, 0);
        }

        {
            let access = crate::state::security_state().lock_save_state_access();
            assert!(access.is_none());
        }
    }
}
