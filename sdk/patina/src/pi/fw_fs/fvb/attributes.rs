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

/// Type alias for firmware volume block attributes (version 2) as defined in the PI Specification
pub type EfiFvbAttributes2 = u32;

/// EFI_FV_FILE_ATTRIBUTES bit definitions
/// Note: Typically named `EFI_FVB2_*` in EDK II code.
/// `ALIGNMENT` traditionally has the same value as `ALIGNMENT_2G`. To reduce confusion, only
/// `ALIGNMENT_2G` is specified.
pub mod raw {
    /// Raw FVB2 attribute constant definitions
    pub mod fvb2 {
        /// Capability to disable read operations
        pub const READ_DISABLED_CAP: u32 = 0x00000001;
        /// Capability to enable read operations
        pub const READ_ENABLED_CAP: u32 = 0x00000002;
        /// Current read enable/disable status
        pub const READ_STATUS: u32 = 0x00000004;
        /// Capability to disable write operations
        pub const WRITE_DISABLED_CAP: u32 = 0x00000008;
        /// Capability to enable write operations
        pub const WRITE_ENABLED_CAP: u32 = 0x00000010;
        /// Current write enable/disable status
        pub const WRITE_STATUS: u32 = 0x00000020;
        /// Capability to lock the firmware volume block
        pub const LOCK_CAP: u32 = 0x00000040;
        /// Current lock status
        pub const LOCK_STATUS: u32 = 0x00000080;
        /// Sticky write attribute - data persists across resets
        pub const STICKY_WRITE: u32 = 0x00000200;
        /// Block can be memory-mapped
        pub const MEMORY_MAPPED: u32 = 0x00000400;
        /// Erase polarity bit - indicates value of erased bits
        pub const ERASE_POLARITY: u32 = 0x00000800;
        /// Capability to lock read operations
        pub const READ_LOCK_CAP: u32 = 0x00001000;
        /// Current read lock status
        pub const READ_LOCK_STATUS: u32 = 0x00002000;
        /// Capability to lock write operations
        pub const WRITE_LOCK_CAP: u32 = 0x00004000;
        /// Current write lock status
        pub const WRITE_LOCK_STATUS: u32 = 0x00008000;
        /// No alignment requirement (1-byte alignment)
        pub const ALIGNMENT_1: u32 = 0x00000000;
        /// 2-byte alignment requirement
        pub const ALIGNMENT_2: u32 = 0x00010000;
        /// 4-byte alignment requirement
        pub const ALIGNMENT_4: u32 = 0x00020000;
        /// 8-byte alignment requirement
        pub const ALIGNMENT_8: u32 = 0x00030000;
        /// 16-byte alignment requirement
        pub const ALIGNMENT_16: u32 = 0x00040000;
        /// 32-byte alignment requirement
        pub const ALIGNMENT_32: u32 = 0x00050000;
        /// 64-byte alignment requirement
        pub const ALIGNMENT_64: u32 = 0x00060000;
        /// 128-byte alignment requirement
        pub const ALIGNMENT_128: u32 = 0x00070000;
        /// 256-byte alignment requirement
        pub const ALIGNMENT_256: u32 = 0x00080000;
        /// 512-byte alignment requirement
        pub const ALIGNMENT_512: u32 = 0x00090000;
        /// 1 KB alignment requirement
        pub const ALIGNMENT_1K: u32 = 0x000A0000;
        /// 2 KB alignment requirement
        pub const ALIGNMENT_2K: u32 = 0x000B0000;
        /// 4 KB alignment requirement
        pub const ALIGNMENT_4K: u32 = 0x000C0000;
        /// 8 KB alignment requirement
        pub const ALIGNMENT_8K: u32 = 0x000D0000;
        /// 16 KB alignment requirement
        pub const ALIGNMENT_16K: u32 = 0x000E0000;
        /// 32 KB alignment requirement
        pub const ALIGNMENT_32K: u32 = 0x000F0000;
        /// 64 KB alignment requirement
        pub const ALIGNMENT_64K: u32 = 0x00100000;
        /// 128 KB alignment requirement
        pub const ALIGNMENT_128K: u32 = 0x00110000;
        /// 256 KB alignment requirement
        pub const ALIGNMENT_256K: u32 = 0x00120000;
        /// 512 KB alignment requirement
        pub const ALIGNMENT_512K: u32 = 0x00130000;
        /// 1 MB alignment requirement
        pub const ALIGNMENT_1M: u32 = 0x00140000;
        /// 2 MB alignment requirement
        pub const ALIGNMENT_2M: u32 = 0x00150000;
        /// 4 MB alignment requirement
        pub const ALIGNMENT_4M: u32 = 0x00160000;
        /// 8 MB alignment requirement
        pub const ALIGNMENT_8M: u32 = 0x00170000;
        /// 16 MB alignment requirement
        pub const ALIGNMENT_16M: u32 = 0x00180000;
        /// 32 MB alignment requirement
        pub const ALIGNMENT_32M: u32 = 0x00190000;
        /// 64 MB alignment requirement
        pub const ALIGNMENT_64M: u32 = 0x001A0000;
        /// 128 MB alignment requirement
        pub const ALIGNMENT_128M: u32 = 0x001B0000;
        /// 256 MB alignment requirement
        pub const ALIGNMENT_256M: u32 = 0x001C0000;
        /// 512 MB alignment requirement
        pub const ALIGNMENT_512M: u32 = 0x001D0000;
        /// 1 GB alignment requirement
        pub const ALIGNMENT_1G: u32 = 0x001E0000;
        /// 2 GB alignment requirement
        pub const ALIGNMENT_2G: u32 = 0x001F0000;
        /// Weak alignment - less strict alignment requirements
        pub const WEAK_ALIGNMENT: u32 = 0x80000000;
    }
}

