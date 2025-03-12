use core::{ffi::c_void, fmt, mem};

use mu_pi::hob::{
    header, EfiPhysicalAddress, GuidHob, ResourceDescriptor, ResourceDescriptorV2, EFI_RESOURCE_IO, END_OF_HOB_LIST,
    GUID_EXTENSION, RESOURCE_DESCRIPTOR, RESOURCE_DESCRIPTOR2,
};
use r_efi::efi;

/// Public result type for the crate.
pub type Result<T> = core::result::Result<T, PlatformError>;

#[derive(Debug)]
pub enum PlatformError {
    MemoryRangeOverlap {
        start1: EfiPhysicalAddress,
        end1: EfiPhysicalAddress,
        start2: EfiPhysicalAddress,
        end2: EfiPhysicalAddress,
    },
    InconsistentMemoryAttributes {
        start1: EfiPhysicalAddress,
        end1: EfiPhysicalAddress,
        start2: EfiPhysicalAddress,
        end2: EfiPhysicalAddress,
    },
    InconsistentRanges,
    MissingMemoryProtections,
    InternalError, // error from verification code, not the platform itself
}

/// A struct that runs multiple requirements
/// The idea is that we'll be able to add new requirements dynamically
/// Although it's not as dynamic as you'd like
/// Because there is a max size on the Requirements slice
pub struct Runner<'a> {
    requirements: &'a mut [&'a dyn Requirement],
    count: usize,
}

impl<'a> Runner<'a> {
    /// Creates a new Runner instance with a mutable slice of requirements.
    pub fn new(requirements: &'a mut [&'a dyn Requirement]) -> Self {
        Self { requirements, count: 0 }
    }

    /// Adds a new requirement to the Runner (if space is available).
    pub fn add_requirement(&mut self, requirement: &'a dyn Requirement) -> Result<()> {
        if self.count < self.requirements.len() {
            self.requirements[self.count] = requirement;
            self.count += 1;
            Ok(())
        } else {
            Err(PlatformError::InternalError) // No space left
        }
    }

    /// Runs all added requirements against the given HOB list.
    pub fn run(&self, physical_hob_list: *const c_void) -> Result<()> {
        for i in 0..self.count {
            self.requirements[i].verify_requirement(physical_hob_list)?;
        }
        Ok(())
    }
}

/// Trait for verifying platform-specific requirements.
pub trait Requirement {
    // i don't know if this is the right argument. do we need info other than the physical hob list sometimes?
    fn verify_requirement(&self, physical_hob_list: *const c_void) -> Result<()>;
}

// SHERRY: is this defined anywhere? i didn't see it but maybe i didn't look hard enough. could be in another repo (not here or mu_pi at least)
const DXE_MEMORY_PROTECTION_SETTINGS_GUID: efi::Guid =
    efi::Guid::from_fields(0x9ABFD639, 0xD1D0, 0x4EFF, 0xBD, 0xB6, &[0x7E, 0xC4, 0x19, 0x0D, 0x17, 0xD5]);
const NOT_NULL: &str = "Ptr should not be NULL";

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformError::MemoryRangeOverlap { start1, end1, start2, end2 } => {
                write!(
                    f,
                    "Memory range overlap detected: [{:#x}, {:#x}) overlaps with [{:#x}, {:#x})",
                    start1, end1, start2, end2
                )
            }
            PlatformError::InconsistentMemoryAttributes { start1, end1, start2, end2 } => {
                write!(
                    f,
                    "Memory ranges overlap but have different attributes: [{:#x}, {:#x}) overlaps with [{:#x}, {:#x})",
                    start1, end1, start2, end2
                )
            }
            PlatformError::InconsistentRanges => {
                write!(f, "V1 and V2 ranges do not match")
            }
            PlatformError::MissingMemoryProtections => {
                write!(f, "Memory protection settings HOB is missing or invalid")
            }
            // This could probably be more disambiguated
            PlatformError::InternalError => {
                write!(f, "Verification failed due to internal error")
            }
        }
    }
}

pub fn verify_platform_requirements(physical_hob_list: *const c_void) -> Result<()> {
    // all the logs in this file are bad (we eventually need to replace them with our own logger)
    log::info!("Verifying platform requirements...");

    let overlap_req = OverlapRequirement;

    // this probably needs to have way more requirements but i'll poc it with one
    let mut storage: [&dyn Requirement; 1] = [&overlap_req];
    let mut runner = Runner::new(&mut storage);

    let _ = runner.add_requirement(&overlap_req);

    // Run the requirements
    match runner.run(physical_hob_list) {
        Ok(()) => log::info!("All requirements passed."),
        Err(e) => log::info!("Requirement check failed: {:?}", e),
    }

    log::info!("Passed platform verification.");
    Ok(())
}

