//! Firmware Volume Attributes
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

pub type EfiFvAttributes = u64;

/// EFI_FV_ATTRIBUTES bit definitions
/// Note: `ALIGNMENT` traditionally has the same value as `ALIGNMENT_2G`. To reduce confusion, only
///       `ALIGNMENT_2G` is specified.
pub mod raw {
    pub mod fv2 {
        pub const READ_DISABLE_CAP: u64 = 0x0000000000000001;
        pub const READ_ENABLE_CAP: u64 = 0x0000000000000002;
        pub const READ_STATUS: u64 = 0x0000000000000004;
        pub const WRITE_DISABLE_CAP: u64 = 0x0000000000000008;
        pub const WRITE_ENABLE_CAP: u64 = 0x0000000000000010;
        pub const WRITE_STATUS: u64 = 0x0000000000000020;
        pub const LOCK_CAP: u64 = 0x0000000000000040;
        pub const LOCK_STATUS: u64 = 0x0000000000000080;
        pub const WRITE_POLICY_RELIABLE: u64 = 0x0000000000000100;
        pub const READ_LOCK_CAP: u64 = 0x0000000000001000;
        pub const READ_LOCK_STATUS: u64 = 0x0000000000002000;
        pub const WRITE_LOCK_CAP: u64 = 0x0000000000004000;
        pub const WRITE_LOCK_STATUS: u64 = 0x0000000000008000;
        pub const ALIGNMENT_1: u64 = 0x0000000000000000;
        pub const ALIGNMENT_2: u64 = 0x0000000000010000;
        pub const ALIGNMENT_4: u64 = 0x0000000000020000;
        pub const ALIGNMENT_8: u64 = 0x0000000000030000;
        pub const ALIGNMENT_16: u64 = 0x0000000000040000;
        pub const ALIGNMENT_32: u64 = 0x0000000000050000;
        pub const ALIGNMENT_64: u64 = 0x0000000000060000;
        pub const ALIGNMENT_128: u64 = 0x0000000000070000;
        pub const ALIGNMENT_256: u64 = 0x0000000000080000;
        pub const ALIGNMENT_512: u64 = 0x0000000000090000;
        pub const ALIGNMENT_1K: u64 = 0x00000000000A0000;
        pub const ALIGNMENT_2K: u64 = 0x00000000000B0000;
        pub const ALIGNMENT_4K: u64 = 0x00000000000C0000;
        pub const ALIGNMENT_8K: u64 = 0x00000000000D0000;
        pub const ALIGNMENT_16K: u64 = 0x00000000000E0000;
        pub const ALIGNMENT_32K: u64 = 0x00000000000F0000;
        pub const ALIGNMENT_64K: u64 = 0x0000000000100000;
        pub const ALIGNMENT_128K: u64 = 0x0000000000110000;
        pub const ALIGNMENT_256K: u64 = 0x0000000000120000;
        pub const ALIGNMENT_512K: u64 = 0x0000000000130000;
        pub const ALIGNMENT_1M: u64 = 0x0000000000140000;
        pub const ALIGNMENT_2M: u64 = 0x0000000000150000;
        pub const ALIGNMENT_4M: u64 = 0x0000000000160000;
        pub const ALIGNMENT_8M: u64 = 0x0000000000170000;
        pub const ALIGNMENT_16M: u64 = 0x0000000000180000;
        pub const ALIGNMENT_32M: u64 = 0x0000000000190000;
        pub const ALIGNMENT_64M: u64 = 0x00000000001A0000;
        pub const ALIGNMENT_128M: u64 = 0x00000000001B0000;
        pub const ALIGNMENT_256M: u64 = 0x00000000001C0000;
        pub const ALIGNMENT_512M: u64 = 0x00000000001D0000;
        pub const ALIGNMENT_1G: u64 = 0x00000000001E0000;
        pub const ALIGNMENT_2G: u64 = 0x00000000001F0000;
    }
}

#[repr(u64)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Fv2 {
    ReadDisableCap = raw::fv2::READ_DISABLE_CAP,
    ReadEnableCap = raw::fv2::READ_ENABLE_CAP,
    ReadStatus = raw::fv2::READ_STATUS,
    WriteDisableCap = raw::fv2::WRITE_DISABLE_CAP,
    WriteEnableCap = raw::fv2::WRITE_ENABLE_CAP,
    WriteStatus = raw::fv2::WRITE_STATUS,
    LockCap = raw::fv2::LOCK_CAP,
    LockStatus = raw::fv2::LOCK_STATUS,
    WritePolicyReliable = raw::fv2::WRITE_POLICY_RELIABLE,
    ReadLockCap = raw::fv2::READ_LOCK_CAP,
    ReadLockStatus = raw::fv2::READ_LOCK_STATUS,
    WriteLockCap = raw::fv2::WRITE_LOCK_CAP,
    WriteLockStatus = raw::fv2::WRITE_LOCK_STATUS,
    Alignment1 = raw::fv2::ALIGNMENT_1,
    Alignment2 = raw::fv2::ALIGNMENT_2,
    Alignment4 = raw::fv2::ALIGNMENT_4,
    Alignment8 = raw::fv2::ALIGNMENT_8,
    Alignment16 = raw::fv2::ALIGNMENT_16,
    Alignment32 = raw::fv2::ALIGNMENT_32,
    Alignment64 = raw::fv2::ALIGNMENT_64,
    Alignment128 = raw::fv2::ALIGNMENT_128,
    Alignment256 = raw::fv2::ALIGNMENT_256,
    Alignment512 = raw::fv2::ALIGNMENT_512,
    Alignment1K = raw::fv2::ALIGNMENT_1K,
    Alignment2K = raw::fv2::ALIGNMENT_2K,
    Alignment4K = raw::fv2::ALIGNMENT_4K,
    Alignment8K = raw::fv2::ALIGNMENT_8K,
    Alignment16K = raw::fv2::ALIGNMENT_16K,
    Alignment32K = raw::fv2::ALIGNMENT_32K,
    Alignment64K = raw::fv2::ALIGNMENT_64K,
    Alignment128K = raw::fv2::ALIGNMENT_128K,
    Alignment256K = raw::fv2::ALIGNMENT_256K,
    Alignment512K = raw::fv2::ALIGNMENT_512K,
    Alignment1M = raw::fv2::ALIGNMENT_1M,
    Alignment2M = raw::fv2::ALIGNMENT_2M,
    Alignment4M = raw::fv2::ALIGNMENT_4M,
    Alignment8M = raw::fv2::ALIGNMENT_8M,
    Alignment16M = raw::fv2::ALIGNMENT_16M,
    Alignment32M = raw::fv2::ALIGNMENT_32M,
    Alignment64M = raw::fv2::ALIGNMENT_64M,
    Alignment128M = raw::fv2::ALIGNMENT_128M,
    Alignment256M = raw::fv2::ALIGNMENT_256M,
    Alignment512M = raw::fv2::ALIGNMENT_512M,
    Alignment1G = raw::fv2::ALIGNMENT_1G,
    Alignment2G = raw::fv2::ALIGNMENT_2G,
}
