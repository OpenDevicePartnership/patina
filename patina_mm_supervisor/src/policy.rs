//! MM Supervisor secure policy.
//!
//! Defines the firmware policy structures the MM IPL passes down. The submodules:
//!
//! - [`validation`] decides once whether the supervisor accepts the blob, and stops
//!   initialization if it does not.
//! - [`gate`] decides, for every later syscall, whether it may read or write an MSR or I/O
//!   port, run a privileged instruction, read a save-state field, or access a memory range.
//! - [`boundary_audit`] reports once whether the accepted policy keeps the boundary between
//!   Ring 0 and Ring 3 enforceable. It only logs.
//! - [`helper`] prints a policy for debugging.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

pub(crate) mod boundary_audit;
pub(crate) mod gate;
pub(crate) mod helper;
pub(crate) mod validation;

pub(crate) use boundary_audit::audit_boundary_grants;
pub(crate) use gate::{PolicyGate, PolicyGateError};
pub(crate) use helper::dump_policy;
pub(crate) use validation::walk_page_table;

use core::slice;

use patina::{MemoryRange, MemoryRangeError};

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

/// Access attribute: allow access to the resources this policy root describes.
pub const ACCESS_ATTR_ALLOW: u8 = 0;
/// Access attribute: deny access to the resources this policy root describes.
pub const ACCESS_ATTR_DENY: u8 = 1;

/// Resource attribute: read access.
pub const RESOURCE_ATTR_READ: u32 = 0x01;
/// Resource attribute: write access.
pub const RESOURCE_ATTR_WRITE: u32 = 0x02;
/// Resource attribute: execute access.
pub const RESOURCE_ATTR_EXECUTE: u32 = 0x04;
/// Resource attribute: strict width (an I/O access must match the descriptor width exactly).
pub const RESOURCE_ATTR_STRICT_WIDTH: u32 = 0x08;
/// Resource attribute: conditional read access.
pub const RESOURCE_ATTR_COND_READ: u32 = 0x10;
/// Resource attribute: conditional write access.
pub const RESOURCE_ATTR_COND_WRITE: u32 = 0x20;

/// Byte size of one descriptor of `policy_type`, or `None` if the supervisor does not recognize
/// the type.
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
    /// Number of privileged instructions tracked.
    pub const COUNT: u16 = 3;

    /// Returns the instruction index.
    pub fn as_index(self) -> u16 {
        self as u16
    }
}

impl core::fmt::Display for Instruction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Cli => "CLI",
            Self::Wbinvd => "WBINVD",
            Self::Hlt => "HLT",
        })
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
    /// Returns the field index.
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
    /// Returns the matching `RESOURCE_ATTR_*` mask.
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
    /// Returns the size in bytes.
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

impl TryFrom<&MemDescriptorV1_0> for MemoryRange {
    type Error = MemoryRangeError;

    /// Returns the region `desc` describes.
    ///
    /// ## Errors
    ///
    /// Fails as [`MemoryRange::new`] does when the size is zero or the end is past `u64::MAX`.
    fn try_from(desc: &MemDescriptorV1_0) -> Result<Self, Self::Error> {
        MemoryRange::new(desc.base_address, desc.size)
    }
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
    /// Instruction index (an [`Instruction`] value).
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
    /// Save state map field (a [`SaveStateField`] value).
    pub map_field: u32,
    /// Save state attributes (combination of `RESOURCE_ATTR_*`).
    pub attributes: u32,
    /// Access condition (a [`SaveStateCondition`] value).
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

    /// Returns a pointer to the policy root array.
    ///
    /// ## Safety
    ///
    /// `self` must be the header of a valid policy buffer.
    pub unsafe fn get_policy_roots_ptr(&self) -> *const PolicyRootV1 {
        let base = core::ptr::from_ref::<Self>(self).cast::<u8>();
        // SAFETY: `self` heads a valid policy buffer (function contract), so `policy_root_offset`
        // stays within it. This only computes a pointer.
        unsafe { base.add(self.policy_root_offset as usize).cast::<PolicyRootV1>() }
    }

