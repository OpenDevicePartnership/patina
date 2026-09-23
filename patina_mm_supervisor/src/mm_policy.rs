//! MM Supervisor Secure Policy
//!
//! This module provides a comprehensive policy management library for the MM Supervisor,
//! including policy data structures, access validation (policy gate), and helper utilities.
//!
//! ## Features
//!
//! ### Policy Gate
//! Initialize with a policy buffer pointer, then query whether operations are allowed:
//! - `is_io_allowed()` - Check I/O port access
//! - `is_msr_allowed()` - Check MSR access
//! - `is_instruction_allowed()` - Check privileged instruction execution
//! - `is_save_state_read_allowed()` - Check save state read access
//!
//! ### Helper Functions
//! - `dump_policy()` - Print policy contents for debugging
//! - `compare_policies()` - Compare two policies (order-independent)
//! - `populate_memory_policy_from_page_table()` - Walk page tables to generate memory policy
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

pub(crate) mod gate;
pub(crate) mod helpers;

pub(crate) use gate::{PolicyError, PolicyGate};
pub(crate) use helpers::{dump_policy, walk_page_table};

use core::slice;

/// Memory policy descriptor type.
pub const TYPE_MEM: u32 = 1;
/// I/O policy descriptor type.
pub const TYPE_IO: u32 = 2;
/// MSR policy descriptor type.
pub const TYPE_MSR: u32 = 3;
/// Instruction policy descriptor type.
pub const TYPE_INSTRUCTION: u32 = 4;
/// Save state policy descriptor type.
pub const TYPE_SAVE_STATE: u32 = 5;

/// Access attribute: Allow access to resources described by this policy root.
pub const ACCESS_ATTR_ALLOW: u8 = 0;
/// Access attribute: Deny access to resources described by this policy root.
pub const ACCESS_ATTR_DENY: u8 = 1;

/// Resource attribute: Read access.
pub const RESOURCE_ATTR_READ: u32 = 0x01;
/// Resource attribute: Write access.
pub const RESOURCE_ATTR_WRITE: u32 = 0x02;
/// Resource attribute: Execute access.
pub const RESOURCE_ATTR_EXECUTE: u32 = 0x04;
/// Resource attribute: Strict width (for I/O - must match exact width).
pub const RESOURCE_ATTR_STRICT_WIDTH: u32 = 0x08;
/// Resource attribute: Conditional read access.
pub const RESOURCE_ATTR_COND_READ: u32 = 0x10;
/// Resource attribute: Conditional write access.
pub const RESOURCE_ATTR_COND_WRITE: u32 = 0x20;

/// Byte size of one descriptor belonging to `policy_type`, or `None` if the type is not one the
/// supervisor recognizes.
pub fn descriptor_size(policy_type: u32) -> Option<usize> {
    match policy_type {
        TYPE_MEM => Some(core::mem::size_of::<MemDescriptorV1_0>()),
        TYPE_IO => Some(core::mem::size_of::<IoDescriptorV1_0>()),
        TYPE_MSR => Some(core::mem::size_of::<MsrDescriptorV1_0>()),
        TYPE_INSTRUCTION => Some(core::mem::size_of::<InstructionDescriptorV1_0>()),
        TYPE_SAVE_STATE => Some(core::mem::size_of::<SaveStateDescriptorV1_0>()),
        _ => None,
    }
}

/// MSRs whose write access defines the privilege boundary the supervisor rests on, as inclusive
/// `(first, last, description)` ranges.
///
/// The platform policy decides what Ring 3 may touch; this list decides nothing. It exists so the
/// supervisor can say, once, whether the policy it was handed still leaves a boundary to enforce.
/// Granting Ring 3 a write here does not weaken isolation, it removes it: `LSTAR` repoints the
/// syscall entry at attacker-chosen code that the CPU then runs in Ring 0, `KERNEL_GS_BASE`
/// chooses the Ring 0 stack the entry stub switches to, `IA32_PL0_SSP` relocates the supervisor
/// shadow stack, `SMRR_PHYS*` unlock MMRAM, clearing `EFER.NXE` disables NX enforcement, the MTRR
/// and `PAT` registers enable cache-poisoning attacks on MMRAM, and `IA32_DS_AREA` and
/// `IA32_RTIT_CTL` aim trace stores at an arbitrary address.
///
/// Registers a real platform has cause to write are deliberately absent even when they are
/// security-relevant - `IA32_APIC_BASE` and `IA32_FEATURE_CONTROL` among them - so that a normal
/// policy does not produce noise here.
const BOUNDARY_MSRS: &[(u32, u32, &str)] = &[
    (0x0000_009E, 0x0000_009E, "IA32_SMBASE"),
    (0x0000_0174, 0x0000_0176, "IA32_SYSENTER_CS/ESP/EIP"),
    (0x0000_01D9, 0x0000_01D9, "IA32_DEBUGCTL"),
    (0x0000_01F2, 0x0000_01F3, "SMRR_PHYSBASE/SMRR_PHYSMASK"),
    (0x0000_0200, 0x0000_020F, "variable-range MTRRs"),
    (0x0000_0250, 0x0000_0250, "MTRR_FIX64K_00000"),
    (0x0000_0258, 0x0000_0259, "MTRR_FIX16K_80000/A0000"),
    (0x0000_0268, 0x0000_026F, "fixed-range MTRRs"),
    (0x0000_0277, 0x0000_0277, "IA32_PAT"),
    (0x0000_02FF, 0x0000_02FF, "IA32_MTRR_DEF_TYPE"),
    (0x0000_04E0, 0x0000_04E0, "MSR_SMM_FEATURE_CONTROL"),
    (0x0000_0570, 0x0000_0570, "IA32_RTIT_CTL"),
    (0x0000_0600, 0x0000_0600, "IA32_DS_AREA"),
    (0x0000_06A0, 0x0000_06A0, "IA32_U_CET"),
    (0x0000_06A2, 0x0000_06A2, "IA32_S_CET"),
    (0x0000_06A4, 0x0000_06A8, "IA32_PL0_SSP..IA32_PL3_SSP/IA32_INTERRUPT_SSP_TABLE_ADDR"),
    (0x0000_0DA0, 0x0000_0DA0, "IA32_XSS"),
    (0xC000_0080, 0xC000_0084, "IA32_EFER/STAR/LSTAR/CSTAR/FMASK"),
    (0xC000_0101, 0xC000_0102, "IA32_GS_BASE/KERNEL_GS_BASE"),
];

