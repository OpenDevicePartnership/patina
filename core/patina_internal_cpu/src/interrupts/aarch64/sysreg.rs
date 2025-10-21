// Copyright 2023 The arm-gic Authors.
// This project is dual-licensed under Apache 2.0 and MIT terms.
// See LICENSE-APACHE and LICENSE-MIT for details.

/// Reads and returns the value of the given aarch64 system register.
#[allow(unused_macros)]
macro_rules! read_sysreg {
    ($name:ident) => {
        {
            let mut value: u64;
            ::core::arch::asm!(
                concat!("mrs {value:x}, ", ::core::stringify!($name)),
                value = out(reg) value,
                options(nomem, nostack),
            );
            value
        }
    }
}
#[allow(unused_imports)]
pub(crate) use read_sysreg;

/// Writes the given value to the given aarch64 system register.
#[allow(unused_macros)]
macro_rules! write_sysreg {
    ($name:ident, $value:expr) => {
        {
            // no barrier required case
            let v: u64 = $value;
            ::core::arch::asm!(
                concat!("msr ", ::core::stringify!($name), ", {value:x}"),
                value = in(reg) v,
                options(nomem, nostack),
            )
        }
    };
    ($name:ident, $value:expr, $barrier:expr) => {
        {
            // barrier required case
            let v: u64 = $value;
            ::core::arch::asm!(
                concat!("msr ", ::core::stringify!($name), ", {value:x}"),
                $barrier,
                value = in(reg) v,
                options(nomem, nostack),
            )
        }
    };
}
#[allow(unused_imports)]
pub(crate) use write_sysreg;
