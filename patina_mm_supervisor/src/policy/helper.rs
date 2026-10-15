//! Firmware Policy Dump Helpers
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use super::{
    ACCESS_ATTR_ALLOW, InstructionDescriptorV1_0, IoDescriptorV1_0, MemDescriptorV1_0, MsrDescriptorV1_0,
    RESOURCE_ATTR_EXECUTE, RESOURCE_ATTR_READ, RESOURCE_ATTR_WRITE, SaveStateDescriptorV1_0, SecurePolicyDataV1_0,
    TYPE_INSTRUCTION, TYPE_IO, TYPE_MEM, TYPE_MSR, TYPE_SAVE_STATE,
};
use core::mem::size_of;

/// Dumps a single memory policy entry for debugging.
pub fn dump_mem_policy_entry(desc: &MemDescriptorV1_0) {
    let readable = if (desc.mem_attributes & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
    let writable = if (desc.mem_attributes & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
    let executable = if (desc.mem_attributes & RESOURCE_ATTR_EXECUTE) != 0 { "X" } else { "." };

    log::info!(
        "  MEM: [0x{:016x}-0x{:016x}] {}{}{}",
        desc.base_address,
        desc.base_address.saturating_add(desc.size).saturating_sub(1),
        readable,
        writable,
        executable
    );
}

/// Dumps policy data for debugging (like `DumpSmmPolicyData`).
///
/// ## Safety
///
/// The caller must ensure that `policy_ptr` points to a valid policy buffer.
pub unsafe fn dump_policy(policy_ptr: *const u8) {
    if policy_ptr.is_null() {
        log::error!("dump_policy: null pointer");
        return;
    }

    // SAFETY: `policy_ptr` is non-null (checked above) and, per this function's contract, points
    // to a valid policy buffer, so the header can be reborrowed for reading.
    let policy = unsafe { &*policy_ptr.cast::<SecurePolicyDataV1_0>() };

    let len = policy.size as usize;
    if len < size_of::<SecurePolicyDataV1_0>() {
        log::error!("dump_policy: invalid policy size: 0x{len:x}");
        return;
    }

    log::info!("SMM_SUPV_SECURE_POLICY_DATA_V1_0:");
    log::info!("  Version: {}.{}", policy.version_major, policy.version_minor);
    log::info!("  Size: 0x{:x}", policy.size);
    log::info!("  MemoryPolicyOffset: 0x{:x}", policy.memory_policy_offset);
    log::info!("  MemoryPolicyCount: 0x{:x}", policy.memory_policy_count);
    log::info!("  Flags: 0x{:x}", policy.flags);
    log::info!("  Capabilities: 0x{:x}", policy.capabilities);
    log::info!("  PolicyRootOffset: 0x{:x}", policy.policy_root_offset);
    log::info!("  PolicyRootCount: 0x{:x}", policy.policy_root_count);

    // SAFETY: `policy` is the validated header of a valid policy buffer, so its policy-root array
    // (root pointer + count) is in-bounds.
    let policy_roots = unsafe { policy.get_policy_roots() };

    for (index, root) in policy_roots.iter().enumerate() {
        log::info!("Policy Root {index}:");
        log::info!("  Version: {}", root.version);
        log::info!("  PolicyRootSize: {}", root.policy_root_size);
        log::info!("  Type: {}", root.policy_type);
        log::info!("  Offset: 0x{:x}", root.offset);
        log::info!("  Count: {}", root.count);
        log::info!("  AccessAttr: {}", if root.access_attr == ACCESS_ATTR_ALLOW { "ALLOW" } else { "DENY" });

        match root.policy_type {
            TYPE_MEM => {
                assert!(
                    root.offset as usize + root.count as usize * size_of::<MemDescriptorV1_0>() <= len,
                    "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                    index,
                    root.offset,
                    root.count,
                    len
                );
                // SAFETY: `root` came from the validated policy buffer and `policy_ptr` points to
                // that same buffer, so the descriptor array is in-bounds.
                let descriptors = unsafe { root.get_mem_descriptors(policy_ptr) };
                for desc in descriptors {
                    dump_mem_policy_entry(desc);
                }
            }
            TYPE_IO => {
                assert!(
                    root.offset as usize + root.count as usize * size_of::<IoDescriptorV1_0>() <= len,
                    "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                    index,
                    root.offset,
                    root.count,
                    len
                );
                // SAFETY: `root` came from the validated policy buffer and `policy_ptr` points to
                // that same buffer, so the descriptor array is in-bounds.
                let descriptors = unsafe { root.get_io_descriptors(policy_ptr) };
                for desc in descriptors {
                    let readable = if (u32::from(desc.attributes) & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
                    let writable = if (u32::from(desc.attributes) & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
                    log::info!(
                        "  IO: [0x{:04x}-0x{:04x}] {}{}",
                        desc.io_address,
                        u32::from(desc.io_address).saturating_add(u32::from(desc.length_or_width)).saturating_sub(1),
                        readable,
                        writable
                    );
                }
            }
            TYPE_MSR => {
                assert!(
                    root.offset as usize + root.count as usize * size_of::<MsrDescriptorV1_0>() <= len,
                    "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                    index,
                    root.offset,
                    root.count,
                    len
                );
                // SAFETY: `root` came from the validated policy buffer and `policy_ptr` points to
                // that same buffer, so the descriptor array is in-bounds.
                let descriptors = unsafe { root.get_msr_descriptors(policy_ptr) };
                for desc in descriptors {
                    let readable = if (u32::from(desc.attributes) & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
                    let writable = if (u32::from(desc.attributes) & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
                    log::info!(
                        "  MSR: [0x{:08x}-0x{:08x}] {}{}",
                        desc.msr_address,
                        desc.msr_address.saturating_add(u32::from(desc.length)).saturating_sub(1),
                        readable,
                        writable
                    );
                }
            }
            TYPE_INSTRUCTION => {
                assert!(
                    root.offset as usize + root.count as usize * size_of::<InstructionDescriptorV1_0>() <= len,
                    "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                    index,
                    root.offset,
                    root.count,
                    len
                );
                // SAFETY: `root` came from the validated policy buffer and `policy_ptr` points to
                // that same buffer, so the descriptor array is in-bounds.
                let descriptors = unsafe { root.get_instruction_descriptors(policy_ptr) };
                for desc in descriptors {
                    let name = match desc.instruction_index {
                        0 => "CLI",
                        1 => "WBINVD",
                        2 => "HLT",
                        _ => "UNKNOWN",
                    };
                    let executable = if (u32::from(desc.attributes) & RESOURCE_ATTR_EXECUTE) != 0 { "X" } else { "." };
                    log::info!("  INSTRUCTION: {name} {executable}");
                }
            }
            TYPE_SAVE_STATE => {
                assert!(
                    root.offset as usize + root.count as usize * size_of::<SaveStateDescriptorV1_0>() <= len,
                    "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                    index,
                    root.offset,
                    root.count,
                    len
                );
                // SAFETY: `root` came from the validated policy buffer and `policy_ptr` points to
                // that same buffer, so the descriptor array is in-bounds.
                let descriptors = unsafe { root.get_save_state_descriptors(policy_ptr) };
                for desc in descriptors {
                    let field = match desc.map_field {
                        0 => "RAX",
                        1 => "IO_TRAP",
                        _ => "UNKNOWN",
                    };
                    let condition = match desc.access_condition {
                        0 => "Unconditional",
                        1 => "IoRead",
                        2 => "IoWrite",
                        _ => "Unknown",
                    };
                    log::info!("  SAVESTATE: {} attr=0x{:x} cond={}", field, desc.attributes, condition);
                }
            }
            _ => {
                log::error!("  Unknown policy type: {}", root.policy_type);
            }
        }
    }
}
