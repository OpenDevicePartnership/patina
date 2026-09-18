//! Locates the Firmware Basic Boot Performance Table (FBPT) allocated during the previous boot.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::vec::Vec;
use core::{mem, ptr};

use patina::{BinaryGuid, Char16Array, uefi::runtime_services::RuntimeServices};

/// Return the address where the FBPT has been allocated during the previous boot.
pub(crate) fn find_previous_table_address(runtime_services: &impl RuntimeServices) -> Option<usize> {
    runtime_services
        .get_variable::<FirmwarePerformanceVariable>(
            &FirmwarePerformanceVariable::ADDRESS_VARIABLE_NAME,
            &FirmwarePerformanceVariable::ADDRESS_VARIABLE_GUID,
            Some(mem::size_of::<FirmwarePerformanceVariable>()),
        )
        .map(|(v, _)| v.boot_performance_table_pointer)
        .ok()
}

/// Struct used to get the value from the `FirmwarePerformanceVariable`
#[repr(C, packed)]
pub(crate) struct FirmwarePerformanceVariable {
    boot_performance_table_pointer: usize,
    _s3_performance_table_pointer: usize,
}

impl FirmwarePerformanceVariable {
    const ADDRESS_VARIABLE_NAME: Char16Array<20> = Char16Array::from_str("FirmwarePerformance");
    const ADDRESS_VARIABLE_GUID: BinaryGuid = BinaryGuid::from_string("C095791A-3001-47B2-80C9-EAC7319F2FA4");
}

impl TryFrom<Vec<u8>> for FirmwarePerformanceVariable {
    type Error = ();

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        if value.len() == mem::size_of::<Self>() {
            // SAFETY: This is safe because the value for ADDRESS_VARIABLE_GUID is an address where a FirmwarePerformanceVariable is.
            Ok(unsafe { ptr::read_unaligned(value.as_ptr().cast::<FirmwarePerformanceVariable>()) })
        } else {
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patina::uefi::runtime_services::MockRuntimeServices;

    #[test]
    fn test_find_previous_address() {
        let mut runtime_services = MockRuntimeServices::new();

        runtime_services
            .expect_get_variable::<FirmwarePerformanceVariable>()
            .once()
            .withf(|name, namespace, size_hint| {
                assert_eq!(FirmwarePerformanceVariable::ADDRESS_VARIABLE_NAME.as_char16_str(), name);
                assert_eq!(&FirmwarePerformanceVariable::ADDRESS_VARIABLE_GUID, namespace);
                assert_eq!(&Some(16), size_hint);
                true
            })
            .returning(|_, _, _| {
                Ok((
                    FirmwarePerformanceVariable {
                        boot_performance_table_pointer: 0x12341234,
                        _s3_performance_table_pointer: 0,
                    },
                    16,
                ))
            });

        let address = find_previous_table_address(&runtime_services);

        assert_eq!(Some(0x12341234), address);
    }
}
