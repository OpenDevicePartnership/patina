//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

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

pub fn c_char_ptr_from_str(str: &str) -> *const c_char {
    let mut s = String::from(str);
    s.push(0 as char);
    s.as_ptr() as *const c_char
}
