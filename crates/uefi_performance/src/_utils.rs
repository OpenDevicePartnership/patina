use alloc::string::String;
use core::{ffi::c_char, ptr};

/// # Safety
/// make sure c_ptr a valid c string pointer.
pub unsafe fn string_from_c_char_ptr(mut c_ptr: *const c_char) -> Option<String> {
    if c_ptr.is_null() {
        return None;
    }

    let mut str = String::new();
    loop {
        let c = unsafe { ptr::read(c_ptr) };
        if c == 0 {
            break;
        }
        str.push(c as u8 as char);
        c_ptr = unsafe { c_ptr.add(1) };
    }
    Some(str)
}

pub fn c_char_ptr_from_str(str: &str) -> *const c_char {
    let mut s = String::from(str);
    s.push(0 as char);
    s.as_ptr() as *const c_char
}