    /// Returns the policy roots as a slice.
    ///
    /// ## Safety
    ///
    /// `self` must be the header of a valid policy buffer.
    pub unsafe fn get_policy_roots(&self) -> &[PolicyRootV1] {
        // SAFETY: `self` heads a valid policy buffer (function contract), so the roots pointer and
        // `policy_root_count` describe an in-bounds, initialized array.
        unsafe { slice::from_raw_parts(self.get_policy_roots_ptr(), self.policy_root_count as usize) }
    }
}

impl PolicyRootV1 {
    /// Returns true if the reserved fields are all zeros.
    pub fn has_valid_reserved(&self) -> bool {
        self.reserved == [0, 0, 0]
    }

    /// Returns a pointer to the descriptors of this policy root.
    ///
    /// ## Safety
    ///
    /// `policy_base` must point to the valid policy buffer that contains this root.
    pub unsafe fn get_descriptors_ptr<T>(&self, policy_base: *const u8) -> *const T {
        // SAFETY: `policy_base` points to a valid policy buffer (function contract), so `offset`
        // stays within it. This only computes a pointer.
        unsafe { policy_base.add(self.offset as usize).cast::<T>() }
    }

    /// Returns the memory descriptors of this policy root.
    ///
    /// ## Safety
    ///
    /// `policy_base` must point to the valid policy buffer that contains this root.
    pub unsafe fn get_mem_descriptors(&self, policy_base: *const u8) -> &[MemDescriptorV1_0] {
        // SAFETY: `policy_base` points to a valid policy buffer (function contract), so the
        // descriptor pointer and `count` describe an in-bounds, initialized array.
        unsafe {
            slice::from_raw_parts(self.get_descriptors_ptr::<MemDescriptorV1_0>(policy_base), self.count as usize)
        }
    }

    /// Returns the I/O descriptors of this policy root.
    ///
    /// ## Safety
    ///
    /// `policy_base` must point to the valid policy buffer that contains this root.
    pub unsafe fn get_io_descriptors(&self, policy_base: *const u8) -> &[IoDescriptorV1_0] {
        // SAFETY: `policy_base` points to a valid policy buffer (function contract), so the
        // descriptor pointer and `count` describe an in-bounds, initialized array.
        unsafe { slice::from_raw_parts(self.get_descriptors_ptr::<IoDescriptorV1_0>(policy_base), self.count as usize) }
    }

    /// Returns the MSR descriptors of this policy root.
    ///
    /// ## Safety
    ///
    /// `policy_base` must point to the valid policy buffer that contains this root.
    pub unsafe fn get_msr_descriptors(&self, policy_base: *const u8) -> &[MsrDescriptorV1_0] {
        // SAFETY: `policy_base` points to a valid policy buffer (function contract), so the
        // descriptor pointer and `count` describe an in-bounds, initialized array.
        unsafe {
            slice::from_raw_parts(self.get_descriptors_ptr::<MsrDescriptorV1_0>(policy_base), self.count as usize)
        }
    }

    /// Returns the instruction descriptors of this policy root.
    ///
    /// ## Safety
    ///
    /// `policy_base` must point to the valid policy buffer that contains this root.
    pub unsafe fn get_instruction_descriptors(&self, policy_base: *const u8) -> &[InstructionDescriptorV1_0] {
        // SAFETY: `policy_base` points to a valid policy buffer (function contract), so the
        // descriptor pointer and `count` describe an in-bounds, initialized array.
        unsafe {
            slice::from_raw_parts(
                self.get_descriptors_ptr::<InstructionDescriptorV1_0>(policy_base),
                self.count as usize,
            )
        }
    }

    /// Returns the save state descriptors of this policy root.
    ///
    /// ## Safety
    ///
    /// `policy_base` must point to the valid policy buffer that contains this root.
    pub unsafe fn get_save_state_descriptors(&self, policy_base: *const u8) -> &[SaveStateDescriptorV1_0] {
        // SAFETY: `policy_base` points to a valid policy buffer (function contract), so the
        // descriptor pointer and `count` describe an in-bounds, initialized array.
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
