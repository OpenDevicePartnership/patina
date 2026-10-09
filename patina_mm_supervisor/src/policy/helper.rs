//! Firmware Policy Dump
//!
//! Prints the contents of a firmware policy blob to the log: each root, and the memory,
//! I/O, MSR, instruction and save-state entries it points at. Used for debugging what a
//! platform actually handed down, and never on a path that decides access.
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
    let r = if (desc.mem_attributes & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
    let w = if (desc.mem_attributes & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
    let x = if (desc.mem_attributes & RESOURCE_ATTR_EXECUTE) != 0 { "X" } else { "." };

    log::info!(
        "  MEM: [0x{:016x}-0x{:016x}] {}{}{}",
        desc.base_address,
        desc.base_address.saturating_add(desc.size).saturating_sub(1),
        r,
        w,
        x
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

    for (i, root) in policy_roots.iter().enumerate() {
        log::info!("Policy Root {i}:");
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
                    i,
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
                    i,
                    root.offset,
                    root.count,
                    len
                );
                // SAFETY: `root` came from the validated policy buffer and `policy_ptr` points to
                // that same buffer, so the descriptor array is in-bounds.
                let descriptors = unsafe { root.get_io_descriptors(policy_ptr) };
                for desc in descriptors {
                    let r = if (u32::from(desc.attributes) & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
                    let w = if (u32::from(desc.attributes) & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
                    log::info!(
                        "  IO: [0x{:04x}-0x{:04x}] {}{}",
                        desc.io_address,
                        u32::from(desc.io_address).saturating_add(u32::from(desc.length_or_width)).saturating_sub(1),
                        r,
                        w
                    );
                }
            }
            TYPE_MSR => {
                assert!(
                    root.offset as usize + root.count as usize * size_of::<MsrDescriptorV1_0>() <= len,
                    "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                    i,
                    root.offset,
                    root.count,
                    len
                );
                // SAFETY: `root` came from the validated policy buffer and `policy_ptr` points to
                // that same buffer, so the descriptor array is in-bounds.
                let descriptors = unsafe { root.get_msr_descriptors(policy_ptr) };
                for desc in descriptors {
                    let r = if (u32::from(desc.attributes) & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
                    let w = if (u32::from(desc.attributes) & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
                    log::info!(
                        "  MSR: [0x{:08x}-0x{:08x}] {}{}",
                        desc.msr_address,
                        desc.msr_address.saturating_add(u32::from(desc.length)).saturating_sub(1),
                        r,
                        w
                    );
                }
            }
            TYPE_INSTRUCTION => {
                assert!(
                    root.offset as usize + root.count as usize * size_of::<InstructionDescriptorV1_0>() <= len,
                    "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                    i,
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
                    let x = if (u32::from(desc.attributes) & RESOURCE_ATTR_EXECUTE) != 0 { "X" } else { "." };
                    log::info!("  INSTRUCTION: {name} {x}");
                }
            }
            TYPE_SAVE_STATE => {
                assert!(
                    root.offset as usize + root.count as usize * size_of::<SaveStateDescriptorV1_0>() <= len,
                    "  Policy root {} out of bounds (offset=0x{:x}, count={}, total_size=0x{:x})",
                    i,
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    use super::super::test_support::{Descriptors, PolicyBuilder, full_policy, instruction, io, msr, save_state};
    use super::super::{ACCESS_ATTR_DENY, Instruction, RESOURCE_ATTR_COND_READ, SaveStateCondition, SaveStateField};

    #[test]
    fn test_dump_mem_policy_entry() {
        // Just verify it doesn't panic
        let desc = MemDescriptorV1_0 {
            base_address: 0x1000,
            size: 0x1000,
            mem_attributes: RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE,
            reserved: 0,
        };
        dump_mem_policy_entry(&desc);

        // An entry granting nothing renders every permission as absent.
        dump_mem_policy_entry(&MemDescriptorV1_0 { mem_attributes: 0, ..desc });
        dump_mem_policy_entry(&MemDescriptorV1_0 { mem_attributes: RESOURCE_ATTR_EXECUTE, ..desc });
    }

    #[test]
    fn test_dump_policy_walks_every_root_type() {
        let policy = full_policy()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Instruction(vec![]))
            .root(ACCESS_ATTR_ALLOW, Descriptors::Unknown(99))
            .build();

        // SAFETY: the builder produced a valid policy buffer that outlives this call.
        unsafe { dump_policy(policy.as_ptr()) };
    }

    #[test]
    fn test_dump_policy_renders_unset_attributes_and_unknown_entries() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_DENY, Descriptors::Io(vec![io(0x60, 1, 0)]))
            .root(ACCESS_ATTR_DENY, Descriptors::Msr(vec![msr(0x1B, 1, 0)]))
            .root(
                ACCESS_ATTR_DENY,
                Descriptors::Instruction(vec![
                    instruction(Instruction::Cli, 0),
                    instruction(Instruction::Wbinvd, RESOURCE_ATTR_EXECUTE as u16),
                    InstructionDescriptorV1_0 { instruction_index: 9, attributes: 0, reserved: 0 },
                ]),
            )
            .root(
                ACCESS_ATTR_DENY,
                Descriptors::SaveState(vec![
                    save_state(SaveStateField::IoTrap, RESOURCE_ATTR_COND_READ, SaveStateCondition::IoWrite),
                    SaveStateDescriptorV1_0 { map_field: 9, attributes: 0, access_condition: 9, reserved: 0 },
                ]),
            )
            .build();

        // SAFETY: the builder produced a valid policy buffer that outlives this call.
        unsafe { dump_policy(policy.as_ptr()) };
    }

    #[test]
    fn test_dump_policy_rejects_null_and_undersized_buffers() {
        // SAFETY: `dump_policy` checks for null before dereferencing.
        unsafe { dump_policy(core::ptr::null()) };

        let policy = PolicyBuilder::new().declared_size(4).build();
        // SAFETY: the header is valid; only its `size` field is understated.
        unsafe { dump_policy(policy.as_ptr()) };
    }

    #[test]
    fn test_dump_policy_formats_entry_ranges_when_logging_is_enabled() {
        // The dump builds every range through log::info!, which skips its arguments when no
        // logger is installed. The silent test logger reports every level as enabled, so the
        // end address arithmetic for a memory, I/O and MSR entry runs rather than being
        // skipped.
        crate::test_support::init_test_logger();

        let policy = full_policy().build();
        // SAFETY: the builder produced a valid policy buffer that outlives this call.
        unsafe { dump_policy(policy.as_ptr()) };

        dump_mem_policy_entry(&MemDescriptorV1_0 {
            base_address: 0x2000,
            size: 0x1000,
            mem_attributes: RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE | RESOURCE_ATTR_EXECUTE,
            reserved: 0,
        });

        // A zero-sized entry would underflow without the saturating subtraction.
        dump_mem_policy_entry(&MemDescriptorV1_0 { base_address: 0, size: 0, mem_attributes: 0, reserved: 0 });
    }
}
