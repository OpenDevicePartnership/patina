pub(crate) mod runtime_function;
mod unwind;

fn is_valid_address(address: u64) -> bool {
    #[cfg(all(target_arch = "x86_64", target_os = "uefi"))]
    let address_bits = patina::arch::x64::virtual_address_bits();
    #[cfg(not(all(target_arch = "x86_64", target_os = "uefi")))]
    let address_bits = 48;

    address_fits_width(address, address_bits)
}

fn address_fits_width(address: u64, address_bits: u32) -> bool {
    let mask = u64::MAX << (address_bits - 1);
    address & mask == 0 || address & mask == mask
}

#[cfg(test)]
mod address_tests {
    use super::address_fits_width;

    #[test]
    fn canonical_address_boundaries() {
        for bits in [48, 57] {
            let mask = u64::MAX << (bits - 1);
            assert!(address_fits_width(0, bits));
            assert!(address_fits_width(!mask, bits));
            assert!(address_fits_width(mask, bits));
            assert!(address_fits_width(u64::MAX, bits));
            assert!(!address_fits_width(1 << (bits - 1), bits));
            assert!(!address_fits_width(mask - 1, bits));
            assert!(!address_fits_width(0x5050_4120_544f_4f42, bits));
        }
        assert!(!address_fits_width(0x00ff_ffff_ffff_ffff, 48));
        assert!(address_fits_width(0x00ff_ffff_ffff_ffff, 57));
        assert!(!address_fits_width(0xff00_0000_0000_0000, 48));
        assert!(address_fits_width(0xff00_0000_0000_0000, 57));
    }
}
