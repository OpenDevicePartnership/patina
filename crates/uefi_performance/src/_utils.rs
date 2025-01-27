//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use alloc::alloc::alloc;
use alloc::ffi::CString;
use alloc::string::{String, ToString};
use core::ffi::{c_char, CStr};
use core::ptr;

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
