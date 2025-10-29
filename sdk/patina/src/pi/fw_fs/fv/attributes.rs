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

/// Type alias for firmware volume attributes (64-bit)
pub type EfiFvAttributes = u64;

/// EFI_FV_ATTRIBUTES bit definitions
/// Note: `ALIGNMENT` traditionally has the same value as `ALIGNMENT_2G`. To reduce confusion, only
///       `ALIGNMENT_2G` is specified.
pub mod raw {
    /// Firmware volume attribute raw constants (version 2)
    pub mod fv2 {
        /// Capability to disable read operations
        pub const READ_DISABLE_CAP: u64 = 0x0000000000000001;
        /// Capability to enable read operations
        pub const READ_ENABLE_CAP: u64 = 0x0000000000000002;
        /// Current read enable/disable status
        pub const READ_STATUS: u64 = 0x0000000000000004;
        /// Capability to disable write operations
        pub const WRITE_DISABLE_CAP: u64 = 0x0000000000000008;
        /// Capability to enable write operations
        pub const WRITE_ENABLE_CAP: u64 = 0x0000000000000010;
        /// Current write enable/disable status
        pub const WRITE_STATUS: u64 = 0x0000000000000020;
        /// Capability to lock the firmware volume
        pub const LOCK_CAP: u64 = 0x0000000000000040;
        /// Current lock status
        pub const LOCK_STATUS: u64 = 0x0000000000000080;
        /// Reliable write policy - ensures atomic writes
        pub const WRITE_POLICY_RELIABLE: u64 = 0x0000000000000100;
        /// Capability to lock read operations
        pub const READ_LOCK_CAP: u64 = 0x0000000000001000;
        /// Current read lock status
        pub const READ_LOCK_STATUS: u64 = 0x0000000000002000;
        /// Capability to lock write operations
        pub const WRITE_LOCK_CAP: u64 = 0x0000000000004000;
        /// Current write lock status
        pub const WRITE_LOCK_STATUS: u64 = 0x0000000000008000;
        /// No alignment requirement (1-byte alignment)
        pub const ALIGNMENT_1: u64 = 0x0000000000000000;
        /// 2-byte alignment requirement
        pub const ALIGNMENT_2: u64 = 0x0000000000010000;
        /// 4-byte alignment requirement
        pub const ALIGNMENT_4: u64 = 0x0000000000020000;
        /// 8-byte alignment requirement
        pub const ALIGNMENT_8: u64 = 0x0000000000030000;
        /// 16-byte alignment requirement
        pub const ALIGNMENT_16: u64 = 0x0000000000040000;
        /// 32-byte alignment requirement
        pub const ALIGNMENT_32: u64 = 0x0000000000050000;
        /// 64-byte alignment requirement
        pub const ALIGNMENT_64: u64 = 0x0000000000060000;
        /// 128-byte alignment requirement
        pub const ALIGNMENT_128: u64 = 0x0000000000070000;
        /// 256-byte alignment requirement
        pub const ALIGNMENT_256: u64 = 0x0000000000080000;
        /// 512-byte alignment requirement
        pub const ALIGNMENT_512: u64 = 0x0000000000090000;
        /// 1 KB alignment requirement
        pub const ALIGNMENT_1K: u64 = 0x00000000000A0000;
        /// 2 KB alignment requirement
        pub const ALIGNMENT_2K: u64 = 0x00000000000B0000;
        /// 4 KB alignment requirement
        pub const ALIGNMENT_4K: u64 = 0x00000000000C0000;
        /// 8 KB alignment requirement
        pub const ALIGNMENT_8K: u64 = 0x00000000000D0000;
        /// 16 KB alignment requirement
        pub const ALIGNMENT_16K: u64 = 0x00000000000E0000;
        /// 32 KB alignment requirement
        pub const ALIGNMENT_32K: u64 = 0x00000000000F0000;
        /// 64 KB alignment requirement
        pub const ALIGNMENT_64K: u64 = 0x0000000000100000;
        /// 128 KB alignment requirement
        pub const ALIGNMENT_128K: u64 = 0x0000000000110000;
        /// 256 KB alignment requirement
        pub const ALIGNMENT_256K: u64 = 0x0000000000120000;
        /// 512 KB alignment requirement
        pub const ALIGNMENT_512K: u64 = 0x0000000000130000;
        /// 1 MB alignment requirement
        pub const ALIGNMENT_1M: u64 = 0x0000000000140000;
        /// 2 MB alignment requirement
        pub const ALIGNMENT_2M: u64 = 0x0000000000150000;
        /// 4 MB alignment requirement
        pub const ALIGNMENT_4M: u64 = 0x0000000000160000;
        /// 8 MB alignment requirement
        pub const ALIGNMENT_8M: u64 = 0x0000000000170000;
        /// 16 MB alignment requirement
        pub const ALIGNMENT_16M: u64 = 0x0000000000180000;
        /// 32 MB alignment requirement
        pub const ALIGNMENT_32M: u64 = 0x0000000000190000;
        /// 64 MB alignment requirement
        pub const ALIGNMENT_64M: u64 = 0x00000000001A0000;
        /// 128 MB alignment requirement
        pub const ALIGNMENT_128M: u64 = 0x00000000001B0000;
        /// 256 MB alignment requirement
        pub const ALIGNMENT_256M: u64 = 0x00000000001C0000;
        /// 512 MB alignment requirement
        pub const ALIGNMENT_512M: u64 = 0x00000000001D0000;
        /// 1 GB alignment requirement
        pub const ALIGNMENT_1G: u64 = 0x00000000001E0000;
        /// 2 GB alignment requirement
        pub const ALIGNMENT_2G: u64 = 0x00000000001F0000;
    }
}

