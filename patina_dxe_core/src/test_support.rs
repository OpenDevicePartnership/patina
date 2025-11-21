//! DXE Core Test Support
//!
//! Code to help support testing.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::{GCD, protocols::PROTOCOL_DB};
use core::ffi::c_void;
use patina::{
    guids::ZERO,
    pi::{
        BootMode,
        dxe_services::GcdMemoryType,
        hob::{self, HobList, ResourceDescriptorV2, header},
    },
};
use patina_internal_cpu::paging::{CacheAttributeValue, PatinaPageTable};
use patina_paging::{MemoryAttributes, PtError};
use r_efi::efi;
use std::{any::Any, cell::RefCell, fs::File, io::Read, slice};

#[macro_export]
macro_rules! test_collateral {
    ($fname:expr) => {
        concat!(env!("CARGO_MANIFEST_DIR"), "/resources/test/", $fname)
    };
}

/// A global mutex that can be used for tests to synchronize on access to global state.
/// Usage model is for tests that affect or assert things against global state to acquire this mutex to ensure that
/// other tests run in parallel do not modify or interact with global state non-deterministically.
/// The test should acquire the mutex when it starts to care about or modify global state, and release it when it no
/// longer cares about global state or modifies it (typically this would be the start and end of a test case,
/// respectively).
static GLOBAL_STATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub struct MockPageTable {
    mapped: RefCell<Vec<(u64, u64, MemoryAttributes)>>,
    unmapped: RefCell<Vec<(u64, u64)>>,
    installed: RefCell<bool>,
    // Track current mappings to provide realistic query behavior
    current_mappings: RefCell<Vec<(u64, u64, MemoryAttributes)>>,
}

impl PatinaPageTable for MockPageTable {
    fn map_memory_region(&mut self, base: u64, len: u64, attrs: MemoryAttributes) -> Result<(), PtError> {
        self.mapped.borrow_mut().push((base, len, attrs));

        // Update current mappings - remove any overlapping regions first
        let mut current = self.current_mappings.borrow_mut();
        current.retain(|(existing_base, existing_len, _)| {
            let existing_end = existing_base + existing_len;
            let new_end = base + len;
            // Keep if no overlap
            !(base < existing_end && new_end > *existing_base)
        });
        // Add new mapping
        current.push((base, len, attrs));
        Ok(())
    }

    fn unmap_memory_region(&mut self, base: u64, len: u64) -> Result<(), PtError> {
        self.unmapped.borrow_mut().push((base, len));

        // Remove from current mappings
        let mut current = self.current_mappings.borrow_mut();
        current.retain(|(existing_base, existing_len, _)| {
            let existing_end = existing_base + existing_len;
            let new_end = base + len;
            // Keep if no overlap
            !(base < existing_end && new_end > *existing_base)
        });
        Ok(())
    }

    fn query_memory_region(&self, base: u64, len: u64) -> Result<MemoryAttributes, (PtError, CacheAttributeValue)> {
        let current = self.current_mappings.borrow();
        let end = base + len;

        // Find a mapping that covers the requested range
        for (mapped_base, mapped_len, attrs) in current.iter() {
            let mapped_end = mapped_base + mapped_len;
            if *mapped_base <= base && end <= mapped_end {
                return Ok(*attrs);
            }
        }

        // No mapping found - return NoMapping error with empty cache attributes
        Err((PtError::NoMapping, CacheAttributeValue::Unmapped))
    }
    fn install_page_table(&mut self) -> Result<(), PtError> {
        *self.installed.borrow_mut() = true;
        Ok(())
    }
    fn dump_page_tables(&self, _address: u64, _size: u64) -> Result<(), PtError> {
        // No-op for testing
        Ok(())
    }
}

unsafe impl Send for MockPageTable {}
unsafe impl Sync for MockPageTable {}

impl Default for MockPageTable {
    fn default() -> Self {
        Self::new()
    }
}

impl MockPageTable {
    pub fn get_mapped_regions(&self) -> Vec<(u64, u64, MemoryAttributes)> {
        self.mapped.borrow().clone()
    }

