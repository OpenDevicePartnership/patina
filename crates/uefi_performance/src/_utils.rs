//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use alloc::ffi::CString;
use alloc::string::{String, ToString};
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
mod tests {
    use super::*;

    #[test]
    fn test_string_from_c_char_ptr() {
        let test_str = "hello world";
        let c_str = CString::new(test_str).unwrap();
        let c_ptr = c_str.as_ptr();

        unsafe {
            assert_eq!(string_from_c_char_ptr(c_ptr), Some(test_str.to_string()));
            assert_eq!(string_from_c_char_ptr(core::ptr::null()), None);
        }
    }

    #[test]
    fn test_c_char_ptr_from_str() {
        let test_str = "hello world";
        let c_ptr = c_char_ptr_from_str(test_str);

        unsafe {
            assert_eq!(CStr::from_ptr(c_ptr).to_str().unwrap(), test_str);
        }

        // Test empty string
        let empty_str = "";
        let c_ptr = c_char_ptr_from_str(empty_str);

        unsafe {
            assert_eq!(CStr::from_ptr(c_ptr).to_str().unwrap(), empty_str);
        }
    }
}
