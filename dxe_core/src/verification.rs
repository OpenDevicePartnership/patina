use core::fmt;

use mu_pi::hob::{EfiPhysicalAddress, Hob, HobList, ResourceDescriptor};
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
    MissingMemoryProtections,
}

// SHERRY: is this defined anywhere? i didn't see it but maybe i didn't look hard enough. could be in another repo (not here or mu_pi at least)
const DXE_MEMORY_PROTECTION_SETTINGS_GUID: efi::Guid =
    efi::Guid::from_fields(0x9E9FD06B, 0x873D, 0x47D9, 0xA2, 0x07, &[0x4F, 0x7A, 0x2D, 0xA0, 0x07, 0x5A]);

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
                    "Memory range overlap detected: [{:#x}, {:#x}) overlaps with [{:#x}, {:#x})",
                    start1, end1, start2, end2
                )
            }
            PlatformError::MissingMemoryProtections => {
                write!(f, "Memory protection settings HOB is missing or invalid")
            }
        }
    }
}
pub fn verify_platform_requirements(hob_list: &HobList) -> Result<()> {
    verify_resource_descriptor_hobs(hob_list)?;
    verify_memory_protection_hobs(hob_list)?;
    Ok(())
}

fn verify_resource_descriptor_hobs(hob_list: &HobList) -> Result<()> {
    check_memory_overlap(hob_list)?;
    check_v1_v2_consistency(hob_list)?;
    Ok(())
}

fn get_memory_range(hob: &Hob) -> Option<(EfiPhysicalAddress, EfiPhysicalAddress)> {
    match hob {
        Hob::ResourceDescriptorV2(hob) => {
            let start = hob.v1.physical_start;
            let end = start.saturating_add(hob.v1.resource_length);
            Some((start, end))
        }
        Hob::ResourceDescriptor(hob) => {
            let start = hob.physical_start;
            let end = start.saturating_add(hob.resource_length);
            Some((start, end))
        }
        _ => None, // Ignore other HOB types
    }
}

fn check_hob_overlap<F>(hob_list: &HobList, filter: F) -> Result<()>
where
    F: Fn(&Hob) -> bool,
{
    for (i, hob1) in hob_list.iter().enumerate() {
        if filter(hob1) {
            if let Some((start1, end1)) = get_memory_range(hob1) {
                for hob2 in hob_list.iter().skip(i + 1) {
                    if filter(hob2) {
                        if let Some((start2, end2)) = get_memory_range(hob2) {
                            if start1 < end2 && start2 < end1 {
                                return Err(PlatformError::MemoryRangeOverlap { start1, end1, start2, end2 });
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn check_memory_overlap(hob_list: &HobList) -> Result<()> {
    // Check overlaps within V2 HOBs
    check_hob_overlap(hob_list, |hob| matches!(hob, Hob::ResourceDescriptorV2(_)))?;

    // Check overlaps within V1 HOBs
    check_hob_overlap(hob_list, |hob| matches!(hob, Hob::ResourceDescriptor(_)))?;

    Ok(())
}

fn check_v1_v2_consistency(hob_list: &HobList) -> Result<()> {
    for v1_hob in hob_list.iter() {
        if let Hob::ResourceDescriptor(v1) = v1_hob {
            let (v1_start, v1_end) = get_memory_range(v1_hob).unwrap();

            for v2_hob in hob_list.iter() {
                if let Hob::ResourceDescriptorV2(v2) = v2_hob {
                    let (v2_start, v2_end) = get_memory_range(v2_hob).unwrap();

                    // Check if the V1 and V2 HOBs overlap
                    if v1_start < v2_end && v2_start < v1_end {
                        // Ensure fields are consistent
                        if !is_consistent(v1, &v2.v1) {
                            return Err(PlatformError::InconsistentMemoryAttributes {
                                start1: v1_start,
                                end1: v1_end,
                                start2: v2_start,
                                end2: v2_end,
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn is_consistent(v1: &ResourceDescriptor, v2: &ResourceDescriptor) -> bool {
    v1.resource_type == v2.resource_type && v1.resource_attribute == v2.resource_attribute && v1.owner == v2.owner
}

fn verify_memory_protection_hobs(hob_list: &HobList) -> Result<()> {
    for hob in hob_list {
        if let Hob::GuidHob(guid_hob, _) = hob {
            if guid_hob.name == DXE_MEMORY_PROTECTION_SETTINGS_GUID {
                return Ok(()); // Found the target GUID, verification passes
            }
        }
    }

    Err(PlatformError::MissingMemoryProtections)
}