#[repr(u64)]
#[derive(Debug, Copy, Clone, PartialEq)]
/// Firmware volume attribute enumeration (version 2)
pub enum Fv2 {
    /// Read disable capability
    ReadDisableCap = raw::fv2::READ_DISABLE_CAP,
    /// Read enable capability
    ReadEnableCap = raw::fv2::READ_ENABLE_CAP,
    /// Read status
    ReadStatus = raw::fv2::READ_STATUS,
    /// Write disable capability
    WriteDisableCap = raw::fv2::WRITE_DISABLE_CAP,
    /// Write enable capability
    WriteEnableCap = raw::fv2::WRITE_ENABLE_CAP,
    /// Write status
    WriteStatus = raw::fv2::WRITE_STATUS,
    /// Lock capability
    LockCap = raw::fv2::LOCK_CAP,
    /// Lock status
    LockStatus = raw::fv2::LOCK_STATUS,
    /// Reliable write policy
    WritePolicyReliable = raw::fv2::WRITE_POLICY_RELIABLE,
    /// Read lock capability
    ReadLockCap = raw::fv2::READ_LOCK_CAP,
    /// Read lock status
    ReadLockStatus = raw::fv2::READ_LOCK_STATUS,
    /// Write lock capability
    WriteLockCap = raw::fv2::WRITE_LOCK_CAP,
    /// Write lock status
    WriteLockStatus = raw::fv2::WRITE_LOCK_STATUS,
    /// 1-byte alignment
    Alignment1 = raw::fv2::ALIGNMENT_1,
    /// 2-byte alignment
    Alignment2 = raw::fv2::ALIGNMENT_2,
    /// 4-byte alignment
    Alignment4 = raw::fv2::ALIGNMENT_4,
    /// 8-byte alignment
    Alignment8 = raw::fv2::ALIGNMENT_8,
    /// 16-byte alignment
    Alignment16 = raw::fv2::ALIGNMENT_16,
    /// 32-byte alignment
    Alignment32 = raw::fv2::ALIGNMENT_32,
    /// 64-byte alignment
    Alignment64 = raw::fv2::ALIGNMENT_64,
    /// 128-byte alignment
    Alignment128 = raw::fv2::ALIGNMENT_128,
    /// 256-byte alignment
    Alignment256 = raw::fv2::ALIGNMENT_256,
    /// 512-byte alignment
    Alignment512 = raw::fv2::ALIGNMENT_512,
    /// 1 KB alignment
    Alignment1K = raw::fv2::ALIGNMENT_1K,
    /// 2 KB alignment
    Alignment2K = raw::fv2::ALIGNMENT_2K,
    /// 4 KB alignment
    Alignment4K = raw::fv2::ALIGNMENT_4K,
    /// 8 KB alignment
    Alignment8K = raw::fv2::ALIGNMENT_8K,
    /// 16 KB alignment
    Alignment16K = raw::fv2::ALIGNMENT_16K,
    /// 32 KB alignment
    Alignment32K = raw::fv2::ALIGNMENT_32K,
    /// 64 KB alignment
    Alignment64K = raw::fv2::ALIGNMENT_64K,
    /// 128 KB alignment
    Alignment128K = raw::fv2::ALIGNMENT_128K,
    /// 256 KB alignment
    Alignment256K = raw::fv2::ALIGNMENT_256K,
    /// 512 KB alignment
    Alignment512K = raw::fv2::ALIGNMENT_512K,
    /// 1 MB alignment
    Alignment1M = raw::fv2::ALIGNMENT_1M,
    /// 2 MB alignment
    Alignment2M = raw::fv2::ALIGNMENT_2M,
    /// 4 MB alignment
    Alignment4M = raw::fv2::ALIGNMENT_4M,
    /// 8 MB alignment
    Alignment8M = raw::fv2::ALIGNMENT_8M,
    /// 16 MB alignment
    Alignment16M = raw::fv2::ALIGNMENT_16M,
    /// 32 MB alignment
    Alignment32M = raw::fv2::ALIGNMENT_32M,
    /// 64 MB alignment
    Alignment64M = raw::fv2::ALIGNMENT_64M,
    /// 128 MB alignment
    Alignment128M = raw::fv2::ALIGNMENT_128M,
    /// 256 MB alignment
    Alignment256M = raw::fv2::ALIGNMENT_256M,
    /// 512 MB alignment
    Alignment512M = raw::fv2::ALIGNMENT_512M,
    /// 1 GB alignment
    Alignment1G = raw::fv2::ALIGNMENT_1G,
    /// 2 GB alignment
    Alignment2G = raw::fv2::ALIGNMENT_2G,
}
