//! UEFI Advanced Logger Memory Log Support
//!
//! This module provides definitions for core Advanced Logger memory log structures.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

// This file contains core structures used by different modules for interacting with the advanced logger. Different feature
// sets will use different functions. For this reason, allow unused code in this module.
#![allow(dead_code)]

use core::{
    mem::size_of,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
};
use patina::{
    base::align_up,
    error::{EfiError, Result},
};
use r_efi::efi;
use zerocopy_derive::*;

// { 0x4d60cfb5, 0xf481, 0x4a98, {0x9c, 0x81, 0xbf, 0xf8, 0x64, 0x60, 0xc4, 0x3e }}
pub const ADV_LOGGER_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("4D60CFB5-F481-4A98-9C81-BFF86460C43E");

pub const ADV_LOGGER_INFO_VERSION_V5: u16 = 5;
pub const ADV_LOGGER_INFO_VERSION_V6: u16 = 6;

// UEFI Debug Levels
pub const DEBUG_LEVEL_ERROR: u32 = 0x80000000;
pub const DEBUG_LEVEL_WARNING: u32 = 0x00000002;
pub const DEBUG_LEVEL_INFO: u32 = 0x00000040;
pub const DEBUG_LEVEL_VERBOSE: u32 = 0x00400000;

// Phase definitions.
pub const ADVANCED_LOGGER_PHASE_DXE: u16 = 4;

/// A struct for carrying log entry both as input and output to this module.
/// This struct contains the key information for the log entry, but excludes the
/// log entry specifics that are not needed by generic code.
#[derive(Clone, Copy)]
pub struct LogEntry<'a> {
    pub phase: u16,
    pub level: u32,
    pub timestamp: u64,
    pub data: &'a [u8],
}

impl<'a> LogEntry<'a> {
    /// Returns the message data as a slice.
    pub fn get_message(&self) -> &'a [u8] {
        self.data
    }
}

/// Implementation of the C struct ADVANCED_LOGGER_INFO for tracking in-memory
/// logging structure for Advanced Logger.
#[derive(Debug)]
#[repr(C)]
pub(crate) struct AdvLoggerInfoV5 {
    /// Signature 'ALOG'
    signature: u32,
    /// Current Version
    pub(crate) version: u16,
    /// Reserved for future
    reserved1: [u16; 3],
    /// Offset from LoggerInfo to start of log, expected to be the size of this structure 8 byte aligned
    pub(crate) log_buffer_offset: u32,
    /// Reserved for future
    reserved2: u32,
    /// Offset from LoggerInfo to where to store next log entry.
    pub(crate) log_current_offset: AtomicU32,
    /// Number of bytes of messages missed
    discarded_size: AtomicU32,
    /// Size of allocated buffer
    pub(crate) log_buffer_size: u32,
    /// Log in permanent RAM
    in_permanent_ram: bool,
    /// After ExitBootServices
    at_runtime: bool,
    /// After VirtualAddressChange
    gone_virtual: bool,
    /// HdwPort initialized
    hw_port_initialized: bool,
    /// HdwPort is Disabled
    hw_port_disabled: bool,
    /// Reserved for future
    reserved3: [bool; 3],
    /// Ticks per second for log timing
    timer_frequency: AtomicU64,
    /// Ticks when Time Acquired
    ticks_at_time: u64,
    /// UEFI Time Field
    time: efi::Time,
    /// Logging level to be printed at hw port
    hw_print_level: u32,
    /// Reserved
    reserved4: u32,
}

/// Implementation of the ADVANCED_LOGGER_INFO V6 C struct.
#[derive(Debug)]
#[repr(C)]
pub(crate) struct AdvLoggerInfoV6 {
    pub(crate) v5: AdvLoggerInfoV5,
    /// The address for a new logger info structure if it has been migrated
    new_logger_info_address: u64,
}

pub(crate) type AdvLoggerInfo = AdvLoggerInfoV6;

impl AdvLoggerInfo {
    /// Signature for the AdvLoggerInfo structure.
    pub const SIGNATURE: u32 = 0x474F4C41; // "ALOG"

    /// Version of the current AdvLoggerInfo structure.
    pub const VERSION: u16 = ADV_LOGGER_INFO_VERSION_V6;