    pub fn get_unmapped_regions(&self) -> Vec<(u64, u64)> {
        self.unmapped.borrow().clone()
    }

    pub fn get_current_mappings(&self) -> Vec<(u64, u64, MemoryAttributes)> {
        self.current_mappings.borrow().clone()
    }

    pub fn new() -> Self {
        Self {
            mapped: RefCell::new(Vec::new()),
            unmapped: RefCell::new(Vec::new()),
            installed: RefCell::new(false),
            current_mappings: RefCell::new(Vec::new()),
        }
    }
}

pub struct MockPageTableWrapper {
    inner: std::rc::Rc<std::cell::RefCell<MockPageTable>>,
}

impl MockPageTableWrapper {
    pub fn new(inner: std::rc::Rc<std::cell::RefCell<MockPageTable>>) -> Self {
        Self { inner }
    }
}

impl PatinaPageTable for MockPageTableWrapper {
    fn map_memory_region(&mut self, base: u64, len: u64, attrs: MemoryAttributes) -> Result<(), PtError> {
        self.inner.borrow_mut().map_memory_region(base, len, attrs)
    }

    fn unmap_memory_region(&mut self, base: u64, len: u64) -> Result<(), PtError> {
        self.inner.borrow_mut().unmap_memory_region(base, len)
    }

    fn query_memory_region(&self, base: u64, len: u64) -> Result<MemoryAttributes, (PtError, CacheAttributeValue)> {
        self.inner.borrow().query_memory_region(base, len)
    }

    fn install_page_table(&mut self) -> Result<(), PtError> {
        self.inner.borrow_mut().install_page_table()
    }

    fn dump_page_tables(&self, address: u64, size: u64) -> Result<(), PtError> {
        self.inner.borrow().dump_page_tables(address, size)
    }
}

/// All tests should run from inside this.
pub(crate) fn with_global_lock<F: Fn() + std::panic::RefUnwindSafe>(f: F) -> Result<(), Box<dyn Any + Send>> {
    let _guard = GLOBAL_STATE_TEST_LOCK.lock().unwrap();
    std::panic::catch_unwind(|| {
        f();
    })
}

/// Allocates a chunk of memory of the specified size from the system allocator.
///
/// ## Safety
/// This function is intended for test code only. The caller must ensure that the size is valid
/// for allocation.
pub(crate) unsafe fn get_memory(size: usize) -> &'static mut [u8] {
    // SAFETY: Test code - allocates memory from the system allocator with UEFI page alignment.
    // The returned slice is intentionally leaked for test simplicity and valid for 'static lifetime.
    let addr = unsafe { alloc::alloc::alloc(alloc::alloc::Layout::from_size_align(size, 0x1000).unwrap()) };
    // SAFETY: The allocated pointer is valid for `size` bytes and properly aligned.
    unsafe { core::slice::from_raw_parts_mut(addr, size) }
}

// default GCD allocation.
const TEST_GCD_MEM_SIZE: usize = 0x1000000;

/// Reset the GCD with a default chunk of memory from the system allocator. This will ensure that the GCD is able
/// to support interactions with other core subsystem (e.g. allocators).
///
/// Note: for simplicity, this implementation intentionally leaks the memory allocated for the GCD. Expectation is
/// that this should be called few enough times in testing so that this leak does not cause problems.
///
/// ## Safety
/// This function modifies global state. It should be called with the test lock held to ensure
/// that no other tests are concurrently modifying the GCD.
pub(crate) unsafe fn init_test_gcd(size: Option<usize>) {
    let size = size.unwrap_or(TEST_GCD_MEM_SIZE);
    // SAFETY: Allocates memory from the system allocator with UEFI page alignment for GCD memory blocks.
    let addr = unsafe { alloc::alloc::alloc(alloc::alloc::Layout::from_size_align(size, 0x1000).unwrap()) };
    // SAFETY: Resetting the global GCD state in test context - called with test lock held.
    unsafe { GCD.reset() };
    GCD.init(48, 16);
    // SAFETY: Initializing GCD memory blocks with allocated memory.
    unsafe {
        GCD.init_memory_blocks(
            GcdMemoryType::SystemMemory,
            addr as usize,
            TEST_GCD_MEM_SIZE,
            efi::MEMORY_UC
                | efi::MEMORY_WC
                | efi::MEMORY_WT
                | efi::MEMORY_WB
                | efi::MEMORY_WP
                | efi::MEMORY_RP
                | efi::MEMORY_XP
                | efi::MEMORY_RO,
        )
        .unwrap()
    };
}

