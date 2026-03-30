//! Hand-Off Block List (HOB)
//!
//! The HOB list is a contiguous list of HOB structures, each with a common header
//! followed by type-specific data. Typically, the PEI Foundation creates and manages
//! the HOB list during the PEI phase, and it is passed to the DXE Foundation
//! during the PEI-to-DXE handoff.
//!
//! Based on the UEFI Platform Initialization Specification Volume III.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::pi::hob::{
    CPU, Capsule, Cpu, END_OF_HOB_LIST, FV, FV2, FV3, FirmwareVolume, FirmwareVolume2, FirmwareVolume3, GUID_EXTENSION,
    GuidHob, HANDOFF, Hob, HobTrait, MEMORY_ALLOCATION, MemoryAllocation, MemoryAllocationModule,
    PhaseHandoffInformationTable, RESOURCE_DESCRIPTOR, RESOURCE_DESCRIPTOR2, ResourceDescriptor, ResourceDescriptorV2,
    UEFI_CAPSULE, header,
};
use core::{ffi::c_void, mem, slice};

use indoc::indoc;

use crate::base::{align_down, align_up};
use core::fmt;

// Expectation is someone will provide alloc
use alloc::{boxed::Box, vec::Vec};

/// Represents a HOB list.
///
/// This is a parsed Rust representation of the HOB list that provides better type safety and ergonomics but does not
/// have binary compatibility with the original PI Spec HOB list structure.
pub struct HobList<'a>(Vec<Hob<'a>>);

impl Default for HobList<'_> {
    fn default() -> Self {
        HobList::new()
    }
}

impl<'a> HobList<'a> {
    /// Instantiates a Hoblist.
    pub const fn new() -> Self {
        HobList(Vec::new())
    }

