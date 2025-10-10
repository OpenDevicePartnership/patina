//! Helper functions related to address manipulation.
//!
//! Portions derived from the x86_64 crate addr.rs file.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// Align address downwards.
///
/// Returns the greatest `x` with alignment `align` so that `x <= addr`.
///
/// Panics if the alignment is not a power of two.
#[inline]
pub const fn align_down(addr: u64, align: u64) -> u64 {
    assert!(align.is_power_of_two(), "`align` must be a power of two");
    addr & !(align - 1)
}

/// Align address upwards.
///
/// Returns the smallest `x` with alignment `align` so that `x >= addr`.
///
/// Panics if the alignment is not a power of two or if an overflow occurs.
#[inline]
pub const fn align_up(addr: u64, align: u64) -> u64 {
    assert!(align.is_power_of_two(), "`align` must be a power of two");
    let align_mask = align - 1;
    if addr & align_mask == 0 {
        addr // already aligned
    } else {
        // FIXME: Replace with .expect, once `Option::expect` is const.
        if let Some(aligned) = (addr | align_mask).checked_add(1) {
            aligned
        } else {
            panic!("attempt to add with overflow")
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::address_helper::{align_down, align_up};

    #[test]
    #[should_panic]
    fn align_down_align_panic() {
        // alignment is not a power of 2
        align_down(0, 0x0);
    }

    #[test]
    #[should_panic]
    fn align_up_align_panic() {
        // alignment is not a power of 2
        align_up(0, 0x0);
    }

    #[test]
    fn test_align_down() {
        // align 1
        assert_eq!(align_down(0, 1), 0);
        assert_eq!(align_down(1234, 1), 1234);
        assert_eq!(align_down(0xffff_ffff_ffff_ffff, 1), 0xffff_ffff_ffff_ffff);
        // align 2
        assert_eq!(align_down(0, 2), 0);
        assert_eq!(align_down(1234, 2), 1234);
        assert_eq!(align_down(0xffff_ffff_ffff_ffff, 2), 0xffff_ffff_ffff_fffe);
        // address 0
        assert_eq!(align_down(0, 128), 0);
        assert_eq!(align_down(0, 1), 0);
        assert_eq!(align_down(0, 2), 0);
        assert_eq!(align_down(0, 0x8000_0000_0000_0000), 0);

        // Validate alignment of (align > address) -> 0
        assert_eq!(align_down(0xFFFF, 0x1000_0000), 0);
    }

    #[test]
    fn test_align_up() {
        // align 1
        assert_eq!(align_up(0, 1), 0);
        assert_eq!(align_up(1234, 1), 1234);
        assert_eq!(align_up(0xffff_ffff_ffff_ffff, 1), 0xffff_ffff_ffff_ffff);
        // align 2
        assert_eq!(align_up(0, 2), 0);
        assert_eq!(align_up(1233, 2), 1234);
        assert_eq!(align_up(0xffff_ffff_ffff_fffe, 2), 0xffff_ffff_ffff_fffe);
        // address 0
        assert_eq!(align_up(0, 128), 0);
        assert_eq!(align_up(0, 1), 0);
        assert_eq!(align_up(0, 2), 0);
        assert_eq!(align_up(0, 0x8000_0000_0000_0000), 0);
    }

    #[test]
    #[should_panic]
    fn test_align_up_overflow() {
        // causes buffer overflow when checked_add(1) is called
        align_up(0xffff_ffff_ffff_ffff, 0x1_0000_0000_0000);
    }
}