/// Reports every [`BOUNDARY_MSRS`] register the policy grants Ring 3 write access to, returning
/// how many it found.
///
/// This is a diagnostic, not a gate. It runs once, when the policy is installed, and asks the
/// policy its own question rather than second-guessing it, so the supervisor keeps exactly one
/// place that decides what Ring 3 may do. Reporting at initialization also puts the finding in
/// front of the platform author deterministically on every boot, instead of waiting for whichever
/// driver happens to issue the offending `WRMSR`.
///
/// A non-zero result means the platform policy has voided the Ring 0 / Ring 3 boundary and needs
/// to be corrected; the supervisor continues, because a list of registers compiled into the
/// supervisor is not a better authority on a platform's needs than its reviewed policy.
pub(crate) fn audit_boundary_msr_grants(gate: &PolicyGate) -> usize {
    let mut granted = 0;

    for &(first, last, description) in BOUNDARY_MSRS {
        for msr in first..=last {
            if gate.is_msr_allowed(msr, AccessType::Write).is_ok() {
                log::error!(
                    "MM policy grants Ring 3 write access to MSR 0x{msr:08x} ({description}). This register defines \
                     the supervisor's privilege boundary; granting it makes Ring 3 isolation unenforceable. Remove \
                     the entry from the platform MM policy."
                );
                granted += 1;
            }
        }
    }

    if granted == 0 {
        log::info!("MM policy audit: no boundary-defining MSR is writable from Ring 3");
    } else {
        log::error!("MM policy audit: {granted} boundary-defining MSR(s) are writable from Ring 3");
    }

    granted
}

/// I/O ports whose write access defines the privilege boundary the supervisor rests on, as
/// inclusive `(first, last, description)` ranges.
///
/// Same contract as [`BOUNDARY_MSRS`]: this list decides nothing, it reports. Granting Ring 3
/// write access to PCI configuration space reaches the chipset registers that lock MMRAM and
/// program the DRAM remap windows, so it can unlock MMRAM or alias normal memory over it without
/// ever issuing a `WRMSR`; granting the ACPI software MMI command port lets a demoted driver
/// forge the command value the MMI dispatchers key on, invoking handlers as though the request
/// came from outside MM.
///
/// Ports a real platform has cause to drive from MM are deliberately absent - the embedded
/// controller, CMOS, GPIO and the ACPI PM block among them - so that a normal policy does not
/// produce noise here.
const BOUNDARY_IO_PORTS: &[(u16, u16, &str)] =
    &[(0x00B2, 0x00B3, "ACPI software MMI command/data port"), (0x0CF8, 0x0CFF, "PCI configuration address/data")];

/// Reports every [`BOUNDARY_IO_PORTS`] port the policy grants Ring 3 write access to, returning
/// how many it found.
///
/// The companion to [`audit_boundary_msr_grants`], and a diagnostic on the same terms: it runs
/// once when the policy is installed, asks the policy its own question, and changes nothing.
/// Ports are probed one byte at a time, which is the granularity at which the policy describes
/// them.
pub(crate) fn audit_boundary_io_grants(gate: &PolicyGate) -> usize {
    let mut granted = 0;

    for &(first, last, description) in BOUNDARY_IO_PORTS {
        for port in first..=last {
            if gate.is_io_allowed(u32::from(port), IoWidth::Byte, AccessType::Write).is_ok() {
                log::error!(
                    "MM policy grants Ring 3 write access to I/O port 0x{port:04x} ({description}). This port \
                     reaches the configuration that defines the MM boundary; granting it makes Ring 3 isolation \
                     unenforceable. Remove the entry from the platform MM policy."
                );
                granted += 1;
            }
        }
    }

    if granted == 0 {
        log::info!("MM policy audit: no boundary-defining I/O port is writable from Ring 3");
    } else {
        log::error!("MM policy audit: {granted} boundary-defining I/O port(s) are writable from Ring 3");
    }

    granted
}