#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq)]
/// Firmware Volume Block attribute enumeration (version 2)
pub enum Fvb2 {
    /// Read disable capability
    ReadDisableCap = raw::fvb2::READ_DISABLED_CAP,
    /// Read enable capability
    ReadEnableCap = raw::fvb2::READ_ENABLED_CAP,
    /// Read status
    ReadStatus = raw::fvb2::READ_STATUS,
    /// Write disable capability
    WriteDisableCap = raw::fvb2::WRITE_DISABLED_CAP,
    /// Write enable capability
    WriteEnableCap = raw::fvb2::WRITE_ENABLED_CAP,
    /// Write status
    WriteStatus = raw::fvb2::WRITE_STATUS,
    /// Lock capability
    LockCap = raw::fvb2::LOCK_CAP,
    /// Lock status
    LockStatus = raw::fvb2::LOCK_STATUS,
    /// Sticky write - data persists across resets
    StickyWrite = raw::fvb2::STICKY_WRITE,
    /// Memory-mapped block
    MemoryMapped = raw::fvb2::MEMORY_MAPPED,
    /// Erase polarity
    ErasePolarity = raw::fvb2::ERASE_POLARITY,
    /// Read lock capability
    ReadLockCap = raw::fvb2::READ_LOCK_CAP,
    /// Read lock status
    ReadLockStatus = raw::fvb2::READ_LOCK_STATUS,
    /// Write lock capability
    WriteLockCap = raw::fvb2::WRITE_LOCK_CAP,
    /// Write lock status
    WriteLockStatus = raw::fvb2::WRITE_LOCK_STATUS,
    /// 1-byte alignment
    Alignment1 = raw::fvb2::ALIGNMENT_1,
    /// 2-byte alignment
    Alignment2 = raw::fvb2::ALIGNMENT_2,
    /// 4-byte alignment
    Alignment4 = raw::fvb2::ALIGNMENT_4,
    /// 8-byte alignment
    Alignment8 = raw::fvb2::ALIGNMENT_8,
    /// 16-byte alignment
    Alignment16 = raw::fvb2::ALIGNMENT_16,
    /// 32-byte alignment
    Alignment32 = raw::fvb2::ALIGNMENT_32,
    /// 64-byte alignment
    Alignment64 = raw::fvb2::ALIGNMENT_64,
    /// 128-byte alignment
    Alignment128 = raw::fvb2::ALIGNMENT_128,
    /// 256-byte alignment
    Alignment256 = raw::fvb2::ALIGNMENT_256,
    /// 512-byte alignment
    Alignment512 = raw::fvb2::ALIGNMENT_512,
    /// 1 KB alignment
    Alignment1K = raw::fvb2::ALIGNMENT_1K,
    /// 2 KB alignment
    Alignment2K = raw::fvb2::ALIGNMENT_2K,
    /// 4 KB alignment
    Alignment4K = raw::fvb2::ALIGNMENT_4K,
    /// 8 KB alignment
    Alignment8K = raw::fvb2::ALIGNMENT_8K,
    /// 16 KB alignment
    Alignment16K = raw::fvb2::ALIGNMENT_16K,
    /// 32 KB alignment
    Alignment32K = raw::fvb2::ALIGNMENT_32K,
    /// 64 KB alignment
    Alignment64K = raw::fvb2::ALIGNMENT_64K,
    /// 128 KB alignment
    Alignment128K = raw::fvb2::ALIGNMENT_128K,
    /// 256 KB alignment
    Alignment256K = raw::fvb2::ALIGNMENT_256K,
    /// 512 KB alignment
    Alignment512K = raw::fvb2::ALIGNMENT_512K,
    /// 1 MB alignment
    Alignment1M = raw::fvb2::ALIGNMENT_1M,
    /// 2 MB alignment
    Alignment2M = raw::fvb2::ALIGNMENT_2M,
    /// 4 MB alignment
    Alignment4M = raw::fvb2::ALIGNMENT_4M,
    /// 8 MB alignment
    Alignment8M = raw::fvb2::ALIGNMENT_8M,
    /// 16 MB alignment
    Alignment16M = raw::fvb2::ALIGNMENT_16M,
    /// 32 MB alignment
    Alignment32M = raw::fvb2::ALIGNMENT_32M,
    /// 64 MB alignment
    Alignment64M = raw::fvb2::ALIGNMENT_64M,
    /// 128 MB alignment
    Alignment128M = raw::fvb2::ALIGNMENT_128M,
    /// 256 MB alignment
    Alignment256M = raw::fvb2::ALIGNMENT_256M,
    /// 512 MB alignment
    Alignment512M = raw::fvb2::ALIGNMENT_512M,
    /// 1 GB alignment
    Alignment1G = raw::fvb2::ALIGNMENT_1G,
    /// 2 GB alignment
    Alignment2G = raw::fvb2::ALIGNMENT_2G,
    /// Weak alignment - less strict requirements
    WeakAlignment = raw::fvb2::WEAK_ALIGNMENT,
}