    pub fn new(
        log_buffer_size: u32,
        hw_port_disabled: bool,
        timer_frequency: u64,
        ticks_at_time: u64,
        time: efi::Time,
        hw_print_level: u32,
    ) -> Self {
        let log_buffer_offset = size_of::<AdvLoggerInfo>() as u32;
        Self {
            v5: AdvLoggerInfoV5 {
                signature: Self::SIGNATURE,
                version: Self::VERSION,
                reserved1: [0, 0, 0],
                log_buffer_offset,
                reserved2: 0,
                log_current_offset: AtomicU32::new(log_buffer_offset),
                discarded_size: AtomicU32::new(0),
                log_buffer_size: log_buffer_size - log_buffer_offset,
                in_permanent_ram: true,
                at_runtime: false,
                gone_virtual: false,
                hw_port_initialized: false,
                hw_port_disabled,
                reserved3: [false, false, false],
                timer_frequency: AtomicU64::new(timer_frequency),
                ticks_at_time,
                time,
                hw_print_level,
                reserved4: 0,
            },
            new_logger_info_address: 0,
        }
    }
}

/// A reference to an advanced logger info structure of a supported version.
///
/// This enum provides a unified interface for accessing fields of supported versions.
#[derive(Clone, Copy)]
pub(crate) enum AdvLoggerInfoRef<'a> {
    V5(&'a AdvLoggerInfoV5),
    V6(&'a AdvLoggerInfoV6),
}

impl core::fmt::Debug for AdvLoggerInfoRef<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AdvLoggerInfoRef::V5(info) => info.fmt(f),
            AdvLoggerInfoRef::V6(info) => info.fmt(f),
        }
    }
}

impl<'a> AdvLoggerInfoRef<'a> {
    /// # Safety
    ///
    /// The caller must ensure `address` is readable and points to a valid
    /// `AdvLoggerInfo` header.
    pub unsafe fn from_address(address: efi::PhysicalAddress) -> Option<Self> {
        let log_info = address as *const AdvLoggerInfoV5;

        // SAFETY: The caller must ensure that the address is valid and
        //         that the memory is readable per this function's safety contract.
        let version = unsafe {
            if (*log_info).signature != AdvLoggerInfo::SIGNATURE {
                return None;
            }

            (*log_info).version
        };

        let header_size = Self::header_size_for_version(version)? as u32;

        // SAFETY: Version is confirmed as supported, verify offsets are consistent.
        let (log_buffer_offset, log_buffer_size, log_current_offset) = unsafe {
            (
                (*log_info).log_buffer_offset,
                (*log_info).log_buffer_size,
                (*log_info).log_current_offset.load(Ordering::Relaxed),
            )
        };

        if log_buffer_offset < header_size {
            return None;
        }

        let full_size = log_buffer_offset + log_buffer_size;
        if log_current_offset < log_buffer_offset || log_current_offset > full_size {
            return None;
        }

        // SAFETY: The log_info is valid, convert the data for future safety.
        unsafe {
            match version {
                ADV_LOGGER_INFO_VERSION_V5 => log_info.as_ref().map(AdvLoggerInfoRef::V5),
                ADV_LOGGER_INFO_VERSION_V6 => (address as *const AdvLoggerInfoV6).as_ref().map(AdvLoggerInfoRef::V6),
                _ => None,
            }
        }
    }

    pub fn from_bytes(log_bytes: &'a [u8]) -> Result<Self> {
        if log_bytes.len() < size_of::<AdvLoggerInfoV5>() {
            return Err(EfiError::BufferTooSmall);
        }

        // SAFETY: We have checked the length of the buffer is at least the size.
        //         Ideally we use ZeroCopy to do this, but `efi::Time` does not
        //         support it.
        let header_v5 =
            unsafe { log_bytes.as_ptr().cast::<AdvLoggerInfoV5>().as_ref() }.ok_or(EfiError::InvalidParameter)?;

        // Check that this is a valid log header.
        if header_v5.signature != AdvLoggerInfo::SIGNATURE {
            return Err(EfiError::InvalidParameter);
        }

        // Check that the logger info version is supported.
        let version = header_v5.version;
        let header_size = Self::header_size_for_version(version).ok_or(EfiError::Unsupported)?;
        if log_bytes.len() < header_size {
            return Err(EfiError::BufferTooSmall);
        }

        match version {
            ADV_LOGGER_INFO_VERSION_V5 => Ok(AdvLoggerInfoRef::V5(header_v5)),
            ADV_LOGGER_INFO_VERSION_V6 => {
                // SAFETY: log_bytes was validated above to check the length, structure signature,
                // version, and header size for the version to ensure that it is valid to interpret
                // the bytes as a V6 structure.
                let header_v6 = unsafe { log_bytes.as_ptr().cast::<AdvLoggerInfoV6>().as_ref() }
                    .ok_or(EfiError::InvalidParameter)?;
                Ok(AdvLoggerInfoRef::V6(header_v6))
            }
            _ => Err(EfiError::Unsupported),
        }
    }