/// Privileged instruction types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Instruction {
    /// CLI - Clear Interrupt Flag
    Cli = 0,
    /// WBINVD - Write Back and Invalidate Cache
    Wbinvd = 1,
    /// HLT - Halt
    Hlt = 2,
}

impl Instruction {
    /// Total count of privileged instructions tracked.
    pub const COUNT: u16 = 3;

    /// Convert to instruction index.
    pub fn as_index(self) -> u16 {
        self as u16
    }
}

/// Save state map fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SaveStateField {
    /// RAX register
    Rax = 0,
    /// I/O trap information
    IoTrap = 1,
}

impl SaveStateField {
    /// Convert to field index.
    pub fn as_index(self) -> u32 {
        self as u32
    }
}

/// Save state access conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SaveStateCondition {
    /// Unconditional access
    Unconditional = 0,
    /// Conditional on I/O read trap
    IoRead = 1,
    /// Conditional on I/O write trap
    IoWrite = 2,
}

/// Type of access being requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    /// Read access
    Read,
    /// Write access
    Write,
    /// Execute access (for instructions)
    Execute,
}

impl AccessType {
    /// Convert to resource attribute mask.
    pub fn as_attr_mask(self) -> u32 {
        match self {
            AccessType::Read => RESOURCE_ATTR_READ,
            AccessType::Write => RESOURCE_ATTR_WRITE,
            AccessType::Execute => RESOURCE_ATTR_EXECUTE,
        }
    }
}

/// I/O access width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum IoWidth {
    /// 8-bit (1 byte) access
    Byte = 1,
    /// 16-bit (2 byte) access
    Word = 2,
    /// 32-bit (4 byte) access
    Dword = 4,
}

impl IoWidth {
    /// Get the size in bytes.
    pub fn size(self) -> u32 {
        self as u32
    }
}

/// Memory policy descriptor (V1.0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemDescriptorV1_0 {
    /// Base address of memory region.
    pub base_address: u64,
    /// Size of memory region in bytes.
    pub size: u64,
    /// Memory attributes (combination of `RESOURCE_ATTR_*`).
    pub mem_attributes: u32,
    /// Reserved, must be 0.
    pub reserved: u32,
}

/// I/O policy descriptor (V1.0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IoDescriptorV1_0 {
    /// Base I/O port address.
    pub io_address: u16,
    /// Length or width of the I/O range.
    pub length_or_width: u16,
    /// I/O attributes (combination of `RESOURCE_ATTR_*`).
    pub attributes: u16,
    /// Reserved, must be 0.
    pub reserved: u16,
}

/// MSR policy descriptor (V1.0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MsrDescriptorV1_0 {
    /// Base MSR address.
    pub msr_address: u32,
    /// Length of MSR range.
    pub length: u16,
    /// MSR attributes (combination of `RESOURCE_ATTR_*`).
    pub attributes: u16,
}

/// Instruction policy descriptor (V1.0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstructionDescriptorV1_0 {
    /// Instruction index (one of `INSTRUCTION_*` constants).
    pub instruction_index: u16,
    /// Instruction attributes (combination of `RESOURCE_ATTR_*`).
    pub attributes: u16,
    /// Reserved, must be 0.
    pub reserved: u32,
}

/// Save state policy descriptor (V1.0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaveStateDescriptorV1_0 {
    /// Save state map field (one of `SVST_*` constants).
    pub map_field: u32,
    /// Save state attributes (combination of `RESOURCE_ATTR_*`).
    pub attributes: u32,
    /// Access condition (one of `SVST_CONDITION_*` constants).
    pub access_condition: u32,
    /// Reserved, must be 0.
    pub reserved: u32,
}

/// Policy root structure (V1).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PolicyRootV1 {
    /// Version of this policy root structure.
    pub version: u32,
    /// Size of this policy root structure in bytes.
    pub policy_root_size: u32,
    /// Type of descriptors (one of `TYPE_*` constants).
    pub policy_type: u32,
    /// Offset in bytes from policy data start to the descriptors.
    pub offset: u32,
    /// Number of descriptor entries.
    pub count: u32,
    /// Access attribute (one of `ACCESS_ATTR_*` constants).
    pub access_attr: u8,
    /// Reserved, must be all zeros.
    pub reserved: [u8; 3],
}

/// Secure policy data header (V1.0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SecurePolicyDataV1_0 {
    /// Minor version (should be 0x0000).
    pub version_minor: u16,
    /// Major version (should be 0x0001).
    pub version_major: u16,
    /// Total size in bytes of the entire policy block.
    pub size: u32,
    /// Offset to legacy memory policy (0 if not supported).
    pub memory_policy_offset: u32,
    /// Count of legacy memory policy entries (0 if not supported).
    pub memory_policy_count: u32,
    /// Flag field indicating supervisor status.
    pub flags: u32,
    /// Capability field indicating features supported by supervisor.
    pub capabilities: u32,
    /// Reserved, must be 0.
    pub reserved: u64,
    /// Offset from this structure to the policy root array.
    pub policy_root_offset: u32,
    /// Number of policy roots.
    pub policy_root_count: u32,
}

