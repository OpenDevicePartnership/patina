use core::ops::{BitOr, BitOrAssign};

use r_efi::efi;

use super::{boxed::BootServicesBox, BootServices};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AllocType {
    AnyPage,
    MaxAddress(usize),
    Address(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct MemoryType(u32);

impl MemoryType {
    pub const RESERVED_MEMORY_TYPE: MemoryType = MemoryType(efi::RESERVED_MEMORY_TYPE);
    pub const LOADER_CODE: MemoryType = MemoryType(efi::LOADER_CODE);
    pub const LOADER_DATA: MemoryType = MemoryType(efi::LOADER_DATA);
    pub const BOOT_SERVICES_CODE: MemoryType = MemoryType(efi::BOOT_SERVICES_CODE);
    pub const BOOT_SERVICES_DATA: MemoryType = MemoryType(efi::BOOT_SERVICES_DATA);
    pub const RUNTIME_SERVICES_CODE: MemoryType = MemoryType(efi::RUNTIME_SERVICES_CODE);
    pub const RUNTIME_SERVICES_DATA: MemoryType = MemoryType(efi::RUNTIME_SERVICES_DATA);
    pub const CONVENTIONAL_MEMORY: MemoryType = MemoryType(efi::CONVENTIONAL_MEMORY);
    pub const UNUSABLE_MEMORY: MemoryType = MemoryType(efi::UNUSABLE_MEMORY);
    pub const ACPI_RECLAIM_MEMORY: MemoryType = MemoryType(efi::ACPI_RECLAIM_MEMORY);
    pub const ACPI_MEMORY_NVS: MemoryType = MemoryType(efi::ACPI_MEMORY_NVS);
    pub const MEMORY_MAPPED_IO: MemoryType = MemoryType(efi::MEMORY_MAPPED_IO);
    pub const MEMORY_MAPPED_IO_PORT_SPACE: MemoryType = MemoryType(efi::MEMORY_MAPPED_IO_PORT_SPACE);
    pub const PAL_CODE: MemoryType = MemoryType(efi::PAL_CODE);
    pub const PERSISTENT_MEMORY: MemoryType = MemoryType(efi::PERSISTENT_MEMORY);
    pub const UNACCEPTED_MEMORY_TYPE: MemoryType = MemoryType(efi::UNACCEPTED_MEMORY_TYPE);
}

impl From<MemoryType> for u32 {
    fn from(val: MemoryType) -> Self {
        val.0
    }
}

#[derive(Debug)]
pub struct MemoryMap<'a, B: BootServices + ?Sized> {
    pub descriptors: BootServicesBox<'a, [efi::MemoryDescriptor], B>,
    pub map_key: usize,
    pub descriptor_version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryAttribute(u64);

impl MemoryAttribute {
    pub const UC: MemoryAttribute = MemoryAttribute(efi::MEMORY_UC);
    pub const WC: MemoryAttribute = MemoryAttribute(efi::MEMORY_WC);
    pub const WT: MemoryAttribute = MemoryAttribute(efi::MEMORY_WT);
    pub const WB: MemoryAttribute = MemoryAttribute(efi::MEMORY_WB);
    pub const UCE: MemoryAttribute = MemoryAttribute(efi::MEMORY_UCE);
    pub const WP: MemoryAttribute = MemoryAttribute(efi::MEMORY_WP);
    pub const RP: MemoryAttribute = MemoryAttribute(efi::MEMORY_RP);
    pub const XP: MemoryAttribute = MemoryAttribute(efi::MEMORY_XP);
    pub const NV: MemoryAttribute = MemoryAttribute(efi::MEMORY_NV);
    pub const MORE_RELIABLE: MemoryAttribute = MemoryAttribute(efi::MEMORY_MORE_RELIABLE);
    pub const RO: MemoryAttribute = MemoryAttribute(efi::MEMORY_RO);
    pub const SP: MemoryAttribute = MemoryAttribute(efi::MEMORY_SP);
    pub const CPU_CRYPTO: MemoryAttribute = MemoryAttribute(efi::MEMORY_CPU_CRYPTO);
    pub const RUNTIME: MemoryAttribute = MemoryAttribute(efi::MEMORY_RUNTIME);
    pub const ISA_VALID: MemoryAttribute = MemoryAttribute(efi::MEMORY_ISA_VALID);
    pub const ISA_MASK: MemoryAttribute = MemoryAttribute(efi::MEMORY_ISA_MASK);
}

impl BitOr for MemoryAttribute {
    type Output = MemoryAttribute;

    fn bitor(self, rhs: Self) -> Self::Output {
        MemoryAttribute(self.0 | rhs.0)
    }
}

impl BitOrAssign for MemoryAttribute {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0
    }
}

impl From<AllocType> for efi::AllocateType {
    fn from(val: AllocType) -> Self {
        match val {
            AllocType::AnyPage => efi::ALLOCATE_ANY_PAGES,
            AllocType::MaxAddress(_) => efi::ALLOCATE_MAX_ADDRESS,
            AllocType::Address(_) => efi::ALLOCATE_ADDRESS,
        }
    }
}

impl From<MemoryAttribute> for u64 {
    fn from(val: MemoryAttribute) -> Self {
        val.0
    }
}
