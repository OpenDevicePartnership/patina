//! UEFI PE/COFF Errors
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
pub type Result<T> = core::result::Result<T, Error>;

/// Type for describing errors that result from working with PeCoff images.
#[derive(Debug)]
#[allow(dead_code)]
pub enum Error {
    /// Goblin failed to parse the PE32 image.
    ///
    /// See the enclosed goblin error for a reason why the parsing failed.
    Goblin(goblin::error::Error),
    BufferTooShort(usize, &'static str),
    Parse(scroll::Error),
    BadSignature(u16),
    /// The parsed PeCoff image does not contain an Optional Header.
    NoOptionalHeader,
}

impl From<scroll::Error> for Error {
    fn from(e: scroll::Error) -> Self {
        Error::Parse(e)
    }
}

impl From<goblin::error::Error> for Error {
    fn from(e: goblin::error::Error) -> Self {
        Error::Goblin(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern crate alloc;
    extern crate scroll;
    extern crate std;

    use alloc::string::ToString;
    use std::format;

    #[test]
    fn test_convert_error() {
        let goblin_error = goblin::error::Error::Malformed("test".to_string());
        let e: Error = goblin_error.into();
        assert_eq!(format!("{:?}", e), "Goblin(Malformed(\"test\"))");

        let scroll_error = scroll::Error::TooBig { size: 50, len: 40 };
        let e: Error = scroll_error.into();
        assert_eq!(format!("{:?}", e), "Parse(TooBig { size: 50, len: 40 })");
    }
}