impl SecurePolicyDataV1_0 {
    /// Returns true if this is a valid V1.0 policy header.
    pub fn is_valid_version(&self) -> bool {
        self.version_major == 1 && self.version_minor == 0
    }

    /// Gets a pointer to the policy root array.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that this structure is part of a valid policy buffer.
    pub unsafe fn get_policy_roots_ptr(&self) -> *const PolicyRootV1 {
        let base = core::ptr::from_ref::<Self>(self).cast::<u8>();
        // SAFETY: per this function's contract `self` is part of a valid policy buffer, so
        // `policy_root_offset` stays within that buffer. This only computes a pointer (no deref).
        unsafe { base.add(self.policy_root_offset as usize) as *const PolicyRootV1 }
    }

    /// Gets a slice of policy roots.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that this structure is part of a valid policy buffer.
    pub unsafe fn get_policy_roots(&self) -> &[PolicyRootV1] {
        // SAFETY: per this function's contract `self` is part of a valid policy buffer, so the
        // roots pointer and `policy_root_count` describe an in-bounds, initialized array.
        unsafe { slice::from_raw_parts(self.get_policy_roots_ptr(), self.policy_root_count as usize) }
    }
}

impl PolicyRootV1 {
    /// Returns true if the reserved fields are all zeros.
    pub fn has_valid_reserved(&self) -> bool {
        self.reserved == [0, 0, 0]
    }

    /// Gets a pointer to the descriptors for this policy root.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `policy_base` points to a valid policy buffer.
    pub unsafe fn get_descriptors_ptr<T>(&self, policy_base: *const u8) -> *const T {
        // SAFETY: per this function's contract `policy_base` points to a valid policy buffer, so
        // `offset` stays within it. This only computes a pointer (no deref).
        unsafe { policy_base.add(self.offset as usize) as *const T }
    }

    /// Gets memory descriptors from this policy root.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `policy_base` points to a valid policy buffer.
    pub unsafe fn get_mem_descriptors(&self, policy_base: *const u8) -> &[MemDescriptorV1_0] {
        // SAFETY: per this function's contract `policy_base` points to a valid policy buffer, so
        // the descriptor pointer and `count` describe an in-bounds, initialized array.
        unsafe {
            slice::from_raw_parts(self.get_descriptors_ptr::<MemDescriptorV1_0>(policy_base), self.count as usize)
        }
    }

    /// Gets I/O descriptors from this policy root.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `policy_base` points to a valid policy buffer.
    pub unsafe fn get_io_descriptors(&self, policy_base: *const u8) -> &[IoDescriptorV1_0] {
        // SAFETY: per this function's contract `policy_base` points to a valid policy buffer, so
        // the descriptor pointer and `count` describe an in-bounds, initialized array.
        unsafe { slice::from_raw_parts(self.get_descriptors_ptr::<IoDescriptorV1_0>(policy_base), self.count as usize) }
    }

    /// Gets MSR descriptors from this policy root.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `policy_base` points to a valid policy buffer.
    pub unsafe fn get_msr_descriptors(&self, policy_base: *const u8) -> &[MsrDescriptorV1_0] {
        // SAFETY: per this function's contract `policy_base` points to a valid policy buffer, so
        // the descriptor pointer and `count` describe an in-bounds, initialized array.
        unsafe {
            slice::from_raw_parts(self.get_descriptors_ptr::<MsrDescriptorV1_0>(policy_base), self.count as usize)
        }
    }

    /// Gets instruction descriptors from this policy root.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `policy_base` points to a valid policy buffer.
    pub unsafe fn get_instruction_descriptors(&self, policy_base: *const u8) -> &[InstructionDescriptorV1_0] {
        // SAFETY: per this function's contract `policy_base` points to a valid policy buffer, so
        // the descriptor pointer and `count` describe an in-bounds, initialized array.
        unsafe {
            slice::from_raw_parts(
                self.get_descriptors_ptr::<InstructionDescriptorV1_0>(policy_base),
                self.count as usize,
            )
        }
    }

    /// Gets save state descriptors from this policy root.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `policy_base` points to a valid policy buffer.
    pub unsafe fn get_save_state_descriptors(&self, policy_base: *const u8) -> &[SaveStateDescriptorV1_0] {
        // SAFETY: per this function's contract `policy_base` points to a valid policy buffer, so
        // the descriptor pointer and `count` describe an in-bounds, initialized array.
        unsafe {
            slice::from_raw_parts(self.get_descriptors_ptr::<SaveStateDescriptorV1_0>(policy_base), self.count as usize)
        }
    }
}

const _: () = {
    assert!(core::mem::size_of::<MemDescriptorV1_0>() == 24);
    assert!(core::mem::size_of::<IoDescriptorV1_0>() == 8);
    assert!(core::mem::size_of::<MsrDescriptorV1_0>() == 8);
    assert!(core::mem::size_of::<InstructionDescriptorV1_0>() == 8);
    assert!(core::mem::size_of::<SaveStateDescriptorV1_0>() == 16);
    assert!(core::mem::size_of::<PolicyRootV1>() == 24);
    assert!(core::mem::size_of::<SecurePolicyDataV1_0>() == 40);
};

