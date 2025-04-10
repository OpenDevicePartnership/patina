//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use alloc::{
    ffi::CString,
    string::{String, ToString},
};
use core::ffi::{c_char, CStr};

/// # Safety
/// make sure c_ptr a valid c string pointer.
pub unsafe fn string_from_c_char_ptr(c_ptr: *const c_char) -> Option<String> {
    if c_ptr.is_null() {
        return None;
    }
    Some(CStr::from_ptr(c_ptr).to_str().unwrap().to_string())
}

pub fn c_char_ptr_from_str(s: &str) -> *const c_char {
    CString::new(s).map_or(core::ptr::null(), |c_string| c_string.into_raw())
}

#[cfg(test)]
mod test {
    use super::*;
    use core::{assert_eq, ptr, slice};

    #[test]
    fn test_string_from_c_char_ptr_with_null_ptr() {
        assert_eq!(None, unsafe { string_from_c_char_ptr(ptr::null()) });
    }

    #[test]
    fn test_string_from_c_char_ptr() {
        let s = b"this is a string.\0";
        let ptr = s.as_ptr() as *const c_char;
        assert_eq!("this is a string.", unsafe { string_from_c_char_ptr(ptr) }.unwrap().as_str());
    }

    #[test]
    fn test_c_char_ptr_from_str() {
        let s = "this is a string.";
        let ptr = c_char_ptr_from_str(s);
        let byte_str = unsafe { slice::from_raw_parts(ptr as *const u8, s.len() + 1) };
        let expected = b"this is a string.\0";
        assert_eq!(expected, byte_str);
    }
}
