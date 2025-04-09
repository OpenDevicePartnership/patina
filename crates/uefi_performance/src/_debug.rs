//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use core::{fmt::Debug, mem};

pub struct DbgMemory<'a>(pub &'a [u8]);

impl Debug for DbgMemory<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        const IDENT: &str = "    ";
        const N: usize = 4;

        let is_pretty = f.alternate();
        write!(f, "[")?;
        if is_pretty {
            writeln!(f)?
        }

        for (i, b) in self.0.iter().enumerate() {
            match i {
                _ if is_pretty && i % mem::size_of::<usize>() == 0 => write!(f, "{IDENT}")?,
                0 => (),
                _ => write!(f, " ")?,
            }
            write!(f, "{b:02X}")?;
            match i {
                1.. if is_pretty && (i + 1) % (mem::size_of::<usize>() * N) == 0 => writeln!(f)?,
                _ => (),
            }
        }

        if is_pretty {
            writeln!(f)?;
        }
        write!(f, "]")?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use core::assert_eq;

    use alloc::{fmt::format, vec};

    use super::*;

    #[test]
    fn test_debug_memory_formating() {
        let mut bytes = vec![];
        for i in 0..12 {
            bytes.push(i);
        }

        let dbg_str = format!("{:?}", &DbgMemory(bytes.as_slice()));
        assert_eq!("[00 01 02 03 04 05 06 07 08 09 0A 0B]", dbg_str);
    }

    #[test]
    fn test_debug_memory_alternate_formating() {
        let mut bytes = vec![];
        for i in 0..100 {
            bytes.push(i);
        }

        let dbg_str = format!("{:#?}", &DbgMemory(bytes.as_slice()));
        let expected = "[\n\
                              \x20   00 01 02 03 04 05 06 07    08 09 0A 0B 0C 0D 0E 0F    10 11 12 13 14 15 16 17    18 19 1A 1B 1C 1D 1E 1F\n\
                              \x20   20 21 22 23 24 25 26 27    28 29 2A 2B 2C 2D 2E 2F    30 31 32 33 34 35 36 37    38 39 3A 3B 3C 3D 3E 3F\n\
                              \x20   40 41 42 43 44 45 46 47    48 49 4A 4B 4C 4D 4E 4F    50 51 52 53 54 55 56 57    58 59 5A 5B 5C 5D 5E 5F\n\
                              \x20   60 61 62 63\n\
                              ]";
        assert_eq!(expected, dbg_str);
    }
}