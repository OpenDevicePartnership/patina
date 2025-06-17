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
    /// Reserved memory for platform uses.
    ReservedMemoryType,
    /// The code portions of a loaded application, e.g. the entire loaded image (PE).
    LoaderCode,
    /// The data portions of a loaded application, e.g. data allocations made and used by an application.
    LoaderData,
    /// The code portions of a loaded Boot Services Driver, e.g. the entire loaded image (PE).
    BootServicesCode,
    /// The data portions of a loaded Boot Services Driver, e.g. data allocations made and used by a driver.
    BootServicesData,
    /// The code portions of a loaded Runtime Services Driver, e.g. the entire loaded image (PE).
    RuntimeServicesCode,
    /// The data portions of a loaded Runtime Services Driver, e.g. data allocations made and used by a driver.
    RuntimeServicesData,
    /// Free (unallocated) memory.
    ConventionalMemory,
    /// Memory in which errors have been detected.
    UnusableMemory,
    /// Memory reserved for runtime ACPI non-volatile storage.
    ACPIReclaimMemory,
    /// Address space reserved for use by the firmware.
    ACPIMemoryNVS,
    /// Memory-mapped IO region, mapped by the OS to a virtual address so it can be accessed by EFI runtime services.
    MemoryMappedIO,
    /// System memory-mapped IO region that is used to translate memory cycles to IO cycles by the processor.
    MemoryMappedIOPortSpace,
    /// Address space reserved by the firmware for code that is part of the processor.
    PalCode,
    /// EfiConventionalMemory that supports byte-addressable non-volatility.
    PersistentMemory,
    /// Present in the system, but not accepted / initalized for use by the system's underlying memory isolation
    /// technology.
    UnacceptedMemoryType,
    /// Custom memory types can only be created through `from_efi` with the custom
    /// memory type value. This is to ensure that the custom memory types cannot
    /// be created with invalid values.
    OemMemoryType(CustomMemoryType),
    /// Custom memory types that are defined by the OS.
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
    /// Converts a [efi::MemoryType] to an [EfiMemoryType].
    ///
    /// Returns an [EfiError] if the underlying [u32] value does not match any known EFI memory types.
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
