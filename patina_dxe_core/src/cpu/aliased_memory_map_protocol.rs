//! Protocol for creating aliased memory mappings.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//! SPDX-License-Identifier: Apache-2.0

use crate::dxe_services;
use alloc::boxed::Box;
use patina::{
    BinaryGuid,
    error::Result,
    protocol::ProtocolInterface,
    standard::efi,
    uefi::boot_services::{BootServices, StandardBootServices},
};

const PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("CD7B3711-3CE3-456D-B6E9-74364F0BA344");

type CreateAliasedMapping = extern "efiapi" fn(
    *const AliasedMemoryMappingProtocol,
    efi::VirtualAddress,
    efi::PhysicalAddress,
    u64,
    u64,
) -> efi::Status;

type UnmapAliasedMapping =
    extern "efiapi" fn(*const AliasedMemoryMappingProtocol, efi::VirtualAddress, u64) -> efi::Status;

#[repr(C)]
struct AliasedMemoryMappingProtocol {
    create_aliased_mapping: CreateAliasedMapping,
    unmap_aliased_mapping: UnmapAliasedMapping,
}

#[repr(C)]
struct AliasedMemoryMappingProtocolImpl {
    protocol: AliasedMemoryMappingProtocol,
}

// SAFETY: AliasedMemoryMappingProtocolImpl provides a valid protocol structure with a stable GUID.
unsafe impl ProtocolInterface for AliasedMemoryMappingProtocolImpl {
    const PROTOCOL_GUID: patina::BinaryGuid = PROTOCOL_GUID;
}

extern "efiapi" fn create_aliased_mapping(
    this: *const AliasedMemoryMappingProtocol,
    virtual_address: efi::VirtualAddress,
    physical_address: efi::PhysicalAddress,
    length: u64,
    attributes: u64,
) -> efi::Status {
    if this.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    match dxe_services::core_map_aliased_memory_region(virtual_address, physical_address, length, attributes) {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => err.into(),
    }
}

extern "efiapi" fn unmap_aliased_mapping(
    this: *const AliasedMemoryMappingProtocol,
    virtual_address: efi::VirtualAddress,
    length: u64,
) -> efi::Status {
    if this.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    match dxe_services::core_unmap_aliased_memory_region(virtual_address, length) {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => err.into(),
    }
}

impl AliasedMemoryMappingProtocolImpl {
    fn new() -> Self {
        Self { protocol: AliasedMemoryMappingProtocol { create_aliased_mapping, unmap_aliased_mapping } }
    }
}

pub(super) fn install(bs: &StandardBootServices, handle: efi::Handle) -> Result<()> {
    let interface = Box::leak(Box::new(AliasedMemoryMappingProtocolImpl::new()));

    bs.install_protocol_interface(Some(handle), interface)
        .inspect_err(|_| log::error!("Failed to install aliased memory mapping protocol"))?;

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::{
        GCD,
        test_support::{MockPageTable, MockPageTableWrapper, with_clean_global_lock},
    };
    use patina_paging::MemoryAttributes;
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn test_create_aliased_mapping_rejects_null_protocol() {
        assert_eq!(
            create_aliased_mapping(core::ptr::null(), 0x8000_0000, 0x1000_0000, 0x1000, efi::MEMORY_WB),
            efi::Status::INVALID_PARAMETER
        );
    }

    #[test]
    fn test_unmap_aliased_mapping_rejects_null_protocol() {
        assert_eq!(unmap_aliased_mapping(core::ptr::null(), 0x8000_0000, 0x1000), efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_create_aliased_mapping_records_mapping_in_gcd_page_table() {
        with_clean_global_lock(|| {
            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            GCD.add_test_page_table(Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table))));
            let protocol = AliasedMemoryMappingProtocolImpl::new();

            let status = create_aliased_mapping(
                &raw const protocol.protocol,
                0x8000_0000,
                0x1000_0000,
                0x20_0000,
                efi::MEMORY_WB | efi::MEMORY_XP,
            );

            assert_eq!(status, efi::Status::SUCCESS);
            assert_eq!(
                mock_table.borrow().get_aliased_mapped_regions(),
                vec![(
                    0x8000_0000,
                    0x1000_0000,
                    0x20_0000,
                    MemoryAttributes::Writeback | MemoryAttributes::ExecuteProtect,
                )]
            );

            assert_eq!(
                unmap_aliased_mapping(&raw const protocol.protocol, 0x8000_0000, 0x20_0000),
                efi::Status::SUCCESS
            );
            assert_eq!(mock_table.borrow().get_unmapped_regions(), vec![(0x8000_0000, 0x20_0000)]);
        })
        .unwrap();
    }
}
