use core::ffi::c_void;

use mu_pi::hob::{
    header::{self},
    FirmwareVolume, Hob, END_OF_HOB_LIST, FV,
};

extern crate alloc;
use alloc::vec::Vec;

use crate::{assert_hob_size, PlatformError, Result, NOT_NULL};

// this code feels bad and wrong, especially the line where i dereference the fv_hob, but that's what gets it to compile...
fn find_fv_hobs(physical_hob_list: *const c_void) -> Result<Vec<FirmwareVolume>> {
    let mut hob_header: *const header::Hob = physical_hob_list as *const header::Hob;
    let mut fv_hobs = Vec::new();

    loop {
        let current_header = unsafe { hob_header.cast::<header::Hob>().as_ref().expect(NOT_NULL) };
        if current_header.r#type == FV {
            assert_hob_size::<FirmwareVolume>(current_header);
            let fv_hob = unsafe { hob_header.cast::<FirmwareVolume>().as_ref().expect(NOT_NULL) };
            fv_hobs.push(*fv_hob);
        } else if current_header.r#type == END_OF_HOB_LIST {
            break;
        }

        let next_hob = hob_header as usize + current_header.length as usize;
        hob_header = next_hob as *const header::Hob;
    }

    Ok(fv_hobs)
    // if there are no fv hobs maybe we should return an error
}