    pub fn header_size_for_version(version: u16) -> Option<usize> {
        match version {
            ADV_LOGGER_INFO_VERSION_V5 => Some(size_of::<AdvLoggerInfoV5>()),
            ADV_LOGGER_INFO_VERSION_V6 => Some(size_of::<AdvLoggerInfoV6>()),
            _ => None,
        }
    }

    pub fn header_size(&self) -> usize {
        match self {
            AdvLoggerInfoRef::V5(_) => size_of::<AdvLoggerInfoV5>(),
            AdvLoggerInfoRef::V6(_) => size_of::<AdvLoggerInfoV6>(),
        }
    }

    pub fn log_buffer_offset(&self) -> u32 {
        match self {
            AdvLoggerInfoRef::V5(info) => info.log_buffer_offset,
            AdvLoggerInfoRef::V6(info) => info.v5.log_buffer_offset,
        }
    }

    pub fn log_buffer_size(&self) -> u32 {
        match self {
            AdvLoggerInfoRef::V5(info) => info.log_buffer_size,
            AdvLoggerInfoRef::V6(info) => info.v5.log_buffer_size,
        }
    }

    pub fn log_current_offset(&self) -> &AtomicU32 {
        match self {
            AdvLoggerInfoRef::V5(info) => &info.log_current_offset,
            AdvLoggerInfoRef::V6(info) => &info.v5.log_current_offset,
        }
    }

    pub fn discarded_size(&self) -> &AtomicU32 {
        match self {
            AdvLoggerInfoRef::V5(info) => &info.discarded_size,
            AdvLoggerInfoRef::V6(info) => &info.v5.discarded_size,
        }
    }

    pub fn timer_frequency(&self) -> &AtomicU64 {
        match self {
            AdvLoggerInfoRef::V5(info) => &info.timer_frequency,
            AdvLoggerInfoRef::V6(info) => &info.v5.timer_frequency,
        }
    }

    pub fn hw_port_disabled(&self) -> bool {
        match self {
            AdvLoggerInfoRef::V5(info) => info.hw_port_disabled,
            AdvLoggerInfoRef::V6(info) => info.v5.hw_port_disabled,
        }
    }

    pub fn hw_print_level(&self) -> u32 {
        match self {
            AdvLoggerInfoRef::V5(info) => info.hw_print_level,
            AdvLoggerInfoRef::V6(info) => info.v5.hw_print_level,
        }
    }

    pub fn new_logger_info_address(&self) -> Option<u64> {
        match self {
            AdvLoggerInfoRef::V5(_) => None,
            AdvLoggerInfoRef::V6(info) => Some(info.new_logger_info_address),
        }
    }

    pub fn full_size(&self) -> u32 {
        self.log_buffer_offset() + self.log_buffer_size()
    }

    pub fn as_ptr(&self) -> *const u8 {
        match *self {
            AdvLoggerInfoRef::V5(info) => info as *const AdvLoggerInfoV5 as *const u8,
            AdvLoggerInfoRef::V6(info) => info as *const AdvLoggerInfoV6 as *const u8,
        }
    }
}

/// Implementation of the C struct ADVANCED_LOGGER_MESSAGE_ENTRY_V2 for heading
/// a memory log entry.
#[repr(C)]
#[repr(packed)]
#[derive(Debug, IntoBytes, FromBytes, Immutable, KnownLayout)]
pub(crate) struct AdvLoggerMessageEntry {
    /// Signature
    pub signature: u32,
    /// Major version of advanced logger message structure. Current = 2
    pub major_version: u8,
    /// Minor version of advanced logger message structure. Current = 0
    pub minor_version: u8,
    /// Error Level
    pub level: u32,
    /// Time stamp
    pub timestamp: u64,
    /// Boot phase that produced this message entry
    pub boot_phase: u16,
    /// Number of bytes in Message
    pub message_length: u16,
    /// Offset of Message from start of structure, used to calculate the address of the Message
    pub message_offset: u16,
}

impl AdvLoggerMessageEntry {
    /// Signature for the AdvLoggerMessageEntry structure.
    pub const SIGNATURE: u32 = 0x324D4C41; // ALM2

    /// Major version of the AdvLoggerMessageEntry structure.
    pub const MAJOR_VERSION: u8 = 2;
    /// Minor version of the AdvLoggerMessageEntry structure.
    pub const MINOR_VERSION: u8 = 1;