/// Builders for synthetic policy buffers, shared by the policy unit tests.
#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
pub(crate) mod test_support {
    use super::*;

    /// Byte size of the `SecurePolicyDataV1_0` header.
    pub(crate) const HEADER_SIZE: usize = 40;
    /// Byte size of one `PolicyRootV1`.
    pub(crate) const ROOT_SIZE: usize = 24;

    /// The descriptor array carried by one policy root.
    pub(crate) enum Descriptors {
        Mem(Vec<MemDescriptorV1_0>),
        Io(Vec<IoDescriptorV1_0>),
        Msr(Vec<MsrDescriptorV1_0>),
        Instruction(Vec<InstructionDescriptorV1_0>),
        SaveState(Vec<SaveStateDescriptorV1_0>),
        /// An empty root carrying a policy type the supervisor does not recognize.
        Unknown(u32),
    }

    impl Descriptors {
        fn policy_type(&self) -> u32 {
            match self {
                Self::Mem(_) => TYPE_MEM,
                Self::Io(_) => TYPE_IO,
                Self::Msr(_) => TYPE_MSR,
                Self::Instruction(_) => TYPE_INSTRUCTION,
                Self::SaveState(_) => TYPE_SAVE_STATE,
                Self::Unknown(policy_type) => *policy_type,
            }
        }

        fn count(&self) -> usize {
            match self {
                Self::Mem(d) => d.len(),
                Self::Io(d) => d.len(),
                Self::Msr(d) => d.len(),
                Self::Instruction(d) => d.len(),
                Self::SaveState(d) => d.len(),
                Self::Unknown(_) => 0,
            }
        }

        fn entry_size(&self) -> usize {
            match self {
                Self::Mem(_) => size_of::<MemDescriptorV1_0>(),
                Self::Io(_) | Self::Msr(_) | Self::Instruction(_) => 8,
                Self::SaveState(_) => size_of::<SaveStateDescriptorV1_0>(),
                Self::Unknown(_) => 0,
            }
        }

        fn encode(&self, bytes: &mut [u8], at: usize) {
            match self {
                Self::Mem(descriptors) => {
                    for (i, d) in descriptors.iter().enumerate() {
                        let at = at + i * 24;
                        write_u64(bytes, at, d.base_address);
                        write_u64(bytes, at + 8, d.size);
                        write_u32(bytes, at + 16, d.mem_attributes);
                        write_u32(bytes, at + 20, d.reserved);
                    }
                }
                Self::Io(descriptors) => {
                    for (i, d) in descriptors.iter().enumerate() {
                        let at = at + i * 8;
                        write_u16(bytes, at, d.io_address);
                        write_u16(bytes, at + 2, d.length_or_width);
                        write_u16(bytes, at + 4, d.attributes);
                        write_u16(bytes, at + 6, d.reserved);
                    }
                }
                Self::Msr(descriptors) => {
                    for (i, d) in descriptors.iter().enumerate() {
                        let at = at + i * 8;
                        write_u32(bytes, at, d.msr_address);
                        write_u16(bytes, at + 4, d.length);
                        write_u16(bytes, at + 6, d.attributes);
                    }
                }
                Self::Instruction(descriptors) => {
                    for (i, d) in descriptors.iter().enumerate() {
                        let at = at + i * 8;
                        write_u16(bytes, at, d.instruction_index);
                        write_u16(bytes, at + 2, d.attributes);
                        write_u32(bytes, at + 4, d.reserved);
                    }
                }
                Self::SaveState(descriptors) => {
                    for (i, d) in descriptors.iter().enumerate() {
                        let at = at + i * 16;
                        write_u32(bytes, at, d.map_field);
                        write_u32(bytes, at + 4, d.attributes);
                        write_u32(bytes, at + 8, d.access_condition);
                        write_u32(bytes, at + 12, d.reserved);
                    }
                }
                Self::Unknown(_) => {}
            }
        }
    }

