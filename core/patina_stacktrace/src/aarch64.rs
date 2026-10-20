pub(crate) mod runtime_function;
mod unwind;

pub(crate) fn is_valid_address(address: u64) -> bool {
    #[cfg(all(target_arch = "aarch64", target_os = "uefi"))]
    let address_bits = match patina::read_sysreg!(ID_AA64MMFR2_EL1) & 0xf {
        0 => 48,
        1 => 52,
        _ => return false,
    };
    #[cfg(not(all(target_arch = "aarch64", target_os = "uefi")))]
    let address_bits = 48;

    address_fits_width(address, address_bits)
}

fn address_fits_width(address: u64, address_bits: u32) -> bool {
    let mask = u64::MAX << address_bits;
    address & mask == 0 || address & mask == mask
}

#[cfg(test)]
mod tests {
    use super::address_fits_width;

    #[test]
    fn address_width_boundaries() {
        for bits in [48, 52] {
            let mask = u64::MAX << bits;
            assert!(address_fits_width(0, bits));
            assert!(address_fits_width(!mask, bits));
            assert!(address_fits_width(mask, bits));
            assert!(address_fits_width(u64::MAX, bits));
            assert!(!address_fits_width(1 << bits, bits));
            assert!(!address_fits_width(mask - 1, bits));
            assert!(!address_fits_width(0x5050_4120_544f_4f42, bits));
        }
        assert!(!address_fits_width(1 << 48, 48));
        assert!(address_fits_width(1 << 48, 52));
    }
}
