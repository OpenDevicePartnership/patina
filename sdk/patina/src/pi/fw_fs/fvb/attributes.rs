//! Firmware Volume Block Attributes
//!
//! Based on the values defined in the UEFI Platform Initialization (PI) Specification V1.8A Section 3.2.1.1
//! EFI_FIRMWARE_VOLUME_HEADER.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub type EfiFvbAttributes2 = u32;

/// EFI_FV_FILE_ATTRIBUTES bit definitions
/// Note: Typically named `EFI_FVB2_*` in EDK II code.
/// `ALIGNMENT` traditionally has the same value as `ALIGNMENT_2G`. To reduce confusion, only
/// `ALIGNMENT_2G` is specified.
pub mod raw {
    pub mod fvb2 {
        pub const READ_DISABLED_CAP: u32 = 0x00000001;
        pub const READ_ENABLED_CAP: u32 = 0x00000002;
        pub const READ_STATUS: u32 = 0x00000004;
        pub const WRITE_DISABLED_CAP: u32 = 0x00000008;
        pub const WRITE_ENABLED_CAP: u32 = 0x00000010;
        pub const WRITE_STATUS: u32 = 0x00000020;
        pub const LOCK_CAP: u32 = 0x00000040;
        pub const LOCK_STATUS: u32 = 0x00000080;
        pub const STICKY_WRITE: u32 = 0x00000200;
        pub const MEMORY_MAPPED: u32 = 0x00000400;
        pub const ERASE_POLARITY: u32 = 0x00000800;
        pub const READ_LOCK_CAP: u32 = 0x00001000;
        pub const READ_LOCK_STATUS: u32 = 0x00002000;
        pub const WRITE_LOCK_CAP: u32 = 0x00004000;
        pub const WRITE_LOCK_STATUS: u32 = 0x00008000;
        pub const ALIGNMENT_1: u32 = 0x00000000;
        pub const ALIGNMENT_2: u32 = 0x00010000;
        pub const ALIGNMENT_4: u32 = 0x00020000;
        pub const ALIGNMENT_8: u32 = 0x00030000;
        pub const ALIGNMENT_16: u32 = 0x00040000;
        pub const ALIGNMENT_32: u32 = 0x00050000;
        pub const ALIGNMENT_64: u32 = 0x00060000;
        pub const ALIGNMENT_128: u32 = 0x00070000;
        pub const ALIGNMENT_256: u32 = 0x00080000;
        pub const ALIGNMENT_512: u32 = 0x00090000;
        pub const ALIGNMENT_1K: u32 = 0x000A0000;
        pub const ALIGNMENT_2K: u32 = 0x000B0000;
        pub const ALIGNMENT_4K: u32 = 0x000C0000;
        pub const ALIGNMENT_8K: u32 = 0x000D0000;
        pub const ALIGNMENT_16K: u32 = 0x000E0000;
        pub const ALIGNMENT_32K: u32 = 0x000F0000;
        pub const ALIGNMENT_64K: u32 = 0x00100000;
        pub const ALIGNMENT_128K: u32 = 0x00110000;
        pub const ALIGNMENT_256K: u32 = 0x00120000;
        pub const ALIGNMENT_512K: u32 = 0x00130000;
        pub const ALIGNMENT_1M: u32 = 0x00140000;
        pub const ALIGNMENT_2M: u32 = 0x00150000;
        pub const ALIGNMENT_4M: u32 = 0x00160000;
        pub const ALIGNMENT_8M: u32 = 0x00170000;
        pub const ALIGNMENT_16M: u32 = 0x00180000;
        pub const ALIGNMENT_32M: u32 = 0x00190000;
        pub const ALIGNMENT_64M: u32 = 0x001A0000;
        pub const ALIGNMENT_128M: u32 = 0x001B0000;
        pub const ALIGNMENT_256M: u32 = 0x001C0000;
        pub const ALIGNMENT_512M: u32 = 0x001D0000;
        pub const ALIGNMENT_1G: u32 = 0x001E0000;
        pub const ALIGNMENT_2G: u32 = 0x001F0000;
        pub const WEAK_ALIGNMENT: u32 = 0x80000000;
    }
}

#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Fvb2 {
    ReadDisableCap = raw::fvb2::READ_DISABLED_CAP,
    ReadEnableCap = raw::fvb2::READ_ENABLED_CAP,
    ReadStatus = raw::fvb2::READ_STATUS,
    WriteDisableCap = raw::fvb2::WRITE_DISABLED_CAP,
    WriteEnableCap = raw::fvb2::WRITE_ENABLED_CAP,
    WriteStatus = raw::fvb2::WRITE_STATUS,
    LockCap = raw::fvb2::LOCK_CAP,
    LockStatus = raw::fvb2::LOCK_STATUS,
    StickyWrite = raw::fvb2::STICKY_WRITE,
    MemoryMapped = raw::fvb2::MEMORY_MAPPED,
    ErasePolarity = raw::fvb2::ERASE_POLARITY,
    ReadLockCap = raw::fvb2::READ_LOCK_CAP,
    ReadLockStatus = raw::fvb2::READ_LOCK_STATUS,
    WriteLockCap = raw::fvb2::WRITE_LOCK_CAP,
    WriteLockStatus = raw::fvb2::WRITE_LOCK_STATUS,
    Alignment1 = raw::fvb2::ALIGNMENT_1,
    Alignment2 = raw::fvb2::ALIGNMENT_2,
    Alignment4 = raw::fvb2::ALIGNMENT_4,
    Alignment8 = raw::fvb2::ALIGNMENT_8,
    Alignment16 = raw::fvb2::ALIGNMENT_16,
    Alignment32 = raw::fvb2::ALIGNMENT_32,
    Alignment64 = raw::fvb2::ALIGNMENT_64,
    Alignment128 = raw::fvb2::ALIGNMENT_128,
    Alignment256 = raw::fvb2::ALIGNMENT_256,
    Alignment512 = raw::fvb2::ALIGNMENT_512,
    Alignment1K = raw::fvb2::ALIGNMENT_1K,
    Alignment2K = raw::fvb2::ALIGNMENT_2K,
    Alignment4K = raw::fvb2::ALIGNMENT_4K,
    Alignment8K = raw::fvb2::ALIGNMENT_8K,
    Alignment16K = raw::fvb2::ALIGNMENT_16K,
    Alignment32K = raw::fvb2::ALIGNMENT_32K,
    Alignment64K = raw::fvb2::ALIGNMENT_64K,
    Alignment128K = raw::fvb2::ALIGNMENT_128K,
    Alignment256K = raw::fvb2::ALIGNMENT_256K,
    Alignment512K = raw::fvb2::ALIGNMENT_512K,
    Alignment1M = raw::fvb2::ALIGNMENT_1M,
    Alignment2M = raw::fvb2::ALIGNMENT_2M,
    Alignment4M = raw::fvb2::ALIGNMENT_4M,
    Alignment8M = raw::fvb2::ALIGNMENT_8M,
    Alignment16M = raw::fvb2::ALIGNMENT_16M,
    Alignment32M = raw::fvb2::ALIGNMENT_32M,
    Alignment64M = raw::fvb2::ALIGNMENT_64M,
    Alignment128M = raw::fvb2::ALIGNMENT_128M,
    Alignment256M = raw::fvb2::ALIGNMENT_256M,
    Alignment512M = raw::fvb2::ALIGNMENT_512M,
    Alignment1G = raw::fvb2::ALIGNMENT_1G,
    Alignment2G = raw::fvb2::ALIGNMENT_2G,
    WeakAlignment = raw::fvb2::WEAK_ALIGNMENT,
}