fn verify_resource_descriptor_hobs(physical_hob_list: *const c_void) -> Result<()> {
    check_memory_overlap(physical_hob_list)?;
    check_v1_v2_consistency(physical_hob_list)?;
    Ok(())
}

fn assert_hob_size<T>(hob: &header::Hob) {
    let hob_len = hob.length as usize;
    let hob_size = mem::size_of::<T>();
    assert_eq!(hob_len, hob_size, "Trying to cast hob of length {hob_len} into a pointer of size {hob_size}");
}

/// A struct that checks for memory range overlaps
pub struct OverlapRequirement;

impl Requirement for OverlapRequirement {
    fn verify_requirement(&self, physical_hob_list: *const c_void) -> Result<()> {
        check_hob_overlap(physical_hob_list)
    }
}

// this is not correct since we have to account for resource types
// TODO: implement I/O + ResourceDescriptorV2
fn check_hob_overlap(physical_hob_list: *const c_void) -> Result<()> {
    let mut hob_header1: *const header::Hob = physical_hob_list as *const header::Hob;

    loop {
        let current_header1 = unsafe { hob_header1.cast::<header::Hob>().as_ref().expect(NOT_NULL) };
        if current_header1.r#type == RESOURCE_DESCRIPTOR {
            assert_hob_size::<ResourceDescriptor>(current_header1);
            let resource_desc_hob1 = unsafe { hob_header1.cast::<ResourceDescriptor>().as_ref().expect(NOT_NULL) };
            if resource_desc_hob1.resource_type != EFI_RESOURCE_IO {
                let (start1, end1) = (
                    resource_desc_hob1.physical_start,
                    resource_desc_hob1.physical_start + resource_desc_hob1.resource_length,
                );

                // start one after the current HOB
                let mut hob_header2: *const header::Hob =
                    (hob_header1 as usize + current_header1.length as usize) as *const header::Hob;
                loop {
                    let current_header2 = unsafe { hob_header2.cast::<header::Hob>().as_ref().expect(NOT_NULL) };
                    if current_header2.r#type == RESOURCE_DESCRIPTOR {
                        assert_hob_size::<ResourceDescriptor>(current_header2);
                        let resource_desc_hob2 =
                            unsafe { hob_header2.cast::<ResourceDescriptor>().as_ref().expect(NOT_NULL) };
                        if resource_desc_hob2.resource_type != EFI_RESOURCE_IO {
                            let (start2, end2) = (
                                resource_desc_hob2.physical_start,
                                resource_desc_hob2.physical_start + resource_desc_hob2.resource_length,
                            );

                            if start1 < end2 && start2 < end1 {
                                return Err(PlatformError::MemoryRangeOverlap { start1, end1, start2, end2 });
                            }
                        }
                    } else if current_header2.r#type == END_OF_HOB_LIST {
                        break;
                    }

                    let next_hob2 = hob_header2 as usize + current_header2.length as usize;
                    hob_header2 = next_hob2 as *const header::Hob;
                }
            }
        } else if current_header1.r#type == END_OF_HOB_LIST {
            break;
        }

        let next_hob = hob_header1 as usize + current_header1.length as usize;
        hob_header1 = next_hob as *const header::Hob;
    }

    Ok(())
}

fn check_memory_overlap(physical_hob_list: *const c_void) -> Result<()> {
    check_hob_overlap(physical_hob_list)?;
    Ok(())
}

const MAX_INTERVALS: usize = 20; // this is probably enough based on the hob dumps?

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Interval {
    start: u64,
    end: u64,
}

// A simple bubble sort to sort the intervals by start time
fn bubble_sort(intervals: &mut [Option<Interval>; MAX_INTERVALS], count: usize) {
    for i in 0..count {
        for j in 0..(count - i - 1) {
            if let (Some(a), Some(b)) = (intervals[j], intervals[j + 1]) {
                if a.start > b.start {
                    intervals.swap(j, j + 1); // Swap if intervals are out of order
                }
            }
        }
    }
}