    fn write_u16(bytes: &mut [u8], at: usize, value: u16) {
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(bytes: &mut [u8], at: usize, value: u64) {
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// Assembles a byte-exact V1.0 policy buffer from a list of policy roots.
    #[derive(Default)]
    pub(crate) struct PolicyBuilder {
        roots: Vec<(u8, Descriptors)>,
        version_major: Option<u16>,
        version_minor: Option<u16>,
        flags: u32,
        memory_policy_count: u32,
        root_reserved: [u8; 3],
        size_override: Option<u32>,
        root_count_override: Option<u32>,
        descriptor_count_override: Option<u32>,
    }

    impl PolicyBuilder {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// Appends a policy root carrying `descriptors`.
        pub(crate) fn root(mut self, access_attr: u8, descriptors: Descriptors) -> Self {
            self.roots.push((access_attr, descriptors));
            self
        }

        /// Overrides the header version, for validation tests.
        pub(crate) fn version(mut self, major: u16, minor: u16) -> Self {
            self.version_major = Some(major);
            self.version_minor = Some(minor);
            self
        }

        /// Sets the header `flags` field, which a conforming policy leaves zero.
        pub(crate) fn flags(mut self, flags: u32) -> Self {
            self.flags = flags;
            self
        }

        /// Sets the legacy `memory_policy_count` field, which a conforming policy leaves zero.
        pub(crate) fn memory_policy_count(mut self, count: u32) -> Self {
            self.memory_policy_count = count;
            self
        }

        /// Dirties every policy root's reserved bytes, for validation tests.
        pub(crate) fn root_reserved(mut self, reserved: [u8; 3]) -> Self {
            self.root_reserved = reserved;
            self
        }

        /// Overrides the header `size` field without changing the buffer contents.
        pub(crate) fn declared_size(mut self, size: u32) -> Self {
            self.size_override = Some(size);
            self
        }

        /// Overrides the header `policy_root_count` without emitting more roots.
        pub(crate) fn declared_root_count(mut self, count: u32) -> Self {
            self.root_count_override = Some(count);
            self
        }

        /// Overrides every root's descriptor `count` without emitting more descriptors.
        pub(crate) fn declared_descriptor_count(mut self, count: u32) -> Self {
            self.descriptor_count_override = Some(count);
            self
        }

        pub(crate) fn build(self) -> PolicyBuffer {
            let descriptor_offset = HEADER_SIZE + self.roots.len() * ROOT_SIZE;
            let descriptor_bytes: usize = self.roots.iter().map(|(_, d)| d.count() * d.entry_size()).sum();
            let total = descriptor_offset + descriptor_bytes;

            // `Vec<u64>` guarantees the 8-byte alignment `SecurePolicyDataV1_0` requires.
            let mut words = vec![0u64; total.div_ceil(8)];
            let bytes = unsafe {
                // SAFETY: `u64` has no padding or invalid bit patterns, so its backing store can
                // be viewed as bytes for the lifetime of this borrow.
                core::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), words.len() * 8)
            };

            write_u16(bytes, 0, self.version_minor.unwrap_or(0));
            write_u16(bytes, 2, self.version_major.unwrap_or(1));
            write_u32(bytes, 4, self.size_override.unwrap_or(total as u32));
            write_u32(bytes, 12, self.memory_policy_count);
            write_u32(bytes, 16, self.flags);
            write_u32(bytes, 32, HEADER_SIZE as u32);
            write_u32(bytes, 36, self.root_count_override.unwrap_or(self.roots.len() as u32));

            let mut descriptor_at = descriptor_offset;
            for (i, (access_attr, descriptors)) in self.roots.iter().enumerate() {
                let at = HEADER_SIZE + i * ROOT_SIZE;
                write_u32(bytes, at, 1);
                write_u32(bytes, at + 4, ROOT_SIZE as u32);
                write_u32(bytes, at + 8, descriptors.policy_type());
                write_u32(bytes, at + 12, descriptor_at as u32);
                write_u32(bytes, at + 16, self.descriptor_count_override.unwrap_or(descriptors.count() as u32));
                bytes[at + 20] = *access_attr;
                bytes[at + 21..at + 24].copy_from_slice(&self.root_reserved);

                descriptors.encode(bytes, descriptor_at);
                descriptor_at += descriptors.count() * descriptors.entry_size();
            }

            PolicyBuffer { words }
        }
    }

    /// Owns the bytes of a synthetic policy buffer.
    pub(crate) struct PolicyBuffer {
        words: Vec<u64>,
    }

    impl PolicyBuffer {
        pub(crate) fn as_ptr(&self) -> *const u8 {
            self.words.as_ptr().cast()
        }

        /// Byte length of the buffer backing this policy.
        pub(crate) fn len(&self) -> usize {
            self.words.len() * core::mem::size_of::<u64>()
        }

        pub(crate) fn header(&self) -> &SecurePolicyDataV1_0 {
            // SAFETY: `PolicyBuilder::build` wrote an aligned, fully-initialized header at offset 0.
            unsafe { &*self.as_ptr().cast::<SecurePolicyDataV1_0>() }
        }

        /// Creates a gate over this buffer, which outlives the returned gate.
        pub(crate) fn gate(&self) -> PolicyGate {
            self.try_gate().expect("valid policy buffer")
        }

        /// Creates a gate over this buffer, surfacing the validation error for malformed blobs.
        pub(crate) fn try_gate(&self) -> Result<PolicyGate, PolicyError> {
            // SAFETY: `PolicyBuilder::build` produced an aligned buffer of `len()` bytes that
            // this `PolicyBuffer` keeps alive for the gate's lifetime.
            unsafe { PolicyGate::new(self.as_ptr(), self.len()) }
        }
    }

    pub(crate) fn mem(base_address: u64, size: u64, mem_attributes: u32) -> MemDescriptorV1_0 {
        MemDescriptorV1_0 { base_address, size, mem_attributes, reserved: 0 }
    }

