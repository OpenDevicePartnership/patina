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
            write!(f, "{b:02x}")?;
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