fn merge_intervals(
    intervals: &mut [Option<Interval>; MAX_INTERVALS],
    count: usize,
) -> [Option<Interval>; MAX_INTERVALS] {
    let mut merged: [Option<Interval>; MAX_INTERVALS] = Default::default();
    let mut merged_count = 0;

    // Sort the intervals by start using bubble_sort (no std)
    bubble_sort(intervals, count);

    let mut prev_end = i32::MIN;
    for i in 0..count {
        if let Some(current) = intervals[i] {
            if merged_count == 0 || merged[merged_count - 1].unwrap().end < current.start {
                merged[merged_count] = Some(current); // No overlap, add as a new interval
                merged_count += 1;
            } else {
                // Merge overlapping intervals
                let last = merged[merged_count - 1].as_mut().unwrap();
                last.end = last.end.max(current.end);
            }
        }
    }

    merged
}

fn check_v1_v2_ranges(physical_hob_list: *const c_void) -> Result<()> {
    let mut v1_intervals: [Option<Interval>; MAX_INTERVALS] = Default::default();
    let mut v2_intervals: [Option<Interval>; MAX_INTERVALS] = Default::default();
    let mut v1_index = 0;
    let mut v2_index = 0;

    let mut hob_header: *const header::Hob = physical_hob_list as *const header::Hob;
    loop {
        let current_header = unsafe { hob_header.cast::<header::Hob>().as_ref().expect(NOT_NULL) };
        if current_header.r#type == RESOURCE_DESCRIPTOR {
            assert_hob_size::<ResourceDescriptor>(current_header);
            let resource_desc_hob = unsafe { hob_header.cast::<ResourceDescriptor>().as_ref().expect(NOT_NULL) };
            v1_intervals[v1_index] = Some(Interval {
                start: resource_desc_hob.physical_start,
                end: resource_desc_hob.physical_start + resource_desc_hob.resource_length,
            });
            v1_index += 1;
        } else if current_header.r#type == RESOURCE_DESCRIPTOR2 {
            assert_hob_size::<ResourceDescriptorV2>(current_header);
            let resource_desc_hob = unsafe { hob_header.cast::<ResourceDescriptorV2>().as_ref().expect(NOT_NULL) };
            v2_intervals[v2_index] = Some(Interval {
                start: resource_desc_hob.v1.physical_start,
                end: resource_desc_hob.v1.physical_start + resource_desc_hob.v1.resource_length,
            });
            v2_index += 1;
        } else if current_header.r#type == END_OF_HOB_LIST {
            break;
        }

        let next_hob = hob_header as usize + current_header.length as usize;
        hob_header = next_hob as *const header::Hob;
    }

    let v1_merged = merge_intervals(&mut v1_intervals, v1_index);
    let v2_merged = merge_intervals(&mut v2_intervals, v2_index);
    if v1_merged == v2_merged {
        Ok(())
    } else {
        Err(PlatformError::InconsistentRanges)
    }
}

fn check_v1_v2_consistency(physical_hob_list: *const c_void) -> Result<()> {
    let mut hob_header1: *const header::Hob = physical_hob_list as *const header::Hob;

    loop {
        let current_header1 = unsafe { hob_header1.cast::<header::Hob>().as_ref().expect(NOT_NULL) };
        // this part checks that overlapping regions don't have any conflicting info
        if current_header1.r#type == RESOURCE_DESCRIPTOR {
            assert_hob_size::<ResourceDescriptor>(current_header1);
            let resource_desc_hob1 = unsafe { hob_header1.cast::<ResourceDescriptor>().as_ref().expect(NOT_NULL) };
            if resource_desc_hob1.resource_type != EFI_RESOURCE_IO {
                let (start1, end1) = (
                    resource_desc_hob1.physical_start,
                    resource_desc_hob1.physical_start + resource_desc_hob1.resource_length,
                );

                // start at the beginning
                // the reasoning for this is a bit complex but basically at this point we have established no overlap within V1 hobs or within V2 hobs
                // so if there is overlap, it must be between V1/V2
                // so we pair up each possible V1 with each possible V2 (both before and after it)
                // so that we can check for consistency between (v1, v2) hobs if they overlap
                // if we do something like
                // for (i, hob) in hob_list {
                //    for (j, hob2) in hob_list[i + 1..] { (basically starting after the first hob as we do in checking overlap)
                // }
                // we will miss cases where the v2 hob comes before the v1 hob in the list, since it won't hit the first if case
                // even though we still want to check overlap in this case
                let mut hob_header2 = physical_hob_list as *const header::Hob;
                loop {
                    let current_header2 = unsafe { hob_header2.cast::<header::Hob>().as_ref().expect(NOT_NULL) };
                    if current_header2.r#type == RESOURCE_DESCRIPTOR2 {
                        assert_hob_size::<ResourceDescriptor>(current_header2);
                        let resource_desc_hob2 =
                            unsafe { hob_header2.cast::<ResourceDescriptor>().as_ref().expect(NOT_NULL) };
                        if resource_desc_hob2.resource_type != EFI_RESOURCE_IO {
                            let (start2, end2) = (
                                resource_desc_hob2.physical_start,
                                resource_desc_hob2.physical_start + resource_desc_hob2.resource_length,
                            );

                            if start1 < end2 && start2 < end1 {
                                if !is_consistent(resource_desc_hob1, resource_desc_hob2) {
                                    return Err(PlatformError::InconsistentMemoryAttributes {
                                        start1,
                                        end1,
                                        start2,
                                        end2,
                                    });
                                }
                            }
                        }
                    } else if current_header2.r#type == END_OF_HOB_LIST {
                        break;
                    }

                    let next_hob2 = hob_header2 as usize + current_header2.length as usize;
                    hob_header2 = next_hob2 as *const header::Hob;
                }
            }
        } else if current_header1.r#type == END_OF_HOB_LIST {
            break;
        }

        let next_hob = hob_header1 as usize + current_header1.length as usize;
        hob_header1 = next_hob as *const header::Hob;
    }

    // we also need to check that the ranges covered by each are the same
    // the basic steps for this:
    // 1. get all V1 intervals and V2 intervals into a list
    // 2. sort intervals
    // 3. merge intervals
    // 4. make sure the final merged ranges are the same

    Ok(())
}