    pub(crate) fn io(io_address: u16, length_or_width: u16, attributes: u16) -> IoDescriptorV1_0 {
        IoDescriptorV1_0 { io_address, length_or_width, attributes, reserved: 0 }
    }

    pub(crate) fn msr(msr_address: u32, length: u16, attributes: u16) -> MsrDescriptorV1_0 {
        MsrDescriptorV1_0 { msr_address, length, attributes }
    }

    pub(crate) fn instruction(instruction: Instruction, attributes: u16) -> InstructionDescriptorV1_0 {
        InstructionDescriptorV1_0 { instruction_index: instruction.as_index(), attributes, reserved: 0 }
    }

    pub(crate) fn save_state(
        field: SaveStateField,
        attributes: u32,
        condition: SaveStateCondition,
    ) -> SaveStateDescriptorV1_0 {
        SaveStateDescriptorV1_0 {
            map_field: field.as_index(),
            attributes,
            access_condition: condition as u32,
            reserved: 0,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::test_support::{Descriptors, PolicyBuilder, instruction, io, mem, msr, save_state};
    use super::*;

    const WRITE: u16 = RESOURCE_ATTR_WRITE as u16;
    const READ_WRITE: u16 = (RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE) as u16;

    #[test]
    fn test_boundary_msr_audit_passes_a_policy_modelled_on_a_shipping_platform() {
        // The MSR grants from a real platform MM policy, including the security-relevant ones the
        // list deliberately leaves to the platform. None of these may be reported, or the audit is
        // noise that platform authors will learn to ignore.
        let granted = [
            msr(0x0000_001B, 4, READ_WRITE), // IA32_APIC_BASE
            msr(0x0000_003A, 1, READ_WRITE), // IA32_FEATURE_CONTROL
            msr(0x0000_00FE, 1, READ_WRITE), // IA32_MTRRCAP
            msr(0x0000_01A0, 1, READ_WRITE), // IA32_MISC_ENABLE
            msr(0x0000_01FE, 2, READ_WRITE), // chipset-specific, directly below the variable MTRRs
            msr(0x0000_0830, 1, READ_WRITE), // x2APIC interrupt command
        ];
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Msr(granted.to_vec())).build();

        assert_eq!(audit_boundary_msr_grants(&policy.gate()), 0);
    }

    #[test]
    fn test_boundary_msr_audit_reports_a_policy_that_grants_the_syscall_entry() {
        // An allow range that starts at IA32_EFER and runs over the syscall MSRs - the shape of
        // policy mistake the audit exists to surface.
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![msr(0xC000_0080, 5, READ_WRITE)]))
            .build();