    /// Implements iter for Hoblist.
    ///
    /// # Example(s)
    ///
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///     // example discovering and adding hobs to a hob list
    ///     let mut the_hob_list = HobList::default();
    ///     the_hob_list.discover_hobs(hob_list);
    ///
    ///     for hob in the_hob_list.iter() {
    ///         // ... do something with the hob(s)
    ///     }
    /// }
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = &Hob<'_>> {
        self.0.iter()
    }

    /// Returns a mutable pointer to the underlying data.
    ///
    /// # Example(s)
    ///
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///     // example discovering and adding hobs to a hob list
    ///     let mut the_hob_list = HobList::default();
    ///     the_hob_list.discover_hobs(hob_list);
    ///
    ///     let ptr: *mut c_void = the_hob_list.as_mut_ptr();
    ///     // ... do something with the pointer
    /// }
    /// ```
    pub fn as_mut_ptr<T>(&mut self) -> *mut T {
        self.0.as_mut_ptr() as *mut T
    }

    /// Returns the size of the Hoblist in bytes.
    ///
    /// # Example(s)
    ///
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///     // example discovering and adding hobs to a hob list
    ///     let mut the_hob_list = HobList::default();
    ///     the_hob_list.discover_hobs(hob_list);
    ///
    ///     let length = the_hob_list.size();
    ///     println!("size_of_hobs: {:?}", length);
    /// }
    pub fn size(&self) -> usize {
        let mut size_of_hobs = 0;

        for hob in self.iter() {
            size_of_hobs += hob.size()
        }

        size_of_hobs
    }

    /// Implements len for Hoblist.
    /// Returns the number of hobs in the list.
    ///
    /// # Example(s)
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///    // example discovering and adding hobs to a hob list
    ///    let mut the_hob_list = HobList::default();
    ///    the_hob_list.discover_hobs(hob_list);
    ///
    ///    let length = the_hob_list.len();
    ///    println!("length_of_hobs: {:?}", length);
    /// }
    /// ```
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Implements is_empty for Hoblist.
    /// Returns true if the list is empty.
    ///
    /// # Example(s)
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///    // example discovering and adding hobs to a hob list
    ///    let mut the_hob_list = HobList::default();
    ///    the_hob_list.discover_hobs(hob_list);
    ///
    ///    let is_empty = the_hob_list.is_empty();
    ///    println!("is_empty: {:?}", is_empty);
    /// }
    /// ```
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Implements push for Hoblist.
    ///
    /// Parameters:
    /// * hob: Hob<'a> - the hob to add to the list
    ///
    /// # Example(s)
    /// ```no_run
    /// use core::{ffi::c_void, mem::size_of};
    /// use patina::pi::hob::{HobList, Hob, header, FirmwareVolume, FV};
    ///
    /// fn example(hob_list: *const c_void) {
    ///   // example discovering and adding hobs to a hob list
    ///   let mut the_hob_list = HobList::default();
    ///   the_hob_list.discover_hobs(hob_list);
    ///
    ///   // example pushing a hob onto the list
    ///   let header = header::Hob {
    ///       r#type: FV,
    ///       length: size_of::<FirmwareVolume>() as u16,
    ///       reserved: 0,
    ///   };
    ///
    ///   let firmware_volume = FirmwareVolume {
    ///       header,
    ///       base_address: 0,
    ///       length: 0x0123456789abcdef,
    ///   };
    ///
    ///   let hob = Hob::FirmwareVolume(&firmware_volume);
    ///   the_hob_list.push(hob);
    /// }
    /// ```
    pub fn push(&mut self, hob: Hob<'a>) {
        let cloned_hob = hob.clone();
        self.0.push(cloned_hob);
    }

    /// Discovers hobs from a C style void* and adds them to a rust structure.
    ///
    /// # Example(s)
    ///
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///     // example discovering and adding hobs to a hob list
    ///     let mut the_hob_list = HobList::default();
    ///     the_hob_list.discover_hobs(hob_list);
    /// }
    /// ```
    pub fn discover_hobs(&mut self, hob_list: *const c_void) {
        const NOT_NULL: &str = "Ptr should not be NULL";
        fn assert_hob_size<T>(hob: &header::Hob) {
            let hob_len = hob.length as usize;
            let hob_size = mem::size_of::<T>();
            assert_eq!(
                hob_len, hob_size,
                "Trying to cast hob of length {hob_len} into a pointer of size {hob_size}. Hob type: {:?}",
                hob.r#type
            );
        }

        let mut hob_header: *const header::Hob = hob_list as *const header::Hob;

        loop {
            // SAFETY: hob_header points to valid HOB data provided by firmware. Each HOB has a valid header.
            let current_header = unsafe { hob_header.cast::<header::Hob>().as_ref().expect(NOT_NULL) };
            match current_header.r#type {
                HANDOFF => {
                    assert_hob_size::<PhaseHandoffInformationTable>(current_header);
                    // SAFETY: HOB type is HANDOFF and size was validated. Cast to specific HOB type is valid.
                    let phit_hob =
                        unsafe { hob_header.cast::<PhaseHandoffInformationTable>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::Handoff(phit_hob));
                }
                MEMORY_ALLOCATION => {
                    if current_header.length == mem::size_of::<MemoryAllocationModule>() as u16 {
                        // SAFETY: HOB type is MEMORY_ALLOCATION with correct size for Module variant.
                        let mem_alloc_hob =
                            unsafe { hob_header.cast::<MemoryAllocationModule>().as_ref().expect(NOT_NULL) };
                        self.0.push(Hob::MemoryAllocationModule(mem_alloc_hob));
                    } else {
                        assert_hob_size::<MemoryAllocation>(current_header);
                        // SAFETY: HOB type is MEMORY_ALLOCATION and size was validated.
                        let mem_alloc_hob = unsafe { hob_header.cast::<MemoryAllocation>().as_ref().expect(NOT_NULL) };
                        self.0.push(Hob::MemoryAllocation(mem_alloc_hob));
                    }
                }
                RESOURCE_DESCRIPTOR => {
                    assert_hob_size::<ResourceDescriptor>(current_header);
                    // SAFETY: HOB type is RESOURCE_DESCRIPTOR and size was validated.
                    let resource_desc_hob =
                        unsafe { hob_header.cast::<ResourceDescriptor>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::ResourceDescriptor(resource_desc_hob));
                }
                GUID_EXTENSION => {
                    // SAFETY: HOB type is GUID_EXTENSION. GuidHob header is valid, and data follows immediately after.
                    // Data length is calculated from HOB length minus header size. Pointer arithmetic is within HOB bounds.
                    let (guid_hob, data) = unsafe {
                        let hob = hob_header.cast::<GuidHob>().as_ref().expect(NOT_NULL);
                        let data_ptr = hob_header.byte_add(mem::size_of::<GuidHob>()) as *mut u8;
                        let data_len = hob.header.length as usize - mem::size_of::<GuidHob>();
                        (hob, slice::from_raw_parts(data_ptr, data_len))
                    };
                    self.0.push(Hob::GuidHob(guid_hob, data));
                }
                FV => {
                    assert_hob_size::<FirmwareVolume>(current_header);
                    // SAFETY: HOB type is FV and size was validated.
                    let fv_hob = unsafe { hob_header.cast::<FirmwareVolume>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::FirmwareVolume(fv_hob));
                }
                FV2 => {
                    assert_hob_size::<FirmwareVolume2>(current_header);
                    // SAFETY: HOB type is FV2 and size was validated.
                    let fv2_hob = unsafe { hob_header.cast::<FirmwareVolume2>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::FirmwareVolume2(fv2_hob));
                }
                FV3 => {
                    assert_hob_size::<FirmwareVolume3>(current_header);
                    // SAFETY: HOB type is FV3 and size was validated.
                    let fv3_hob = unsafe { hob_header.cast::<FirmwareVolume3>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::FirmwareVolume3(fv3_hob));
                }
                CPU => {
                    assert_hob_size::<Cpu>(current_header);
                    // SAFETY: HOB type is CPU and size was validated.
                    let cpu_hob = unsafe { hob_header.cast::<Cpu>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::Cpu(cpu_hob));
                }
                UEFI_CAPSULE => {
                    assert_hob_size::<Capsule>(current_header);
                    // SAFETY: HOB type is UEFI_CAPSULE and size was validated.
                    let capsule_hob = unsafe { hob_header.cast::<Capsule>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::Capsule(capsule_hob));
                }
                RESOURCE_DESCRIPTOR2 => {
                    assert_hob_size::<ResourceDescriptorV2>(current_header);
                    // SAFETY: HOB type is RESOURCE_DESCRIPTOR2 and size was validated.
                    let resource_desc_hob =
                        unsafe { hob_header.cast::<ResourceDescriptorV2>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::ResourceDescriptorV2(resource_desc_hob));
                }
                END_OF_HOB_LIST => {
                    break;
                }
                _ => {
                    self.0.push(Hob::Misc(current_header.r#type));
                }
            }
            let next_hob = hob_header as usize + current_header.length as usize;
            hob_header = next_hob as *const header::Hob;
        }
    }

    /// Relocates all HOBs in the list to new memory locations.
    ///
    /// This function creates new instances of each HOB in the list and updates the list to point to these new instances.
    ///
    /// # Example(s)
    ///
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///     // example discovering and adding hobs to a hob list
    ///     let mut the_hob_list = HobList::default();
    ///     the_hob_list.discover_hobs(hob_list);
    ///
    ///     // relocate hobs to new memory locations
    ///     the_hob_list.relocate_hobs();
    /// }
    /// ```
    pub fn relocate_hobs(&mut self) {
        for hob in self.0.iter_mut() {
            match hob {
                Hob::Handoff(hob) => *hob = Box::leak(Box::new(PhaseHandoffInformationTable::clone(hob))),
                Hob::MemoryAllocation(hob) => *hob = Box::leak(Box::new(MemoryAllocation::clone(hob))),
                Hob::MemoryAllocationModule(hob) => *hob = Box::leak(Box::new(MemoryAllocationModule::clone(hob))),
                Hob::Capsule(hob) => *hob = Box::leak(Box::new(Capsule::clone(hob))),
                Hob::ResourceDescriptor(hob) => *hob = Box::leak(Box::new(ResourceDescriptor::clone(hob))),
                Hob::GuidHob(hob, data) => {
                    *hob = Box::leak(Box::new(GuidHob::clone(hob)));
                    *data = Box::leak(data.to_vec().into_boxed_slice());
                }
                Hob::FirmwareVolume(hob) => *hob = Box::leak(Box::new(FirmwareVolume::clone(hob))),
                Hob::FirmwareVolume2(hob) => *hob = Box::leak(Box::new(FirmwareVolume2::clone(hob))),
                Hob::FirmwareVolume3(hob) => *hob = Box::leak(Box::new(FirmwareVolume3::clone(hob))),
                Hob::Cpu(hob) => *hob = Box::leak(Box::new(Cpu::clone(hob))),
                Hob::ResourceDescriptorV2(hob) => *hob = Box::leak(Box::new(ResourceDescriptorV2::clone(hob))),
                Hob::Misc(_) => (), // Data is owned in Misc (nothing to move),
            };
        }
    }
}