fn is_consistent(v1: &ResourceDescriptor, v2: &ResourceDescriptor) -> bool {
    v1.resource_type == v2.resource_type && v1.resource_attribute == v2.resource_attribute && v1.owner == v2.owner
}

fn verify_memory_protection_hobs(physical_hob_list: *const c_void) -> Result<()> {
    let mut hob_header1: *const header::Hob = physical_hob_list as *const header::Hob;

    loop {
        let current_header1 = unsafe { hob_header1.cast::<header::Hob>().as_ref().expect(NOT_NULL) };
        if current_header1.r#type == GUID_EXTENSION {
            let guid_hob = unsafe { hob_header1.cast::<GuidHob>().as_ref().expect(NOT_NULL) };
            if guid_hob.name == DXE_MEMORY_PROTECTION_SETTINGS_GUID {
                return Ok(());
            }
        } else if current_header1.r#type == END_OF_HOB_LIST {
            break;
        }

        let next_hob = hob_header1 as usize + current_header1.length as usize;
        hob_header1 = next_hob as *const header::Hob;
    }

    Err(PlatformError::MissingMemoryProtections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mu_pi::hob::{self, header::Hob};
    use r_efi::efi::Status;

    const TEST_GUID: efi::Guid =
        efi::Guid::from_fields(0x12345678, 0x9ABC, 0xDEF0, 0x12, 0x34, &[0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF]);

    #[test]
    fn test_no_overlap() {
        // Hobs that don't overlap
        let hob1 = ResourceDescriptor {
            header: header::Hob {
                length: mem::size_of::<ResourceDescriptor>() as u16,
                r#type: RESOURCE_DESCRIPTOR,
                reserved: 0,
            },
            resource_type: 0,
            resource_length: 0x1000,
            physical_start: 0x1000,
            resource_attribute: 0,
            owner: TEST_GUID,
        };

        let hob2 = ResourceDescriptor {
            header: header::Hob {
                length: mem::size_of::<ResourceDescriptor>() as u16,
                r#type: RESOURCE_DESCRIPTOR,
                reserved: 0,
            },
            resource_type: 0,
            resource_length: 0x1000,
            physical_start: 0x2000,
            resource_attribute: 0,
            owner: TEST_GUID,
        };

        // this is a hack
        let end = ResourceDescriptor {
            header: header::Hob {
                length: mem::size_of::<ResourceDescriptor>() as u16,
                r#type: END_OF_HOB_LIST,
                reserved: 0,
            },
            resource_type: 0,
            resource_length: 0x1000,
            physical_start: 0x2000,
            resource_attribute: 0,
            owner: TEST_GUID,
        };

        let hob_list = vec![hob1, hob2, end];

        assert!(check_hob_overlap(hob_list.as_ptr() as *const c_void).is_ok());
    }

    // other tests:
    // overlapping
    // v2
    // v1 and v2 overlapping should be ok
    // should not check overlap with non resource descriptor types
    // dxe memory hobs exists / doesn't exist
    // v2 and v1 overlap and are consistent
    // v1 and v2 overlap and are not consistent
}