/// Resets the ALLOCATOR map to empty and resets the static allocators
///
/// ## Safety
/// This function modifies global state. It should be called with the test lock held to ensure
/// that no other tests are concurrently modifying the allocator state.
pub(crate) unsafe fn reset_allocators() {
    // SAFETY: Test code - resetting the global allocator state with the test lock held.
    unsafe { crate::allocator::reset_allocators() }
}

/// Reset and re-initialize the protocol database to default empty state.
///
/// ## Safety
/// This function modifies global state. It should be called with the test lock held to ensure
/// that no other tests are concurrently modifying the protocol database.
pub(crate) unsafe fn init_test_protocol_db() {
    // SAFETY: Test code - resetting the global protocol database state with the test lock held.
    unsafe { PROTOCOL_DB.reset() };
    PROTOCOL_DB.init_protocol_db();
}

/// Reset the dispatcher context to default empty state.
pub(crate) fn reset_dispatcher_context() {
    crate::dispatcher::reset_dispatcher_context_for_tests();
}

pub(crate) fn build_test_hob_list(mem_size: u64) -> *const c_void {
    // SAFETY: Test code - allocates memory for the test HOB list.
    let mem = unsafe { get_memory(mem_size as usize) };
    let mem_base = mem.as_mut_ptr() as u64;

    // Build a test HOB list that describes memory layout as follows:
    //
    // Base:         offset 0                   ************
    // HobList:      offset base+0              HOBS
    // Empty:        offset base+HobListSize    N/A
    // SystemMemory  offset base+0xE0000        SystemMemory (resource_descriptor1)
    // Reserved      offset base+0xF0000        Untested SystemMemory (resource_descriptor2)
    // FreeMemory    offset base+0x100000       FreeMemory (phit)
    // End           offset base+0x200000       ************
    //
    // The test HOB list will also include resource descriptor hobs that describe MMIO/IO as follows:
    // MMIO at 0x10000000 size 0x1000000 (resource_descriptor3)
    // FirmwareDevice at 0x11000000 size 0x1000000 (resource_descriptor4)
    // Reserved at 0x12000000 size 0x1000000 (resource_descriptor5)
    // Legacy I/O at 0x1000 size 0xF000 (resource_descriptor6)
    // Reserved Legacy I/O at 0x0000 size 0x1000 (resource_descriptor7)
    //
    // The test HOB list will also include resource allocation hobs that describe allocations as follows:
    // A Memory Allocation Hob for each memory type. This will be placed in the SystemMemory region at base+0xE0000 as
    // 4K allocations. There is also a Memory Allocation Hob for MMIO space at 0x10000000 for 0x2000 bytes.
    // A Firmware Volume HOB located in the FirmwareDevice region at 0x10000000
    //
    let phit = hob::PhaseHandoffInformationTable {
        header: header::Hob {
            r#type: hob::HANDOFF,
            length: core::mem::size_of::<hob::PhaseHandoffInformationTable>() as u16,
            reserved: 0x00000000,
        },
        version: 0x0009,
        boot_mode: BootMode::BootAssumingNoConfigurationChanges,
        memory_top: mem_base + mem_size,
        memory_bottom: mem_base,
        free_memory_top: mem_base + mem_size,
        free_memory_bottom: mem_base + 0x100000,
        end_of_hob_list: mem_base
            + core::mem::size_of::<hob::PhaseHandoffInformationTable>() as u64
            + core::mem::size_of::<hob::Cpu>() as u64
            + (core::mem::size_of::<ResourceDescriptorV2>() as u64) * 7
            + core::mem::size_of::<header::Hob>() as u64,
    };

    let cpu = hob::Cpu {
        header: header::Hob { r#type: hob::CPU, length: core::mem::size_of::<hob::Cpu>() as u16, reserved: 0 },
        size_of_memory_space: 48,
        size_of_io_space: 16,
        reserved: Default::default(),
    };

    let resource_descriptor1 = ResourceDescriptorV2 {
        v1: hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR2,
                length: core::mem::size_of::<ResourceDescriptorV2>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
            resource_attribute: hob::TESTED_MEMORY_ATTRIBUTES | hob::EFI_RESOURCE_ATTRIBUTE_WRITE_BACK_CACHEABLE,
            physical_start: mem_base + 0xE0000,
            resource_length: 0x10000,
        },
        attributes: efi::MEMORY_WB,
    };

    let resource_descriptor2 = ResourceDescriptorV2 {
        v1: hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR2,
                length: core::mem::size_of::<ResourceDescriptorV2>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
            resource_attribute: hob::INITIALIZED_MEMORY_ATTRIBUTES | hob::EFI_RESOURCE_ATTRIBUTE_WRITE_BACK_CACHEABLE,
            physical_start: mem_base + 0xF0000,
            resource_length: 0x10000,
        },
        attributes: efi::MEMORY_WB,
    };

    let resource_descriptor3 = ResourceDescriptorV2 {
        v1: hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR2,
                length: core::mem::size_of::<ResourceDescriptorV2>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_MEMORY_MAPPED_IO,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT
                | hob::EFI_RESOURCE_ATTRIBUTE_INITIALIZED
                | hob::EFI_RESOURCE_ATTRIBUTE_UNCACHEABLE,
            physical_start: 0x10000000,
            resource_length: 0x1000000,
        },
        attributes: efi::MEMORY_UC,
    };

    let resource_descriptor4 = ResourceDescriptorV2 {
        v1: hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR2,
                length: core::mem::size_of::<ResourceDescriptorV2>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_FIRMWARE_DEVICE,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT
                | hob::EFI_RESOURCE_ATTRIBUTE_INITIALIZED
                | hob::EFI_RESOURCE_ATTRIBUTE_UNCACHEABLE,
            physical_start: 0x11000000,
            resource_length: 0x1000000,
        },
        attributes: efi::MEMORY_UC,
    };

    let resource_descriptor5 = ResourceDescriptorV2 {
        v1: hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR2,
                length: core::mem::size_of::<ResourceDescriptorV2>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_MEMORY_RESERVED,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT
                | hob::EFI_RESOURCE_ATTRIBUTE_INITIALIZED
                | hob::EFI_RESOURCE_ATTRIBUTE_WRITE_BACK_CACHEABLE,
            physical_start: 0x12000000,
            resource_length: 0x1000000,
        },
        attributes: efi::MEMORY_WB,
    };

    let resource_descriptor6 = ResourceDescriptorV2 {
        v1: hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR2,
                length: core::mem::size_of::<ResourceDescriptorV2>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_IO,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT | hob::EFI_RESOURCE_ATTRIBUTE_INITIALIZED,
            physical_start: 0x1000,
            resource_length: 0xF000,
        },
        attributes: 0, // Cacheability not applicable for I/O space
    };

    let resource_descriptor7 = ResourceDescriptorV2 {
        v1: hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR2,
                length: core::mem::size_of::<ResourceDescriptorV2>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_IO_RESERVED,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT,
            physical_start: 0x0000,
            resource_length: 0x1000,
        },
        attributes: 0, // Cacheability not applicable for reserved I/O space
    };

    let mut allocation_hob_template = hob::MemoryAllocation {
        header: header::Hob {
            r#type: hob::MEMORY_ALLOCATION,
            length: core::mem::size_of::<hob::MemoryAllocation>() as u16,
            reserved: 0x00000000,
        },
        alloc_descriptor: header::MemoryAllocation {
            name: ZERO,
            memory_base_address: 0,
            memory_length: 0x1000,
            memory_type: efi::RESERVED_MEMORY_TYPE,
            reserved: Default::default(),
        },
    };

    let firmware_volume_hob = hob::FirmwareVolume {
        header: header::Hob {
            r#type: hob::FV,
            length: core::mem::size_of::<hob::FirmwareVolume>() as u16,
            reserved: 0x00000000,
        },
        base_address: resource_descriptor4.v1.physical_start,
        length: 0x80000,
    };

    let end =
        header::Hob { r#type: hob::END_OF_HOB_LIST, length: core::mem::size_of::<header::Hob>() as u16, reserved: 0 };

    // SAFETY: Test code - constructing a test HOB list by copying structures into allocated memory.
    // The memory is allocated in this function.
    unsafe {
        let mut cursor = mem.as_mut_ptr();

        //PHIT HOB
        core::ptr::copy(&phit, cursor as *mut hob::PhaseHandoffInformationTable, 1);
        cursor = cursor.offset(phit.header.length as isize);

        //CPU HOB
        core::ptr::copy(&cpu, cursor as *mut hob::Cpu, 1);
        cursor = cursor.offset(cpu.header.length as isize);

        //resource descriptor HOBs - all V2 to enable proper migration
        core::ptr::copy(&resource_descriptor1, cursor as *mut ResourceDescriptorV2, 1);
        cursor = cursor.offset(resource_descriptor1.v1.header.length as isize);

        core::ptr::copy(&resource_descriptor2, cursor as *mut ResourceDescriptorV2, 1);
        cursor = cursor.offset(resource_descriptor2.v1.header.length as isize);

        core::ptr::copy(&resource_descriptor3, cursor as *mut ResourceDescriptorV2, 1);
        cursor = cursor.offset(resource_descriptor3.v1.header.length as isize);

        core::ptr::copy(&resource_descriptor4, cursor as *mut ResourceDescriptorV2, 1);
        cursor = cursor.offset(resource_descriptor4.v1.header.length as isize);

        core::ptr::copy(&resource_descriptor5, cursor as *mut ResourceDescriptorV2, 1);
        cursor = cursor.offset(resource_descriptor5.v1.header.length as isize);

        core::ptr::copy(&resource_descriptor6, cursor as *mut ResourceDescriptorV2, 1);
        cursor = cursor.offset(resource_descriptor6.v1.header.length as isize);

        core::ptr::copy(&resource_descriptor7, cursor as *mut ResourceDescriptorV2, 1);
        cursor = cursor.offset(resource_descriptor7.v1.header.length as isize);

        //memory allocation HOBs.
        for (idx, memory_type) in [
            efi::RESERVED_MEMORY_TYPE,
            efi::LOADER_CODE,
            efi::LOADER_DATA,
            efi::BOOT_SERVICES_CODE,
            efi::BOOT_SERVICES_DATA,
            efi::RUNTIME_SERVICES_CODE,
            efi::RUNTIME_SERVICES_DATA,
            efi::ACPI_RECLAIM_MEMORY,
            efi::ACPI_MEMORY_NVS,
            efi::PAL_CODE,
        ]
        .iter()
        .enumerate()
        {
            allocation_hob_template.alloc_descriptor.memory_base_address =
                resource_descriptor1.v1.physical_start + idx as u64 * 0x1000;
            allocation_hob_template.alloc_descriptor.memory_type = *memory_type;

            core::ptr::copy(&allocation_hob_template, cursor as *mut hob::MemoryAllocation, 1);
            cursor = cursor.offset(allocation_hob_template.header.length as isize);
        }

        // memory allocation HOB for MMIO space
        allocation_hob_template.alloc_descriptor.memory_base_address = resource_descriptor3.v1.physical_start;
        allocation_hob_template.alloc_descriptor.memory_length = 0x2000;
        allocation_hob_template.alloc_descriptor.memory_type = efi::MEMORY_MAPPED_IO;
        core::ptr::copy(&allocation_hob_template, cursor as *mut hob::MemoryAllocation, 1);
        cursor = cursor.offset(allocation_hob_template.header.length as isize);

        //FV HOB.
        core::ptr::copy(&firmware_volume_hob, cursor as *mut hob::FirmwareVolume, 1);
        cursor = cursor.offset(firmware_volume_hob.header.length as isize);

        core::ptr::copy(&end, cursor as *mut header::Hob, 1);
    }
    mem.as_ptr() as *const c_void
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::{
        c_void,
        test_support::{BootMode, get_memory, header, hob},
    };
    use patina::{
        guids,
        pi::hob::{Hob::MemoryAllocationModule, ResourceDescriptorV2},
    };

    // Compact Hoblist with DXE core Alloction hob. Use this when DXE core hob is required.
    pub(crate) fn build_test_hob_list_compact(mem_size: u64) -> *const c_void {
        // SAFETY: Test code - allocates memory for compact test HOB list. mem_size is large
        // enough to hold all HOB structures in the given unit test infrastructure.
        let mem = unsafe { get_memory(mem_size as usize) };
        let mem_base = mem.as_mut_ptr() as u64;

        // Build a test HOB list that describes memory

        let phit = hob::PhaseHandoffInformationTable {
            header: header::Hob {
                r#type: hob::HANDOFF,
                length: core::mem::size_of::<hob::PhaseHandoffInformationTable>() as u16,
                reserved: 0x00000000,
            },
            version: 0x0009,
            boot_mode: BootMode::BootAssumingNoConfigurationChanges,
            memory_top: mem_base + mem_size,
            memory_bottom: mem_base,
            free_memory_top: mem_base + mem_size,
            free_memory_bottom: mem_base + 0x100000,
            end_of_hob_list: mem_base
                + core::mem::size_of::<hob::PhaseHandoffInformationTable>() as u64
                + core::mem::size_of::<hob::Cpu>() as u64
                + core::mem::size_of::<ResourceDescriptorV2>() as u64  // Only 1 V2 system memory HOB
                + core::mem::size_of::<header::Hob>() as u64,
        };

        let cpu = hob::Cpu {
            header: header::Hob { r#type: hob::CPU, length: core::mem::size_of::<hob::Cpu>() as u16, reserved: 0 },
            size_of_memory_space: 48,
            size_of_io_space: 16,
            reserved: Default::default(),
        };

        let resource_descriptor1 = ResourceDescriptorV2 {
            v1: hob::ResourceDescriptor {
                header: header::Hob {
                    r#type: hob::RESOURCE_DESCRIPTOR2,
                    length: core::mem::size_of::<ResourceDescriptorV2>() as u16,
                    reserved: 0x00000000,
                },
                owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
                resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
                resource_attribute: hob::TESTED_MEMORY_ATTRIBUTES,
                physical_start: mem_base + 0xE0000,
                resource_length: 0x10000,
            },
            attributes: efi::MEMORY_WB,
        };

        let mut allocation_hob_template: hob::MemoryAllocationModule = hob::MemoryAllocationModule {
            header: header::Hob {
                r#type: hob::MEMORY_ALLOCATION,
                length: core::mem::size_of::<hob::MemoryAllocationModule>() as u16,
                reserved: 0x00000000,
            },
            alloc_descriptor: header::MemoryAllocation {
                name: ZERO,
                memory_base_address: 0,
                memory_length: 0x1000,
                memory_type: efi::LOADER_CODE,
                reserved: Default::default(),
            },
            module_name: guids::DXE_CORE,
            entry_point: 0,
        };

        let end = header::Hob {
            r#type: hob::END_OF_HOB_LIST,
            length: core::mem::size_of::<header::Hob>() as u16,
            reserved: 0,
        };

        // SAFETY: Test code - constructing a compact test HOB list by copying structures into allocated memory.
        // The memory is valid and large enough to hold all HOB structures in the given unit test infrastructure
        // implementation.
        unsafe {
            let mut cursor = mem.as_mut_ptr();

            // PHIT HOB
            core::ptr::copy(&phit, cursor as *mut hob::PhaseHandoffInformationTable, 1);
            cursor = cursor.offset(phit.header.length as isize);

            // CPU HOB
            core::ptr::copy(&cpu, cursor as *mut hob::Cpu, 1);
            cursor = cursor.offset(cpu.header.length as isize);

            // Resource descriptor HOB
            core::ptr::copy(&resource_descriptor1, cursor as *mut ResourceDescriptorV2, 1);
            cursor = cursor.offset(resource_descriptor1.v1.header.length as isize);

            // Memory allocation HOBs.
            for (idx, memory_type) in [
                efi::RESERVED_MEMORY_TYPE,
                efi::LOADER_CODE,
                efi::LOADER_DATA,
                efi::BOOT_SERVICES_CODE,
                efi::BOOT_SERVICES_DATA,
                efi::RUNTIME_SERVICES_CODE,
                efi::RUNTIME_SERVICES_DATA,
                efi::ACPI_RECLAIM_MEMORY,
                efi::ACPI_MEMORY_NVS,
                efi::PAL_CODE,
            ]
            .iter()
            .enumerate()
            {
                allocation_hob_template.alloc_descriptor.memory_base_address =
                    resource_descriptor1.v1.physical_start + idx as u64 * 0x1000;
                allocation_hob_template.alloc_descriptor.memory_type = *memory_type;
                allocation_hob_template.module_name = guids::DXE_CORE;

                core::ptr::copy(&allocation_hob_template, cursor as *mut hob::MemoryAllocationModule, 1);
                cursor = cursor.offset(allocation_hob_template.header.length as isize);
            }

            core::ptr::copy(&end, cursor as *mut header::Hob, 1);
        }
        mem.as_ptr() as *const c_void
    }

    //
    // Fill in Dxe Image in to hoblist.
    // Usage - fill_file_buffer_in_memory_allocation_module(&hob_list).unwrap();
    //
    pub(crate) fn fill_file_buffer_in_memory_allocation_module(hob_list: &HobList) -> Result<(), &'static str> {
        let mut file = File::open(test_collateral!("RustImageTestDxe.efi")).expect("failed to open test file.");
        let mut image: Vec<u8> = Vec::new();
        file.read_to_end(&mut image).expect("failed to read test file");

        // Locate the MemoryAllocationModule HOB for the DXE Core
        let dxe_core_hob = hob_list
            .iter()
            .find_map(|hob| match hob {
                MemoryAllocationModule(module) if module.module_name == guids::DXE_CORE => Some(module),
                _ => None,
            })
            .ok_or("DXE Core MemoryAllocationModule HOB not found")?;

        let memory_base_address = dxe_core_hob.alloc_descriptor.memory_base_address;
        let memory_length = dxe_core_hob.alloc_descriptor.memory_length;

        // Assert that the memory base address and length are valid
        assert!(memory_base_address > 0, "Memory base address is invalid (0).");
        assert!(memory_length > 0, "Memory length is invalid (0).");

        // Get the file size
        let file_size = file.metadata().map_err(|_| "Failed to get file metadata")?.len();

        if file_size > (memory_length as usize).try_into().unwrap() {
            return Err("File contents exceed allocated memory length");
        }

        // SAFETY: Test code - writing file contents into memory region specified by the DXE core HOB.
        // The memory region is valid and sized according to the HOB. The file size has been checked to
        // verify that it fits within the allocated memory length.
        unsafe {
            let memory_slice = slice::from_raw_parts_mut(memory_base_address as *mut u8, memory_length as usize);
            let file_size = file_size as usize; // Convert file_size to usize
            memory_slice[..file_size].copy_from_slice(&image);
            assert_eq!(
                &memory_slice[..file_size], // Use file_size as usize
                &image[..],
                "File contents were not correctly written to memory."
            );
        }

        Ok(())
    }

    #[test]
    fn test_build_test_hob_list_compact() {
        // Note: The mem_size specified here must  be large enough to hold all HOB structures in this test
        // infrastructure.
        let physical_hob_list = build_test_hob_list_compact(0x2000000);
        let mut hob_list = HobList::default();
        hob_list.discover_hobs(physical_hob_list);
        fill_file_buffer_in_memory_allocation_module(&hob_list).unwrap();
    }
}