/// Implements IntoIterator for HobList.
///
/// Defines how it will be converted to an iterator.
impl<'a> IntoIterator for HobList<'a> {
    type Item = Hob<'a>;
    type IntoIter = <Vec<Hob<'a>> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a HobList<'a> {
    type Item = &'a Hob<'a>;
    type IntoIter = core::slice::Iter<'a, Hob<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Implements Debug for Hoblist.
///
/// Writes Hoblist debug information to stdio
///
impl fmt::Debug for HobList<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for hob in self.0.clone().into_iter() {
            match hob {
                Hob::Handoff(hob) => {
                    write!(
                        f,
                        indoc! {"
                        PHASE HANDOFF INFORMATION TABLE (PHIT) HOB
                          HOB Length: 0x{:x}
                          Version: 0x{:x}
                          Boot Mode: {}
                          Memory Bottom: 0x{:x}
                          Memory Top: 0x{:x}
                          Free Memory Bottom: 0x{:x}
                          Free Memory Top: 0x{:x}
                          End of HOB List: 0x{:x}\n"},
                        hob.header.length,
                        hob.version,
                        hob.boot_mode,
                        align_up(hob.memory_bottom, 0x1000).unwrap_or(0),
                        align_down(hob.memory_top, 0x1000).unwrap_or(0),
                        align_up(hob.free_memory_bottom, 0x1000).unwrap_or(0),
                        align_down(hob.free_memory_top, 0x1000).unwrap_or(0),
                        hob.end_of_hob_list
                    )?;
                }
                Hob::MemoryAllocation(hob) => {
                    write!(
                        f,
                        indoc! {"
                        MEMORY ALLOCATION HOB
                          HOB Length: 0x{:x}
                          Memory Base Address: 0x{:x}
                          Memory Length: 0x{:x}
                          Memory Type: {:?}\n"},
                        hob.header.length,
                        hob.alloc_descriptor.memory_base_address,
                        hob.alloc_descriptor.memory_length,
                        hob.alloc_descriptor.memory_type
                    )?;
                }
                Hob::ResourceDescriptor(hob) => {
                    write!(
                        f,
                        indoc! {"
                        RESOURCE DESCRIPTOR HOB
                          HOB Length: 0x{:x}
                          Resource Type: 0x{:x}
                          Resource Attribute Type: 0x{:x}
                          Resource Start Address: 0x{:x}
                          Resource Length: 0x{:x}\n"},
                        hob.header.length,
                        hob.resource_type,
                        hob.resource_attribute,
                        hob.physical_start,
                        hob.resource_length
                    )?;
                }
                Hob::GuidHob(hob, _data) => {
                    let (f0, f1, f2, f3, f4, &[f5, f6, f7, f8, f9, f10]) = hob.name.as_fields();
                    write!(
                        f,
                        indoc! {"
                        GUID HOB
                          Type: {:#x}
                          Length: {:#x},
                          GUID: {{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}\n"},
                        hob.header.r#type, hob.header.length, f0, f1, f2, f3, f4, f5, f6, f7, f8, f9, f10,
                    )?;
                }
                Hob::FirmwareVolume(hob) => {
                    write!(
                        f,
                        indoc! {"
                        FIRMWARE VOLUME (FV) HOB
                          HOB Length: 0x{:x}
                          Base Address: 0x{:x}
                          Length: 0x{:x}\n"},
                        hob.header.length, hob.base_address, hob.length
                    )?;
                }
                Hob::FirmwareVolume2(hob) => {
                    write!(
                        f,
                        indoc! {"
                        FIRMWARE VOLUME 2 (FV2) HOB
                          Base Address: 0x{:x}
                          Length: 0x{:x}\n"},
                        hob.base_address, hob.length
                    )?;
                }
                Hob::FirmwareVolume3(hob) => {
                    write!(
                        f,
                        indoc! {"
                        FIRMWARE VOLUME 3 (FV3) HOB
                          Base Address: 0x{:x}
                          Length: 0x{:x}\n"},
                        hob.base_address, hob.length
                    )?;
                }
                Hob::Cpu(hob) => {
                    write!(
                        f,
                        indoc! {"
                        CPU HOB
                          Memory Space Size: 0x{:x}
                          IO Space Size: 0x{:x}\n"},
                        hob.size_of_memory_space, hob.size_of_io_space
                    )?;
                }
                Hob::Capsule(hob) => {
                    write!(
                        f,
                        indoc! {"
                        CAPSULE HOB
                          Base Address: 0x{:x}
                          Length: 0x{:x}\n"},
                        hob.base_address, hob.length
                    )?;
                }
                Hob::ResourceDescriptorV2(hob) => {
                    write!(
                        f,
                        indoc! {"
                        RESOURCE DESCRIPTOR 2 HOB
                          HOB Length: 0x{:x}
                          Resource Type: 0x{:x}
                          Resource Attribute Type: 0x{:x}
                          Resource Start Address: 0x{:x}
                          Resource Length: 0x{:x}
                          Attributes: 0x{:x}\n"},
                        hob.v1.header.length,
                        hob.v1.resource_type,
                        hob.v1.resource_attribute,
                        hob.v1.physical_start,
                        hob.v1.resource_length,
                        hob.attributes
                    )?;
                }
                _ => (),
            }
        }
        write!(f, "Parsed HOBs")
    }
}

#[cfg(test)]
mod tests {
    use crate::pi::{
        hob,
        hob::{
            Capsule, Cpu, FirmwareVolume, Hob, HobTrait, MemoryAllocation, PhaseHandoffInformationTable,
            ResourceDescriptor, get_pi_hob_list_size,
            hob_list::HobList,
            tests::{
                gen_capsule, gen_cpu, gen_end_of_hoblist, gen_firmware_volume, gen_firmware_volume2,
                gen_firmware_volume3, gen_guid_hob, gen_memory_allocation, gen_memory_allocation_module,
                gen_phase_handoff_information_table, gen_resource_descriptor, gen_resource_descriptor_v2,
                guid_hob_refs,
            },
        },
    };

    use core::{
        ffi::c_void,
        mem::{drop, forget, size_of},
        ptr,
        slice::from_raw_parts,
    };

    use std::vec::Vec;

    // Converts the Hoblist to a C array.
    // # Arguments
    // * `hob_list` - A reference to the HobList.
    //
    // # Returns
    // A tuple containing a pointer to the C array and the length of the C array.
    pub fn to_c_array(hob_list: &HobList) -> (*const c_void, usize) {
        let size = hob_list.size();
        let mut c_array: Vec<u8> = Vec::with_capacity(size);

        for hob in hob_list.iter() {
            // SAFETY: Test code - creating a slice from HOB pointer for serialization.
            // All HOB variants must have contiguous backing memory where as_ptr() points to
            // the start and size() covers the remainder.
            let slice = unsafe { from_raw_parts(hob.as_ptr(), hob.size()) };
            c_array.extend_from_slice(slice);
        }

        let void_ptr = c_array.as_ptr() as *const c_void;

        // in order to not call the destructor on the Vec at the end of this function, we need to forget it
        forget(c_array);

        (void_ptr, size)
    }

    // Implements a function to manually free a C array.
    //
    // # Arguments
    // * `c_array_ptr` - A pointer to the C array.
    // * `len` - The length of the C array.
    //
    // # Safety
    //
    // The caller must ensure that the pointer and length match a Vec originally created by to_c_array.
    pub fn manually_free_c_array(c_array_ptr: *const c_void, len: usize) {
        let ptr = c_array_ptr as *mut u8;
        // SAFETY: Caller is responsible for ensuring the pointer and length are valid per the function contract.
        unsafe {
            drop(Vec::from_raw_parts(ptr, len, len));
        }
    }

    #[test]
    fn test_hoblist_empty() {
        let hoblist = HobList::new();
        assert_eq!(hoblist.len(), 0);
        assert!(hoblist.is_empty());
    }

    #[test]
    fn test_hoblist_push() {
        let mut hoblist = HobList::new();
        let resource = gen_resource_descriptor();
        hoblist.push(Hob::ResourceDescriptor(&resource));
        assert_eq!(hoblist.len(), 1);

        let firmware_volume = gen_firmware_volume();
        hoblist.push(Hob::FirmwareVolume(&firmware_volume));

        assert_eq!(hoblist.len(), 2);

        let resource_v2 = gen_resource_descriptor_v2();
        hoblist.push(Hob::ResourceDescriptorV2(&resource_v2));

        assert_eq!(hoblist.len(), 3);
    }

    #[test]
    fn test_hoblist_iterate() {
        let mut hoblist = HobList::default();
        let resource = gen_resource_descriptor();
        let firmware_volume = gen_firmware_volume();
        let firmware_volume2 = gen_firmware_volume2();
        let firmware_volume3 = gen_firmware_volume3();
        let end_of_hob_list = gen_end_of_hoblist();
        let capsule = gen_capsule();
        let guid_hob_buf = gen_guid_hob();
        let (guid_hob, guid_hob_data) = guid_hob_refs(&guid_hob_buf);
        let memory_allocation = gen_memory_allocation();
        let memory_allocation_module = gen_memory_allocation_module();

        hoblist.push(Hob::ResourceDescriptor(&resource));
        hoblist.push(Hob::FirmwareVolume(&firmware_volume));
        hoblist.push(Hob::FirmwareVolume2(&firmware_volume2));
        hoblist.push(Hob::FirmwareVolume3(&firmware_volume3));
        hoblist.push(Hob::Capsule(&capsule));
        hoblist.push(Hob::GuidHob(guid_hob, guid_hob_data));
        hoblist.push(Hob::MemoryAllocation(&memory_allocation));
        hoblist.push(Hob::MemoryAllocationModule(&memory_allocation_module));
        hoblist.push(Hob::Handoff(&end_of_hob_list));

        let mut count = 0;
        hoblist.iter().for_each(|hob| {
            match hob {
                Hob::ResourceDescriptor(resource) => {
                    assert_eq!(resource.resource_type, hob::EFI_RESOURCE_SYSTEM_MEMORY);
                }
                Hob::MemoryAllocation(memory_allocation) => {
                    assert_eq!(memory_allocation.alloc_descriptor.memory_length, 0x0123456789abcdef);
                }
                Hob::MemoryAllocationModule(memory_allocation_module) => {
                    assert_eq!(memory_allocation_module.alloc_descriptor.memory_length, 0x0123456789abcdef);
                }
                Hob::Capsule(capsule) => {
                    assert_eq!(capsule.base_address, 0);
                }
                Hob::GuidHob(guid_hob, data) => {
                    assert_eq!(guid_hob.name, r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]));
                    assert_eq!(*data, &[1_u8, 2, 3, 4, 5, 6, 7, 8]);
                }
                Hob::FirmwareVolume(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::FirmwareVolume2(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::FirmwareVolume3(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::Handoff(handoff) => {
                    assert_eq!(handoff.memory_top, 0xdeadbeef);
                }
                _ => {
                    panic!("Unexpected hob type");
                }
            }
            count += 1;
        });
        assert_eq!(count, 9);
    }

    #[test]
    fn test_hoblist_discover() {
        // generate some test hobs
        let resource = gen_resource_descriptor();
        let handoff = gen_phase_handoff_information_table();
        let firmware_volume = gen_firmware_volume();
        let firmware_volume2 = gen_firmware_volume2();
        let firmware_volume3 = gen_firmware_volume3();
        let capsule = gen_capsule();
        let guid_hob_buf = gen_guid_hob();
        let (guid_hob, guid_hob_data) = guid_hob_refs(&guid_hob_buf);
        let memory_allocation = gen_memory_allocation();
        let memory_allocation_module = gen_memory_allocation_module();
        let cpu = gen_cpu();
        let resource_v2 = gen_resource_descriptor_v2();
        let end_of_hob_list = gen_end_of_hoblist();

        // create a new hoblist
        let mut hoblist = HobList::new();

        // Push the resource descriptor to the hoblist
        hoblist.push(Hob::ResourceDescriptor(&resource));
        hoblist.push(Hob::Handoff(&handoff));
        hoblist.push(Hob::FirmwareVolume(&firmware_volume));
        hoblist.push(Hob::FirmwareVolume2(&firmware_volume2));
        hoblist.push(Hob::FirmwareVolume3(&firmware_volume3));
        hoblist.push(Hob::Capsule(&capsule));
        hoblist.push(Hob::GuidHob(guid_hob, guid_hob_data));
        hoblist.push(Hob::MemoryAllocation(&memory_allocation));
        hoblist.push(Hob::MemoryAllocationModule(&memory_allocation_module));
        hoblist.push(Hob::Cpu(&cpu));
        hoblist.push(Hob::ResourceDescriptorV2(&resource_v2));
        hoblist.push(Hob::Handoff(&end_of_hob_list));

        // assert that the hoblist has 3 hobs and they are of the correct type

        let mut count = 0;
        hoblist.iter().for_each(|hob| {
            match hob {
                Hob::ResourceDescriptor(resource) => {
                    assert_eq!(resource.resource_type, hob::EFI_RESOURCE_SYSTEM_MEMORY);
                }
                Hob::MemoryAllocation(memory_allocation) => {
                    assert_eq!(memory_allocation.alloc_descriptor.memory_length, 0x0123456789abcdef);
                }
                Hob::MemoryAllocationModule(memory_allocation_module) => {
                    assert_eq!(memory_allocation_module.alloc_descriptor.memory_length, 0x0123456789abcdef);
                }
                Hob::Capsule(capsule) => {
                    assert_eq!(capsule.base_address, 0);
                }
                Hob::GuidHob(guid_hob, data) => {
                    assert_eq!(guid_hob.name, r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]));
                    assert_eq!(&data[..], guid_hob_data);
                }
                Hob::FirmwareVolume(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::FirmwareVolume2(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::FirmwareVolume3(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::Handoff(handoff) => {
                    assert_eq!(handoff.memory_top, 0xdeadbeef);
                }
                Hob::Cpu(cpu) => {
                    assert_eq!(cpu.size_of_memory_space, 0);
                }
                Hob::ResourceDescriptorV2(resource) => {
                    assert_eq!(resource.v1.header.r#type, hob::RESOURCE_DESCRIPTOR2);
                    assert_eq!(resource.v1.resource_type, hob::EFI_RESOURCE_SYSTEM_MEMORY);
                }
                _ => {
                    panic!("Unexpected hob type");
                }
            }
            count += 1;
        });

        assert_eq!(count, 12);

        // c_hoblist is a pointer to the hoblist - we need to manually free it later
        let (c_array_hoblist, length) = to_c_array(&hoblist);

        // create a new hoblist
        let mut cloned_hoblist = HobList::new();
        cloned_hoblist.discover_hobs(c_array_hoblist);

        // assert that the hoblist has 2 hobs and they are of the correct type
        // we don't need to check the end of hoblist hob as it will not be 'discovered'
        // by the discover_hobs function and simply end the iteration
        count = 0;
        hoblist.into_iter().for_each(|hob| {
            match hob {
                Hob::ResourceDescriptor(resource) => {
                    assert_eq!(resource.resource_type, hob::EFI_RESOURCE_SYSTEM_MEMORY);
                }
                Hob::MemoryAllocation(memory_allocation) => {
                    assert_eq!(memory_allocation.alloc_descriptor.memory_length, 0x0123456789abcdef);
                }
                Hob::MemoryAllocationModule(memory_allocation_module) => {
                    assert_eq!(memory_allocation_module.alloc_descriptor.memory_length, 0x0123456789abcdef);
                }
                Hob::Capsule(capsule) => {
                    assert_eq!(capsule.base_address, 0);
                }
                Hob::GuidHob(guid_hob, data) => {
                    assert_eq!(guid_hob.name, r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]));
                    assert_eq!(data, &[1_u8, 2, 3, 4, 5, 6, 7, 8]);
                }
                Hob::FirmwareVolume(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::FirmwareVolume2(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::FirmwareVolume3(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::Handoff(handoff) => {
                    assert_eq!(handoff.memory_top, 0xdeadbeef);
                }
                Hob::ResourceDescriptorV2(resource) => {
                    assert_eq!(resource.v1.header.r#type, hob::RESOURCE_DESCRIPTOR2);
                    assert_eq!(resource.v1.resource_type, hob::EFI_RESOURCE_SYSTEM_MEMORY);
                }
                Hob::Cpu(cpu) => {
                    assert_eq!(cpu.size_of_memory_space, 0);
                }
                _ => {
                    panic!("Unexpected hob type");
                }
            }
            count += 1;
        });

        assert_eq!(count, 12);

        // free the c array
        manually_free_c_array(c_array_hoblist, length);
    }

    #[test]
    fn test_hob_iterator() {
        // generate some test hobs
        let resource = gen_resource_descriptor();
        let handoff = gen_phase_handoff_information_table();
        let firmware_volume = gen_firmware_volume();
        let firmware_volume2 = gen_firmware_volume2();
        let firmware_volume3 = gen_firmware_volume3();
        let capsule = gen_capsule();
        let guid_hob_buf = gen_guid_hob();
        let (guid_hob, guid_hob_data) = guid_hob_refs(&guid_hob_buf);
        let memory_allocation = gen_memory_allocation();
        let memory_allocation_module = gen_memory_allocation_module();
        let cpu = gen_cpu();
        let end_of_hob_list = gen_end_of_hoblist();

        // create a new hoblist
        let mut hoblist = HobList::new();

        // Push the resource descriptor to the hoblist
        hoblist.push(Hob::ResourceDescriptor(&resource));
        hoblist.push(Hob::Handoff(&handoff));
        hoblist.push(Hob::FirmwareVolume(&firmware_volume));
        hoblist.push(Hob::FirmwareVolume2(&firmware_volume2));
        hoblist.push(Hob::FirmwareVolume3(&firmware_volume3));
        hoblist.push(Hob::Capsule(&capsule));
        hoblist.push(Hob::GuidHob(guid_hob, guid_hob_data));
        hoblist.push(Hob::MemoryAllocation(&memory_allocation));
        hoblist.push(Hob::MemoryAllocationModule(&memory_allocation_module));
        hoblist.push(Hob::Cpu(&cpu));
        hoblist.push(Hob::Handoff(&end_of_hob_list));

        let (c_array_hoblist, length) = to_c_array(&hoblist);

        // SAFETY: Test code - creating a reference from C array pointer for HOB testing.
        let hob = Hob::ResourceDescriptor(unsafe {
            (c_array_hoblist as *const hob::ResourceDescriptor).as_ref::<'static>().unwrap()
        });
        for h in &hob {
            println!("{:?}", h.header());
        }

        manually_free_c_array(c_array_hoblist, length);
    }

    #[test]
    fn test_hob_iterator2() {
        let resource = gen_resource_descriptor();
        let handoff = gen_phase_handoff_information_table();
        let firmware_volume = gen_firmware_volume();
        let firmware_volume2 = gen_firmware_volume2();
        let firmware_volume3 = gen_firmware_volume3();
        let capsule = gen_capsule();
        let guid_hob_buf = gen_guid_hob();
        let (guid_hob, guid_hob_data) = guid_hob_refs(&guid_hob_buf);
        let memory_allocation = gen_memory_allocation();
        let memory_allocation_module = gen_memory_allocation_module();
        let cpu = gen_cpu();
        let resource_v2 = gen_resource_descriptor_v2();
        let end_of_hob_list = gen_end_of_hoblist();

        // create a new hoblist
        let mut hoblist = HobList::new();

        // Push the resource descriptor to the hoblist
        hoblist.push(Hob::ResourceDescriptor(&resource));
        hoblist.push(Hob::Handoff(&handoff));
        hoblist.push(Hob::FirmwareVolume(&firmware_volume));
        hoblist.push(Hob::FirmwareVolume2(&firmware_volume2));
        hoblist.push(Hob::FirmwareVolume3(&firmware_volume3));
        hoblist.push(Hob::Capsule(&capsule));
        hoblist.push(Hob::GuidHob(guid_hob, guid_hob_data));
        hoblist.push(Hob::MemoryAllocation(&memory_allocation));
        hoblist.push(Hob::MemoryAllocationModule(&memory_allocation_module));
        hoblist.push(Hob::Cpu(&cpu));
        hoblist.push(Hob::ResourceDescriptorV2(&resource_v2));
        hoblist.push(Hob::Handoff(&end_of_hob_list));

        // Make sure we can iterate over a reference to a HobList without
        // consuming it.
        for hob in &hoblist {
            println!("{:?}", hob.header());
        }

        for hob in hoblist {
            println!("{:?}", hob.header());
        }
    }

    #[test]
    fn test_relocate_hobs() {
        // generate some test hobs
        let resource = gen_resource_descriptor();
        let handoff = gen_phase_handoff_information_table();
        let firmware_volume = gen_firmware_volume();
        let firmware_volume2 = gen_firmware_volume2();
        let firmware_volume3 = gen_firmware_volume3();
        let capsule = gen_capsule();
        let guid_hob_buf = gen_guid_hob();
        let (guid_hob, guid_hob_data) = guid_hob_refs(&guid_hob_buf);
        let memory_allocation = gen_memory_allocation();
        let memory_allocation_module = gen_memory_allocation_module();
        let cpu = gen_cpu();
        let resource_v2 = gen_resource_descriptor_v2();
        let end_of_hob_list = gen_end_of_hoblist();

        // create a new hoblist
        let mut hoblist = HobList::new();

        // Push the resource descriptor to the hoblist
        hoblist.push(Hob::ResourceDescriptor(&resource));
        hoblist.push(Hob::Handoff(&handoff));
        hoblist.push(Hob::FirmwareVolume(&firmware_volume));
        hoblist.push(Hob::FirmwareVolume2(&firmware_volume2));
        hoblist.push(Hob::FirmwareVolume3(&firmware_volume3));
        hoblist.push(Hob::Capsule(&capsule));
        hoblist.push(Hob::GuidHob(guid_hob, guid_hob_data));
        hoblist.push(Hob::MemoryAllocation(&memory_allocation));
        hoblist.push(Hob::MemoryAllocationModule(&memory_allocation_module));
        hoblist.push(Hob::Cpu(&cpu));
        hoblist.push(Hob::Misc(12345));
        hoblist.push(Hob::ResourceDescriptorV2(&resource_v2));
        hoblist.push(Hob::Handoff(&end_of_hob_list));

        let hoblist_address = hoblist.as_mut_ptr::<()>() as usize;
        let hoblist_len = hoblist.len();
        hoblist.relocate_hobs();
        assert_eq!(
            hoblist_address,
            hoblist.as_mut_ptr::<()>() as usize,
            "Only hobs need to be relocated, not the vector."
        );
        assert_eq!(hoblist_len, hoblist.len());

        for (i, hob) in hoblist.into_iter().enumerate() {
            match hob {
                Hob::ResourceDescriptor(hob) if i == 0 => {
                    assert_ne!(ptr::addr_of!(resource), hob);
                    assert_eq!(resource, *hob);
                }
                Hob::Handoff(hob) if i == 1 => {
                    assert_ne!(ptr::addr_of!(handoff), hob);
                    assert_eq!(handoff, *hob);
                }
                Hob::FirmwareVolume(hob) if i == 2 => {
                    assert_ne!(ptr::addr_of!(firmware_volume), hob);
                    assert_eq!(firmware_volume, *hob);
                }
                Hob::FirmwareVolume2(hob) if i == 3 => {
                    assert_ne!(ptr::addr_of!(firmware_volume2), hob);
                    assert_eq!(firmware_volume2, *hob);
                }
                Hob::FirmwareVolume3(hob) if i == 4 => {
                    assert_ne!(ptr::addr_of!(firmware_volume3), hob);
                    assert_eq!(firmware_volume3, *hob);
                }
                Hob::Capsule(hob) if i == 5 => {
                    assert_ne!(ptr::addr_of!(capsule), hob);
                    assert_eq!(capsule, *hob);
                }
                Hob::GuidHob(hob, hob_data) if i == 6 => {
                    assert_ne!(ptr::from_ref(guid_hob), ptr::from_ref(hob));
                    assert_ne!(guid_hob_data.as_ptr(), hob_data.as_ptr());
                    assert_eq!(guid_hob.header, hob.header);
                    assert_eq!(guid_hob.name, hob.name);
                    assert_eq!(guid_hob_data, hob_data);
                }
                Hob::MemoryAllocation(hob) if i == 7 => {
                    assert_ne!(ptr::addr_of!(memory_allocation), hob);
                    assert_eq!(memory_allocation.header, hob.header);
                    assert_eq!(memory_allocation.alloc_descriptor, hob.alloc_descriptor);
                }
                Hob::MemoryAllocationModule(hob) if i == 8 => {
                    assert_ne!(ptr::addr_of!(memory_allocation_module), hob);
                    assert_eq!(memory_allocation_module, *hob);
                }
                Hob::Cpu(hob) if i == 9 => {
                    assert_ne!(ptr::addr_of!(cpu), hob);
                    assert_eq!(cpu, *hob);
                }
                Hob::Misc(hob) if i == 10 => {
                    assert_eq!(12345, hob);
                }
                Hob::ResourceDescriptorV2(hob) if i == 11 => {
                    assert_ne!(ptr::addr_of!(resource_v2), hob);
                    assert_eq!(resource_v2, *hob);
                }
                Hob::Handoff(hob) if i == 12 => {
                    assert_ne!(ptr::addr_of!(end_of_hob_list), hob);
                    assert_eq!(end_of_hob_list, *hob);
                }
                _ => panic!("Hob at index: {i}."),
            }
        }
    }

    #[test]
    fn test_hoblist_debug_display() {
        use alloc::format;

        let mut hoblist = HobList::new();
        let handoff = gen_phase_handoff_information_table();
        hoblist.push(Hob::Handoff(&handoff));

        let debug_output = format!("{:?}", hoblist);

        assert!(debug_output.contains("PHASE HANDOFF INFORMATION TABLE"));
        assert!(debug_output.contains("HOB Length:"));
        assert!(debug_output.contains("Version:"));
        assert!(debug_output.contains("Boot Mode:"));
        assert!(debug_output.contains("Memory Bottom:"));
        assert!(debug_output.contains("Memory Top:"));
        assert!(debug_output.contains("Free Memory Bottom:"));
        assert!(debug_output.contains("Free Memory Top:"));
        assert!(debug_output.contains("End of HOB List:"));
    }

    #[test]
    fn test_get_pi_hob_list_size_single_hob() {
        use core::ffi::c_void;

        let end_of_list = gen_end_of_hoblist();

        // SAFETY: The list is created in this test with a valid end-of-list marker
        let size = unsafe { get_pi_hob_list_size(&end_of_list as *const _ as *const c_void) };

        assert_eq!(size, size_of::<PhaseHandoffInformationTable>());
    }

    #[test]
    fn test_get_pi_hob_list_size_multiple_hobs() {
        use core::ffi::c_void;

        // Create a HOB list with multiple HOBs in contiguous memory
        let capsule = gen_capsule();
        let firmware_volume = gen_firmware_volume();
        let end_of_list = gen_end_of_hoblist();

        let expected_size =
            size_of::<Capsule>() + size_of::<FirmwareVolume>() + size_of::<PhaseHandoffInformationTable>();

        // This buffer will hold the contiguous HOBs
        let mut buffer = Vec::new();

        // Add a capsule HOB
        // SAFETY: Creating a byte slice from a struct for test purposes.
        let capsule_bytes =
            unsafe { core::slice::from_raw_parts(&capsule as *const Capsule as *const u8, size_of::<Capsule>()) };
        buffer.extend_from_slice(capsule_bytes);

        // Add a firmware volume HOB
        // SAFETY: Creating a byte slice from a struct for test purposes.
        let fv_bytes = unsafe {
            core::slice::from_raw_parts(
                &firmware_volume as *const FirmwareVolume as *const u8,
                size_of::<FirmwareVolume>(),
            )
        };
        buffer.extend_from_slice(fv_bytes);

        // Add an end-of-list HOB
        // SAFETY: Creating a byte slice from a struct for test purposes.
        let end_bytes = unsafe {
            core::slice::from_raw_parts(
                &end_of_list as *const PhaseHandoffInformationTable as *const u8,
                size_of::<PhaseHandoffInformationTable>(),
            )
        };
        buffer.extend_from_slice(end_bytes);

        // SAFETY: The list is created in this test with headers and an end-of-list marker that should be valid
        let size = unsafe { get_pi_hob_list_size(buffer.as_ptr() as *const c_void) };

        assert_eq!(size, expected_size);
    }

    #[test]
    fn test_get_pi_hob_list_size_varied_hob_types() {
        use core::ffi::c_void;

        // Create a HOB list with various HOB types
        let cpu = gen_cpu();
        let resource = gen_resource_descriptor();
        let memory_alloc = gen_memory_allocation();
        let end_of_list = gen_end_of_hoblist();

        let expected_size = size_of::<Cpu>()
            + size_of::<ResourceDescriptor>()
            + size_of::<MemoryAllocation>()
            + size_of::<PhaseHandoffInformationTable>();

        // This buffer will hold the contiguous HOBs
        let mut buffer = Vec::new();

        // SAFETY: Creating a byte slice from a struct for test purposes.
        buffer.extend_from_slice(unsafe {
            core::slice::from_raw_parts(&cpu as *const Cpu as *const u8, size_of::<Cpu>())
        });

        // SAFETY: Creating a byte slice from a struct for test purposes.
        buffer.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &resource as *const ResourceDescriptor as *const u8,
                size_of::<ResourceDescriptor>(),
            )
        });

        // SAFETY: Creating a byte slice from a struct for test purposes.
        buffer.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &memory_alloc as *const MemoryAllocation as *const u8,
                size_of::<MemoryAllocation>(),
            )
        });

        // SAFETY: Creating a byte slice from a struct for test purposes.
        buffer.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &end_of_list as *const PhaseHandoffInformationTable as *const u8,
                size_of::<PhaseHandoffInformationTable>(),
            )
        });

        // SAFETY: The list is created in this test with headers and an end-of-list marker that should be valid
        let size = unsafe { get_pi_hob_list_size(buffer.as_ptr() as *const c_void) };

        assert_eq!(size, expected_size);
    }
}
