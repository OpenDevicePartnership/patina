//! HOB Lookup Helpers
//!
//! Search primitives over a Hand-Off Block list: locating a GUID HOB's payload or a
//! memory allocation module by name. These are pure lookups that report a miss as
//! [`Option::None`]; deciding whether a miss is fatal belongs to the caller.
//!
//! Validation of HOB *content* lives in [`super::validation`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::pi::hob::{self, Hob, PhaseHandoffInformationTable};

/// Finds the first memory allocation module HOB matching both names.
///
/// A module is identified by two GUIDs: `allocation_name` is the allocation descriptor's name,
/// which says which producer reserved the pages, and `module_name` identifies the image occupying
/// them. Both must match, because the same image is described by different allocation names
/// depending on who allocated it.
///
/// Returns `None` when no HOB matches both.
pub(crate) fn find_module<'a>(
    hobs: impl IntoIterator<Item = Hob<'a>>,
    allocation_name: patina::BinaryGuid,
    module_name: patina::BinaryGuid,
) -> Option<&'a hob::MemoryAllocationModule> {
    for current_hob in hobs {
        if let Hob::MemoryAllocationModule(module) = current_hob
            && module.alloc_descriptor.name == allocation_name
            && module.module_name == module_name
        {
            log::info!(
                "Found MM module {module_name:?}: entry_point=0x{:016x}, base=0x{:016x}, size=0x{:x}",
                module.entry_point,
                module.alloc_descriptor.memory_base_address,
                module.alloc_descriptor.memory_length
            );
            return Some(module);
        }
    }

    None
}

/// Finds the first GUID HOB matching `target_guid` and returns its data slice.
///
/// Returns `None` if no matching HOB is found.
pub(crate) fn find_guid_hob(
    hob_list_info: &PhaseHandoffInformationTable,
    target_guid: patina::BinaryGuid,
) -> Option<&[u8]> {
    for current_hob in &Hob::Handoff(hob_list_info) {
        if let Hob::GuidHob(guid_hob, data) = current_hob
            && guid_hob.name == target_guid
        {
            return Some(data);
        }
    }
    None
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina::management_mode::supervisor::{
        MM_SUPERVISOR_CORE_GUID, MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID,
        MM_SUPERVISOR_USER_GUID,
    };
    use patina::pi::guid::HOB_MEMORY_ALLOC_MODULE_GUID;

    use crate::comm_buffer::MM_COMMON_REGION_HOB_GUID;
    use crate::hob::pass_down::MM_SUPV_PASS_DOWN_HOB_GUID;
    use crate::test_support::*;

    #[test]
    fn test_find_module_selects_init_and_core_allocations() {
        let init = allocation_module(HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, 0x10_1234);
        let mut core =
            allocation_module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_CORE_GUID, 0x20_1234);
        core.alloc_descriptor.memory_base_address = 0x20_0000;
        let hobs = [Hob::MemoryAllocationModule(&core), Hob::MemoryAllocationModule(&init)];

        assert_eq!(find_module(hobs.clone(), HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID), Some(&init));
        assert_eq!(
            find_module(hobs.clone(), MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_CORE_GUID),
            Some(&core)
        );
        assert_eq!(find_module(hobs, MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID), None);
    }

    #[test]
    fn test_find_module_selects_matching_user_module() {
        let wrong_allocation = allocation_module(MM_SUPERVISOR_CORE_GUID, MM_SUPERVISOR_USER_GUID, 0x1111);
        let wrong_module =
            allocation_module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_CORE_GUID, 0x2222);
        let matching = allocation_module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_USER_GUID, 0x3333);

        assert_eq!(
            find_module(
                [
                    Hob::MemoryAllocationModule(&wrong_allocation),
                    Hob::MemoryAllocationModule(&wrong_module),
                    Hob::MemoryAllocationModule(&matching),
                ],
                MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
                MM_SUPERVISOR_USER_GUID,
            ),
            Some(&matching)
        );
        assert_eq!(
            find_module(
                [Hob::MemoryAllocationModule(&wrong_allocation), Hob::MemoryAllocationModule(&wrong_module)],
                MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
                MM_SUPERVISOR_USER_GUID,
            ),
            None
        );
    }

    #[test]
    fn test_find_guid_hob_selects_first_matching_hob() {
        // A HOB list is built in eight-byte units, so every payload here is one unit long.
        // Two of them carry the same GUID, which is what shows that the first match wins.
        let mut list = RawHobList::new();
        list.push_guid_hob(MM_SUPV_PASS_DOWN_HOB_GUID, &[0x11; 8]);
        list.push_guid_hob(MM_COMMON_REGION_HOB_GUID, &[0x22; 8]);
        list.push_guid_hob(MM_COMMON_REGION_HOB_GUID, &[0x44; 8]);
        let list = list.finish();

        assert_eq!(find_guid_hob(list.handoff(), MM_COMMON_REGION_HOB_GUID), Some([0x22; 8].as_slice()));
        assert_eq!(find_guid_hob(list.handoff(), MM_SUPV_PASS_DOWN_HOB_GUID), Some([0x11; 8].as_slice()));
        // A GUID no HOB carries is a miss, not a match on the nearest one.
        assert_eq!(find_guid_hob(list.handoff(), MM_SUPERVISOR_CORE_GUID), None);
    }
}
