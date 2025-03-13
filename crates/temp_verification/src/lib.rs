#![no_std] // i'm pretty sure we need to be nostd but if we don't it would make things a lot easier

use core::mem;
use core::{ffi::c_void, fmt};

use mu_pi::hob::{header, EfiPhysicalAddress, Hob, PhaseHandoffInformationTable, END_OF_HOB_LIST, HANDOFF};

pub mod bump_allocator;
pub mod fv_verify;
pub mod hob_verify;
mod primitive_logger;

/// Public result type for the crate.
pub type Result<T> = core::result::Result<T, PlatformError>;

// also duplicated from mu_pi but again idk if we should move it
const NOT_NULL: &str = "Ptr should not be NULL";

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
    InternalError, // error from verification code, not the platform itself. don't know if this is a good error to have.
}

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

// this is duplicated in mu_pi but idk if we should pull it out here
fn assert_hob_size<T>(hob: &header::Hob) {
    let hob_len = hob.length as usize;
    let hob_size = mem::size_of::<T>();
    assert_eq!(hob_len, hob_size, "Trying to cast hob of length {hob_len} into a pointer of size {hob_size}");
}

// find free memory space for phit hob
pub fn read_phit_hob(physical_hob_list: *const c_void) -> Option<(usize, usize)> {
    if physical_hob_list.is_null() {
        panic!("HOB list pointer is null!");
    }

    let mut hob_header: *const header::Hob = physical_hob_list as *const header::Hob;

    // is this PHIT hob always the first? nothing in the PI spec specifically says this so i guess we can be conservative and search for it for now
    // also: a lot of HOB iteration. should we make a (non-memory-using) iterator?
    // or, can we use HobList as a convinence after implementing an allocator?
    loop {
        let current_header = unsafe { hob_header.cast::<header::Hob>().as_ref().expect(NOT_NULL) };
        if current_header.r#type == HANDOFF {
            assert_hob_size::<PhaseHandoffInformationTable>(current_header);
            let phit_hob = unsafe { hob_header.cast::<PhaseHandoffInformationTable>().as_ref().expect(NOT_NULL) };
            return Some((phit_hob.free_memory_bottom as usize, phit_hob.free_memory_top as usize));
        } else if current_header.r#type == END_OF_HOB_LIST {
            break;
        }
        let next_hob = hob_header as usize + current_header.length as usize;
        hob_header = next_hob as *const header::Hob;
    }

    None
}

// hacky workaround since tests need an actual allocator
#[cfg(not(test))]
#[global_allocator]
pub static ALLOCATOR: bump_allocator::BumpAllocator = bump_allocator::BumpAllocator::new();