    /// Creates the structure of AdvLoggerMessageEntry.
    ///
    /// This routine is only used internally as creating this structure alone
    /// is not a defined operation. This is used for convenience of setting the
    /// structure values for copying into memory and should not be used to directly
    /// create stack or heap structures.
    ///
    pub const fn new(boot_phase: u16, level: u32, timestamp: u64, message_length: u16) -> Self {
        Self {
            signature: Self::SIGNATURE,
            major_version: Self::MAJOR_VERSION,
            minor_version: Self::MINOR_VERSION,
            level,
            timestamp,
            boot_phase,
            message_length,
            message_offset: size_of::<Self>() as u16,
        }
    }

    /// Creates the structure of AdvLoggerMessageEntry from a [`LogEntry`].
    pub const fn from_log_entry(entry: &LogEntry) -> Self {
        Self::new(entry.phase, entry.level, entry.timestamp, entry.data.len() as u16)
    }

    /// Returns the length of the entire log entry.
    pub fn len(&self) -> usize {
        size_of::<Self>() + self.message_length as usize
    }

    /// Returns the aligned length of the entire log entry.
    pub fn aligned_len(&self) -> usize {
        // The length is already bounded to less than the buffer size and so cannot
        // overflow the usize with a simple 8 bit alignment.
        align_up(self.len(), 8)
            .expect("Aligning log entry to 8 bytes should not overflow since the length is bounded by the buffer size.")
    }
}

#[cfg(test)]
const TEST_DATA_SIZE: usize = 128;

#[cfg(test)]
pub(crate) fn create_buffer_v5(timer_frequency: u64, hw_port_disabled: bool) -> alloc::boxed::Box<[u8]> {
    let header_size = size_of::<AdvLoggerInfoV5>();
    let mut buffer = alloc::vec![0_u8; header_size + TEST_DATA_SIZE].into_boxed_slice();
    let header = AdvLoggerInfoV5 {
        signature: AdvLoggerInfo::SIGNATURE,
        version: ADV_LOGGER_INFO_VERSION_V5,
        reserved1: [0, 0, 0],
        log_buffer_offset: header_size as u32,
        reserved2: 0,
        log_current_offset: AtomicU32::new(header_size as u32),
        discarded_size: AtomicU32::new(0),
        log_buffer_size: TEST_DATA_SIZE as u32,
        in_permanent_ram: true,
        at_runtime: false,
        gone_virtual: false,
        hw_port_initialized: false,
        hw_port_disabled,
        reserved3: [false, false, false],
        timer_frequency: AtomicU64::new(timer_frequency),
        ticks_at_time: 0,
        time: efi::Time::default(),
        hw_print_level: DEBUG_LEVEL_INFO,
        reserved4: 0,
    };

    // SAFETY: The buffer was allocated with sufficient size (header_size + TEST_DATA_SIZE).
    unsafe {
        core::ptr::write(buffer.as_mut_ptr().cast::<AdvLoggerInfoV5>(), header);
    }

    buffer
}

#[cfg(test)]
pub(crate) fn create_buffer_v6(timer_frequency: u64, new_address: u64) -> alloc::boxed::Box<[u8]> {
    let header_size = size_of::<AdvLoggerInfoV6>();
    let mut buffer = alloc::vec![0_u8; header_size + TEST_DATA_SIZE].into_boxed_slice();
    let header = AdvLoggerInfoV6 {
        v5: AdvLoggerInfoV5 {
            signature: AdvLoggerInfo::SIGNATURE,
            version: ADV_LOGGER_INFO_VERSION_V6,
            reserved1: [0, 0, 0],
            log_buffer_offset: header_size as u32,
            reserved2: 0,
            log_current_offset: AtomicU32::new(header_size as u32),
            discarded_size: AtomicU32::new(0),
            log_buffer_size: TEST_DATA_SIZE as u32,
            in_permanent_ram: true,
            at_runtime: false,
            gone_virtual: false,
            hw_port_initialized: false,
            hw_port_disabled: false,
            reserved3: [false, false, false],
            timer_frequency: AtomicU64::new(timer_frequency),
            ticks_at_time: 0,
            time: efi::Time::default(),
            hw_print_level: DEBUG_LEVEL_INFO,
            reserved4: 0,
        },
        new_logger_info_address: new_address,
    };

    // SAFETY: The buffer is sized for the header and is aligned for AdvLoggerInfoV6.
    unsafe {
        core::ptr::write(buffer.as_mut_ptr().cast::<AdvLoggerInfoV6>(), header);
    }

    buffer
}