        assert_eq!(audit_boundary_msr_grants(&policy.gate()), 5);
    }

    #[test]
    fn test_boundary_msr_audit_reports_each_granted_register_once() {
        let policy = PolicyBuilder::new()
            .root(
                ACCESS_ATTR_ALLOW,
                Descriptors::Msr(vec![
                    msr(0x0000_01F2, 2, WRITE),      // both SMRR registers
                    msr(0x0000_06A4, 1, READ_WRITE), // IA32_PL0_SSP alone
                ]),
            )
            .build();

        assert_eq!(audit_boundary_msr_grants(&policy.gate()), 3);
    }

    #[test]
    fn test_boundary_msr_audit_ignores_read_only_grants() {
        // Reads are the platform's call; only a write voids the boundary.
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![msr(0xC000_0080, 5, RESOURCE_ATTR_READ as u16)]))
            .build();

        assert_eq!(audit_boundary_msr_grants(&policy.gate()), 0);
    }

    #[test]
    fn test_boundary_msr_audit_covers_the_syscall_flag_mask() {
        // `IA32_FMASK` is what clears DF and AC when Ring 3 enters Ring 0; a policy that lets
        // Ring 3 write it hands back the flags the supervisor runs under.
        const IA32_FMASK: u32 = 0xC000_0084;

        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![msr(IA32_FMASK, 1, READ_WRITE)]))
            .build();

        assert_eq!(audit_boundary_msr_grants(&policy.gate()), 1);
    }

    #[test]
    fn test_boundary_io_audit_stays_quiet_on_ports_a_platform_legitimately_drives() {
        let granted = [
            io(0x0062, 2, READ_WRITE),    // embedded controller
            io(0x0070, 2, READ_WRITE),    // CMOS index/data
            io(0x0400, 0x40, READ_WRITE), // an ACPI PM block
            io(0x0CF9, 1, READ_WRITE),    // reset control, inside the PCI config window but not it
        ];
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Io(granted.to_vec())).build();

        // 0xCF9 sits within the audited 0xCF8-0xCFF span, so it is reported; the rest are not.
        assert_eq!(audit_boundary_io_grants(&policy.gate()), 1);
    }

    #[test]
    fn test_boundary_io_audit_reports_a_policy_that_grants_pci_config_space() {
        let policy =
            PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0x0CF8, 8, READ_WRITE)])).build();

        assert_eq!(audit_boundary_io_grants(&policy.gate()), 8);
    }

    #[test]
    fn test_boundary_io_audit_ignores_read_only_grants() {
        // Reading PCI config space does not move the boundary; writing it does.
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0x0CF8, 8, RESOURCE_ATTR_READ as u16)]))
            .build();

        assert_eq!(audit_boundary_io_grants(&policy.gate()), 0);
    }

    #[test]
    fn test_policy_enum_conversions() {
        assert_eq!(Instruction::Cli.as_index(), 0);
        assert_eq!(Instruction::Wbinvd.as_index(), 1);
        assert_eq!(Instruction::Hlt.as_index(), 2);
        assert_eq!(Instruction::COUNT, 3);

        assert_eq!(SaveStateField::Rax.as_index(), 0);
        assert_eq!(SaveStateField::IoTrap.as_index(), 1);

        assert_eq!(IoWidth::Byte.size(), 1);
        assert_eq!(IoWidth::Word.size(), 2);
        assert_eq!(IoWidth::Dword.size(), 4);

        assert_eq!(AccessType::Read.as_attr_mask(), RESOURCE_ATTR_READ);
        assert_eq!(AccessType::Write.as_attr_mask(), RESOURCE_ATTR_WRITE);
        assert_eq!(AccessType::Execute.as_attr_mask(), RESOURCE_ATTR_EXECUTE);
    }

    #[test]
    fn test_header_accepts_only_version_1_0() {
        assert!(PolicyBuilder::new().build().header().is_valid_version());
        assert!(!PolicyBuilder::new().version(2, 0).build().header().is_valid_version());
        assert!(!PolicyBuilder::new().version(1, 1).build().header().is_valid_version());
    }

    #[test]
    fn test_policy_root_reserved_validation() {
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![])).build();
        // SAFETY: the builder produced a valid policy buffer that outlives this borrow.
        let roots = unsafe { policy.header().get_policy_roots() };
        assert!(roots[0].has_valid_reserved());

        let dirty =
            PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![])).root_reserved([0, 1, 0]).build();
        // SAFETY: as above.
        let roots = unsafe { dirty.header().get_policy_roots() };
        assert!(!roots[0].has_valid_reserved());
    }

    #[test]
    fn test_policy_header_describes_its_roots() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0x60, 1, RESOURCE_ATTR_READ as u16)]))
            .root(ACCESS_ATTR_DENY, Descriptors::Msr(vec![msr(0x1B, 1, RESOURCE_ATTR_WRITE as u16)]))
            .build();

        let header = policy.header();
        assert_eq!(header.policy_root_count, 2);
        assert_eq!(header.policy_root_offset, 40);
        assert_eq!(header.size, 40 + 2 * 24 + 8 + 8);

        // SAFETY: the builder produced a valid policy buffer that outlives this borrow.
        let roots = unsafe { header.get_policy_roots() };
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].policy_type, TYPE_IO);
        assert_eq!(roots[0].access_attr, ACCESS_ATTR_ALLOW);
        assert_eq!(roots[1].policy_type, TYPE_MSR);
        assert_eq!(roots[1].access_attr, ACCESS_ATTR_DENY);

        // SAFETY: as above; the roots point into the same buffer.
        unsafe {
            assert_eq!(
                roots[0].get_descriptors_ptr::<IoDescriptorV1_0>(policy.as_ptr()),
                roots[0].get_io_descriptors(policy.as_ptr()).as_ptr()
            );
        }
    }

    #[test]
    fn test_policy_root_descriptor_accessors_round_trip() {
        let mem_descriptors = vec![mem(0x1000, 0x2000, RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE)];
        let io_descriptors = vec![io(0x60, 1, 0x01), io(0xCF8, 4, 0x03)];
        let msr_descriptors = vec![msr(0x1B, 1, 0x01)];
        let instruction_descriptors = vec![instruction(Instruction::Hlt, 0x04)];
        let save_state_descriptors =
            vec![save_state(SaveStateField::Rax, RESOURCE_ATTR_READ, SaveStateCondition::Unconditional)];

        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Mem(mem_descriptors.clone()))
            .root(ACCESS_ATTR_ALLOW, Descriptors::Io(io_descriptors.clone()))
            .root(ACCESS_ATTR_ALLOW, Descriptors::Msr(msr_descriptors.clone()))
            .root(ACCESS_ATTR_ALLOW, Descriptors::Instruction(instruction_descriptors.clone()))
            .root(ACCESS_ATTR_ALLOW, Descriptors::SaveState(save_state_descriptors.clone()))
            .build();

        let base = policy.as_ptr();
        // SAFETY: the builder produced a valid policy buffer that outlives these borrows, and
        // every root was written with the offset/count of its own descriptor array.
        unsafe {
            let roots = policy.header().get_policy_roots();
            assert_eq!(roots[0].get_mem_descriptors(base), mem_descriptors.as_slice());
            assert_eq!(roots[1].get_io_descriptors(base), io_descriptors.as_slice());
            assert_eq!(roots[2].get_msr_descriptors(base), msr_descriptors.as_slice());
            assert_eq!(roots[3].get_instruction_descriptors(base), instruction_descriptors.as_slice());
            assert_eq!(roots[4].get_save_state_descriptors(base), save_state_descriptors.as_slice());
        }
    }
}
