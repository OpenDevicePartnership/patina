//! Firmware policy dump helpers.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use super::{
    ACCESS_ATTR_ALLOW, ACCESS_ATTR_DENY, Instruction, MemDescriptorV1_0, RESOURCE_ATTR_EXECUTE, RESOURCE_ATTR_READ,
    RESOURCE_ATTR_WRITE, SaveStateCondition, SaveStateField, SecurePolicyDataV1_0, TYPE_INSTRUCTION, TYPE_IO, TYPE_MEM,
    TYPE_MSR, TYPE_SAVE_STATE, descriptor_size,
};
use core::mem::size_of;

/// Logs one memory policy entry for debugging.
pub fn dump_mem_policy_entry(desc: &MemDescriptorV1_0) {
    let readable = if (desc.mem_attributes & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
    let writable = if (desc.mem_attributes & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
    let executable = if (desc.mem_attributes & RESOURCE_ATTR_EXECUTE) != 0 { "X" } else { "." };

    log::debug!(
        "  MEM: [0x{:016x}-0x{:016x}] {}{}{}",
        desc.base_address,
        desc.base_address.saturating_add(desc.size).saturating_sub(1),
        readable,
        writable,
        executable
    );
}

/// Logs a policy for debugging, like the C `DumpSmmPolicyData`.
///
/// ## Safety
///
/// `policy_ptr` must be null or point to a valid policy buffer.
///
/// A root whose descriptors extend past the header `size`, or whose type is unknown, is logged
/// and skipped.
pub unsafe fn dump_policy(policy_ptr: *const u8) {
    if policy_ptr.is_null() {
        log::error!("dump_policy: null pointer");
        return;
    }

    // SAFETY: `policy_ptr` is non-null (checked above), so by the function contract it points to
    // a valid policy buffer and the header can be read.
    let policy = unsafe { &*policy_ptr.cast::<SecurePolicyDataV1_0>() };

    let len = policy.size as usize;
    if len < size_of::<SecurePolicyDataV1_0>() {
        log::error!("dump_policy: invalid policy size: 0x{len:x}");
        return;
    }

    log::debug!("SMM_SUPV_SECURE_POLICY_DATA_V1_0:");
    log::debug!("  Version: {}.{}", policy.version_major, policy.version_minor);
    log::debug!("  Size: 0x{:x}", policy.size);
    log::debug!("  MemoryPolicyOffset: 0x{:x}", policy.memory_policy_offset);
    log::debug!("  MemoryPolicyCount: 0x{:x}", policy.memory_policy_count);
    log::debug!("  Flags: 0x{:x}", policy.flags);
    log::debug!("  Capabilities: 0x{:x}", policy.capabilities);
    log::debug!("  PolicyRootOffset: 0x{:x}", policy.policy_root_offset);
    log::debug!("  PolicyRootCount: 0x{:x}", policy.policy_root_count);

    // SAFETY: `policy` is the header of a valid policy buffer, so its policy root array is in
    // bounds.
    let policy_roots = unsafe { policy.get_policy_roots() };

    for (index, root) in policy_roots.iter().enumerate() {
        log::debug!("Policy Root {index}:");
        log::debug!("  Version: {}", root.version);
        log::debug!("  PolicyRootSize: {}", root.policy_root_size);
        log::debug!("  Type: {}", root.policy_type);
        log::debug!("  Offset: 0x{:x}", root.offset);
        log::debug!("  Count: {}", root.count);
        match root.access_attr {
            ACCESS_ATTR_ALLOW => log::debug!("  AccessAttr: ALLOW"),
            ACCESS_ATTR_DENY => log::debug!("  AccessAttr: DENY"),
            other => log::debug!("  AccessAttr: unknown ({other})"),
        }

        let Some(size) = descriptor_size(root.policy_type) else {
            log::error!("  Unknown policy type: {}", root.policy_type);
            continue;
        };
        let end = (root.count as usize).checked_mul(size).and_then(|bytes| (root.offset as usize).checked_add(bytes));
        if end.is_none_or(|end| end > len) {
            log::error!(
                "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                index,
                root.offset,
                root.count,
                len
            );
            continue;
        }

        // SAFETY (for each accessor below): `root` comes from the buffer at `policy_ptr`, and the
        // check above keeps its descriptor array within the buffer's declared size.
        match root.policy_type {
            TYPE_MEM => {
                // SAFETY: See above.
                let descriptors = unsafe { root.get_mem_descriptors(policy_ptr) };
                for desc in descriptors {
                    dump_mem_policy_entry(desc);
                }
            }
            TYPE_IO => {
                // SAFETY: See above.
                let descriptors = unsafe { root.get_io_descriptors(policy_ptr) };
                for desc in descriptors {
                    let readable = if (u32::from(desc.attributes) & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
                    let writable = if (u32::from(desc.attributes) & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
                    log::debug!(
                        "  IO: [0x{:04x}-0x{:04x}] {}{}",
                        desc.io_address,
                        u32::from(desc.io_address).saturating_add(u32::from(desc.length_or_width)).saturating_sub(1),
                        readable,
                        writable
                    );
                }
            }
            TYPE_MSR => {
                // SAFETY: See above.
                let descriptors = unsafe { root.get_msr_descriptors(policy_ptr) };
                for desc in descriptors {
                    let readable = if (u32::from(desc.attributes) & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
                    let writable = if (u32::from(desc.attributes) & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
                    log::debug!(
                        "  MSR: [0x{:08x}-0x{:08x}] {}{}",
                        desc.msr_address,
                        desc.msr_address.saturating_add(u32::from(desc.length)).saturating_sub(1),
                        readable,
                        writable
                    );
                }
            }
            TYPE_INSTRUCTION => {
                // SAFETY: See above.
                let descriptors = unsafe { root.get_instruction_descriptors(policy_ptr) };
                for desc in descriptors {
                    let name = match desc.instruction_index {
                        index if index == Instruction::Cli.as_index() => "CLI",
                        index if index == Instruction::Wbinvd.as_index() => "WBINVD",
                        index if index == Instruction::Hlt.as_index() => "HLT",
                        _ => "UNKNOWN",
                    };
                    let executable = if (u32::from(desc.attributes) & RESOURCE_ATTR_EXECUTE) != 0 { "X" } else { "." };
                    log::debug!("  INSTRUCTION: {name} {executable}");
                }
            }
            TYPE_SAVE_STATE => {
                // SAFETY: See above.
                let descriptors = unsafe { root.get_save_state_descriptors(policy_ptr) };
                for desc in descriptors {
                    let field = match desc.map_field {
                        field if field == SaveStateField::Rax.as_index() => "RAX",
                        field if field == SaveStateField::IoTrap.as_index() => "IO_TRAP",
                        _ => "UNKNOWN",
                    };
                    let condition = match desc.access_condition {
                        condition if condition == SaveStateCondition::Unconditional as u32 => "Unconditional",
                        condition if condition == SaveStateCondition::IoRead as u32 => "IoRead",
                        condition if condition == SaveStateCondition::IoWrite as u32 => "IoWrite",
                        _ => "Unknown",
                    };
                    log::debug!("  SAVESTATE: {} attr=0x{:x} cond={}", field, desc.attributes, condition);
                }
            }
            // `descriptor_size` returned `Some`, so the type is one of the arms above.
            _ => {}
        }
    }
}
