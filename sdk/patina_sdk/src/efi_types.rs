//! UEFI Wrapper Types
//!
//! Wrappers for various EFI types and definitions for use in Rust.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use r_efi::efi;

use crate::error::EfiError;

/// A wrapper for the EFI memory types.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EfiMemoryType {
    ReservedMemoryType,
    LoaderCode,
    LoaderData,
    BootServicesCode,
    BootServicesData,
    RuntimeServicesCode,
    RuntimeServicesData,
    ConventionalMemory,
    UnusableMemory,
    ACPIReclaimMemory,
    ACPIMemoryNVS,
    MemoryMappedIO,
    MemoryMappedIOPortSpace,
    PalCode,
    PersistentMemory,
    UnacceptedMemoryType,

    // Custom memory types can only be created through `from_efi` with the custom
    // memory type value. This is to ensure that the custom memory types cannot
    // be created with invalid values.
    OemMemoryType(CustomMemoryType),
    OsMemoryType(CustomMemoryType),
}

/// Wrapper for custom memory types to prevent manual creation of non-compliant
/// memory types.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct CustomMemoryType {
    // internally private to ensure that the memory type passes validity checks.
    memory_type: efi::MemoryType,
}

impl EfiMemoryType {
    pub fn from_efi(value: efi::MemoryType) -> Result<Self, EfiError> {
        let memory_type = match value {
            efi::RESERVED_MEMORY_TYPE => EfiMemoryType::ReservedMemoryType,
            efi::LOADER_CODE => EfiMemoryType::LoaderCode,
            efi::LOADER_DATA => EfiMemoryType::LoaderData,
            efi::BOOT_SERVICES_CODE => EfiMemoryType::BootServicesCode,
            efi::BOOT_SERVICES_DATA => EfiMemoryType::BootServicesData,
            efi::RUNTIME_SERVICES_CODE => EfiMemoryType::RuntimeServicesCode,
            efi::RUNTIME_SERVICES_DATA => EfiMemoryType::RuntimeServicesData,
            efi::CONVENTIONAL_MEMORY => EfiMemoryType::ConventionalMemory,
            efi::UNUSABLE_MEMORY => EfiMemoryType::UnusableMemory,
            efi::ACPI_RECLAIM_MEMORY => EfiMemoryType::ACPIReclaimMemory,
            efi::ACPI_MEMORY_NVS => EfiMemoryType::ACPIMemoryNVS,
            efi::MEMORY_MAPPED_IO => EfiMemoryType::MemoryMappedIO,
            efi::MEMORY_MAPPED_IO_PORT_SPACE => EfiMemoryType::MemoryMappedIOPortSpace,
            efi::PAL_CODE => EfiMemoryType::PalCode,
            efi::PERSISTENT_MEMORY => EfiMemoryType::PersistentMemory,
            efi::UNACCEPTED_MEMORY_TYPE => EfiMemoryType::UnacceptedMemoryType,
            0x70000000..=0x7FFFFFFF => EfiMemoryType::OemMemoryType(CustomMemoryType { memory_type: value }),
            0x80000000..=0xFFFFFFFF => EfiMemoryType::OsMemoryType(CustomMemoryType { memory_type: value }),
            _ => return Err(EfiError::InvalidParameter),
        };

        Ok(memory_type)
    }
}

impl From<EfiMemoryType> for efi::MemoryType {
    fn from(value: EfiMemoryType) -> Self {
        match value {
            EfiMemoryType::ReservedMemoryType => efi::RESERVED_MEMORY_TYPE,
            EfiMemoryType::LoaderCode => efi::LOADER_CODE,
            EfiMemoryType::LoaderData => efi::LOADER_DATA,
            EfiMemoryType::BootServicesCode => efi::BOOT_SERVICES_CODE,
            EfiMemoryType::BootServicesData => efi::BOOT_SERVICES_DATA,
            EfiMemoryType::RuntimeServicesCode => efi::RUNTIME_SERVICES_CODE,
            EfiMemoryType::RuntimeServicesData => efi::RUNTIME_SERVICES_DATA,
            EfiMemoryType::ConventionalMemory => efi::CONVENTIONAL_MEMORY,
            EfiMemoryType::UnusableMemory => efi::UNUSABLE_MEMORY,
            EfiMemoryType::ACPIReclaimMemory => efi::ACPI_RECLAIM_MEMORY,
            EfiMemoryType::ACPIMemoryNVS => efi::ACPI_MEMORY_NVS,
            EfiMemoryType::MemoryMappedIO => efi::MEMORY_MAPPED_IO,
            EfiMemoryType::MemoryMappedIOPortSpace => efi::MEMORY_MAPPED_IO_PORT_SPACE,
            EfiMemoryType::PalCode => efi::PAL_CODE,
            EfiMemoryType::PersistentMemory => efi::PERSISTENT_MEMORY,
            EfiMemoryType::UnacceptedMemoryType => efi::UNACCEPTED_MEMORY_TYPE,
            EfiMemoryType::OemMemoryType(custom_memory_type) => custom_memory_type.memory_type,
            EfiMemoryType::OsMemoryType(custom_memory_type) => custom_memory_type.memory_type,
        }
    }
}
