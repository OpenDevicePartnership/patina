//! ACPI Service Error Definitions.
//!
//! Defines standard errors during operation of the ACPI service interface.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::standard::efi;

/// Custom errors for ACPI operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiError {
    /// Memory allocation failed (usually because the system is out of memory).
    AllocationFailed,
    /// The system page size is not 64B aligned (required for the FACS and UEFI tables in ACPI 2.0+).
    /// If this error occurs, the service will be unable to install the FACS and UEFI tables.
    FacsUefiNot64BAligned,
    /// An invalid ACPI signature was passed into a function expecting a specific signature.
    InvalidSignature,
    /// There was an attempt to install the FADT more than once.
    /// While most ACPI tables are allowed to be duplicated, the FADT is not.
    FadtAlreadyInstalled,
    /// Boot services was unable to install the ACPI table using `install_configuration_table`,
    InstallConfigurationTableFailed,
    /// An invalid table key was passed into `uninstall_acpi_table`,
    /// A table key is invalid when it is not a known value returned by `install_acpi_table`,
    InvalidTableKey,
    /// An out-of-bounds index was passed into `get_acpi_table`,
    InvalidTableIndex,
    /// There was an attempt to unregister a notify function that was not previously registered.
    InvalidNotifyUnregister,
    /// Memory free failed.
    FreeFailed,
    /// The XSDT passed in from the HOB has an invalid length (less than the standard header length).
    XsdtInvalidLengthFromHob,
    /// A table address passed in from the HOB is not actually present in memory.
    HobTableNotInstalled,
    /// There was an attempt to retrieve an out-of-bounds XSDT entry.
    /// The `length` field of the XSDT header determines the number of valid address entries.
    InvalidXsdtEntry,
    /// The notify callback for a newly installed table failed to execute correctly.
    TableNotifyFailed,
    /// The ACPI HOB is present in the HOB list, but points to a null RSDP.
    NullRsdpFromHob,
    /// The RSDP is present but points to a null XSDT.
    NullXsdt,
    /// The table was not installed properly and thus cannot be uninstalled or deleted.
    TableNotPresentInMemory,
    /// There was an attempt to install a null table pointer.
    NullTablePtr,
    /// `get_acpi_table<T>` was provided a type that does not match the type of the table at the given index.
    InvalidTableType,
    /// There was an attempt to initialize the boot services pointer after it has already been set.
    BootServicesAlreadyInitialized,
    /// There was an attempt to initialize the memory manager after it has already been set.
    MemoryManagerAlreadyInitialized,
    /// The provider instance was not initialized.
    ProviderNotInitialized,
    /// There was an attempt to index an invalid location in the XSDT.
    XsdtOverflow,
    /// There was an attempt to install an XSDT when one already exists.
    XsdtAlreadyInstalled,
    /// There was an attempt to access the XSDT before initialization.
    XsdtNotInitialized,
    /// There was an attempt to remove an XSDT entry that does not exist.
    XsdtEntryNotFound,
    /// There was an attempt to update the checksum at an invalid byte offset.
    InvalidChecksumOffset,
    /// There was an attempt to construct a table that does not match the standard ACPI layout.
    InvalidTableFormat,
    /// The FACS address exceeds the 32-bit limit.
    /// This is a Windows-specific requirement.
    FacsAddressExceeds32BitLimit,
    /// Checksum calculation failed.
    ChecksumFailed,
}

impl From<AcpiError> for efi::Status {
    fn from(err: AcpiError) -> Self {
        match err {
            AcpiError::AllocationFailed | AcpiError::FreeFailed => efi::Status::OUT_OF_RESOURCES,
            AcpiError::BootServicesAlreadyInitialized
            | AcpiError::FadtAlreadyInstalled
            | AcpiError::MemoryManagerAlreadyInitialized
            | AcpiError::XsdtAlreadyInstalled => efi::Status::ALREADY_STARTED,
            AcpiError::ChecksumFailed => efi::Status::COMPROMISED_DATA,
            AcpiError::FacsAddressExceeds32BitLimit
            | AcpiError::FacsUefiNot64BAligned
            | AcpiError::HobTableNotInstalled
            | AcpiError::InstallConfigurationTableFailed
            | AcpiError::XsdtInvalidLengthFromHob => efi::Status::UNSUPPORTED,
            AcpiError::InvalidChecksumOffset
            | AcpiError::InvalidNotifyUnregister
            | AcpiError::InvalidSignature
            | AcpiError::InvalidTableFormat
            | AcpiError::InvalidTableIndex
            | AcpiError::InvalidTableType
            | AcpiError::InvalidXsdtEntry
            | AcpiError::NullTablePtr
            | AcpiError::TableNotifyFailed
            | AcpiError::XsdtOverflow => efi::Status::INVALID_PARAMETER,
            AcpiError::InvalidTableKey
            | AcpiError::NullRsdpFromHob
            | AcpiError::NullXsdt
            | AcpiError::ProviderNotInitialized
            | AcpiError::TableNotPresentInMemory
            | AcpiError::XsdtEntryNotFound => efi::Status::NOT_FOUND,
            AcpiError::XsdtNotInitialized => efi::Status::NOT_STARTED,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_acpi_error_to_efi_status() {
        // Test a subset of ACPI errors.
        assert_eq!(efi::Status::from(AcpiError::AllocationFailed), efi::Status::OUT_OF_RESOURCES);
        assert_eq!(efi::Status::from(AcpiError::InvalidSignature), efi::Status::INVALID_PARAMETER);
        assert_eq!(efi::Status::from(AcpiError::FadtAlreadyInstalled), efi::Status::ALREADY_STARTED);
        assert_eq!(efi::Status::from(AcpiError::NullTablePtr), efi::Status::INVALID_PARAMETER);
    }
}
