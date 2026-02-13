//! UEFI Advanced Logger Memory Log Support
//!
//! This module provides a definitions and routines to access a Advanced Logger
//! memory log structure.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{
    cell::UnsafeCell,
    mem::size_of,
    ptr, slice,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
};
use patina::{
    base::align_up,
    error::{EfiError, Result},
};
use r_efi::efi;
use zerocopy::{FromBytes, IntoBytes};
use zerocopy_derive::*;

// { 0x4d60cfb5, 0xf481, 0x4a98, {0x9c, 0x81, 0xbf, 0xf8, 0x64, 0x60, 0xc4, 0x3e }}
pub const ADV_LOGGER_HOB_GUID: efi::Guid =
    efi::Guid::from_fields(0x4d60cfb5, 0xf481, 0x4a98, 0x9c, 0x81, &[0xbf, 0xf8, 0x64, 0x60, 0xc4, 0x3e]);

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

/// This struct represents an advanced logger memory log. It contains the appropriate
/// pointers and interior mutability to allow for safe access to the log data. This
/// serves as the idiomatic rust container for the C based structures.
pub struct AdvancedLog<'a> {
    /// The header of the memory log.
    pub(crate) header: AdvLoggerInfoRef<'a>,
    /// The data portion of the memory log.
    data: LogData<'a>,
}

// SAFETY: The only interior mutability is the UnsafeCell for the data region of
//         the log. Safety here is checked by the allocation logic in add_log_entry
//         which relies on atomics to safely allocate portions of the buffer.
unsafe impl Send for AdvancedLog<'static> {}
// SAFETY: See the Send safety comment
unsafe impl Sync for AdvancedLog<'static> {}

impl AdvancedLog<'static> {
    /// Initializes a `AdvancedLog` from an existing advanced logger buffer at the
    /// provided address. The caller must ensure that this memory is accessible.
    ///
    /// ### Safety
    ///
    /// This function assumes that the provided address points to a valid `AdvLoggerInfo`
    /// structure and that the memory is properly sized initialized based on the
    /// information in that structure.
    pub unsafe fn adopt_memory_log(address: efi::PhysicalAddress) -> Option<Self> {
        // SAFETY: The safety requirements for this function transfer to the function called here.
        let header = unsafe { AdvLoggerInfoRef::from_address(address)? };
        let data_size = header.log_buffer_size();
        let data_start = (address + header.log_buffer_offset() as u64) as *mut u8;
        // SAFETY: The caller must ensure that the memory is properly sized and initialized
        // per the safety contract of this function. from_address() validates the signature
        // and version in the header.
        let data = unsafe { slice::from_raw_parts_mut(data_start, data_size as usize) };

        Some(Self { header, data: LogData::ReadWrite(UnsafeCell::from_mut(data)) })
    }

    // Allow unused as it is used in tests and intended for future general use.
    #[allow(dead_code)]
    /// Initializes a new Advanced Log buffer at the provided address with the
    /// specified length.
    ///
    /// ### Safety
    ///
    /// The caller is responsible for ensuring that the provided address is appropriately
    /// allocated and accessible.
    pub unsafe fn initialize_memory_log(address: efi::PhysicalAddress, length: u32) -> Option<Self> {
        if length < size_of::<AdvLoggerInfo>() as u32
            || !address.is_multiple_of(core::mem::align_of::<AdvLoggerInfo>() as u64)
        {
            return None;
        }

        let header = address as *mut AdvLoggerInfo;
        if header.is_null() {
            None
        } else {
            // SAFETY: The caller should ensure that the address is valid and
            //         that the memory is writable.
            unsafe { ptr::write(header, AdvLoggerInfo::new(length, false, 0, 0, efi::Time::default(), 0)) };

            // SAFETY: The header is now initialized, so we can safely create the
            //         AdvancedLog instance.
            unsafe { Self::adopt_memory_log(address) }
        }
    }
}

impl<'a> AdvancedLog<'a> {
    // Only used in the parser which is not always compiled.
    #[allow(dead_code)]
    pub fn open_log(log_bytes: &'a [u8]) -> Result<Self> {
        let header = AdvLoggerInfoRef::from_bytes(log_bytes)?;

        // Check that the various pointers are consistent.
        let log_current = header.log_current_offset().load(Ordering::Relaxed);
        if log_current < header.log_buffer_offset()
            || log_current > header.full_size()
            || header.log_buffer_offset() < header.header_size() as u32
        {
            return Err(EfiError::InvalidParameter);
        }

        // Only require that the valid portion of the log buffer be present.
        if log_current > log_bytes.len() as u32 {
            return Err(EfiError::BufferTooSmall);
        }

        let (_, data_slice) = log_bytes.split_at(header.log_buffer_offset() as usize);

        Ok(Self { header, data: LogData::ReadOnly(data_slice) })
    }

    pub fn add_log_entry(&self, log_entry: LogEntry) -> Result<()> {
        // Adding a log entry consists of two steps:
        // 1. Atomically allocate space in the log buffer. This must be done before
        //    writing the log entry to ensure that no other system can write to the
        //    same space in the log buffer.
        // 2. Write the log entry to the allocated space in the log buffer.
        //

        // Get the total size of the long entry with the header, including the
        // alignment padding for 8 byte alignment.
        let data_offset = size_of::<AdvLoggerMessageEntry>() as u16;
        let unaligned_size = data_offset as u32 + log_entry.data.len() as u32;
        let message_size = align_up(unaligned_size, 8).unwrap() as u32;

        // try to swap in the updated value. if this grows beyond the buffer, fall out.
        // Using relaxed here as we only want the atomic swap and are not concerned
        // with ordering. The loop should still use the atomic swap and update each
        // iteration.
        let mut current_offset = self.header.log_current_offset().load(Ordering::Relaxed);
        while current_offset + message_size <= self.header.full_size() {
            match self.header.log_current_offset().compare_exchange(
                current_offset,
                current_offset + message_size,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(val) => current_offset = val,
            }
        }

        // check if we fell out of bounds.
        if current_offset + message_size > self.header.full_size() {
            // Add the discarded value. No ordering needed as this is a single
            // operation.
            self.header.discarded_size().fetch_add(message_size, Ordering::Relaxed);
            return Err(EfiError::OutOfResources);
        }

        let data_index = (current_offset - self.header.log_buffer_offset()) as usize;

        // SAFETY: The space hase been allocated. It should now be safe to write
        // data so long as it sticks to the range of the allocated entry. Get only
        // the allocated slice to maintain safety.
        let entry_slice = unsafe {
            let data: *mut [u8] = self.data.get_mut()?;
            (&mut *data).get_mut(data_index..data_index + message_size as usize).ok_or(EfiError::BufferTooSmall)?
        };

        let (header_slice, entry_slice) = entry_slice.split_at_mut(size_of::<AdvLoggerMessageEntry>());
        let (data_slice, remainder_slice) = entry_slice.split_at_mut(log_entry.data.len());

        AdvLoggerMessageEntry::from_log_entry(&log_entry)
            .write_to(header_slice)
            .map_err(|_| EfiError::BufferTooSmall)?;

        log_entry.data.write_to(data_slice).map_err(|_| EfiError::BufferTooSmall)?;
        remainder_slice.fill(0);

        Ok(())
    }

    pub fn hardware_write_enabled(&self, level: u32) -> bool {
        !self.header.hw_port_disabled() && (level & self.header.hw_print_level() != 0)
    }

    pub fn iter(&self) -> AdvLogIterator<'_> {
        AdvLogIterator::new(self)
    }

    pub fn get_frequency(&self) -> u64 {
        self.header.timer_frequency().load(Ordering::Relaxed)
    }

    pub fn set_frequency(&self, frequency: u64) {
        // try to swap, assuming the value it initially 0. If this fails, just continue.
        let _ = self.header.timer_frequency().compare_exchange(0, frequency, Ordering::Relaxed, Ordering::Relaxed);
    }

    pub fn get_address(&self) -> efi::PhysicalAddress {
        self.header.as_ptr() as efi::PhysicalAddress
    }

    pub fn get_new_logger_info_address(&self) -> Option<efi::PhysicalAddress> {
        self.header
            .new_logger_info_address()
            .and_then(|address| if address == 0 { None } else { Some(address as efi::PhysicalAddress) })
    }

    // Allow unused as it is used in tests and intended for future general use.
    #[allow(dead_code)]
    pub fn discarded_size(&self) -> u32 {
        self.header.discarded_size().load(Ordering::Relaxed)
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
    version: u16,
    /// Reserved for future
    reserved1: [u16; 3],
    /// Offset from LoggerInfo to start of log, expected to be the size of this structure 8 byte aligned
    log_buffer_offset: u32,
    /// Reserved for future
    reserved2: u32,
    /// Offset from LoggerInfo to where to store next log entry.
    log_current_offset: AtomicU32,
    /// Number of bytes of messages missed
    discarded_size: AtomicU32,
    /// Size of allocated buffer
    log_buffer_size: u32,
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
    v5: AdvLoggerInfoV5,
    /// The address for a new logger info structure if it has been migrated
    new_logger_info_address: u64,
}

type AdvLoggerInfo = AdvLoggerInfoV6;

impl AdvLoggerInfo {
    /// Signature for the AdvLoggerInfo structure.
    pub const SIGNATURE: u32 = 0x474F4C41; // "ALOG"

    /// Version of the current AdvLoggerInfo structure.
    pub const VERSION: u16 = ADV_LOGGER_INFO_VERSION_V6;

    fn new(
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
    unsafe fn from_address(address: efi::PhysicalAddress) -> Option<Self> {
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

    fn from_bytes(log_bytes: &'a [u8]) -> Result<Self> {
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

    fn header_size_for_version(version: u16) -> Option<usize> {
        match version {
            ADV_LOGGER_INFO_VERSION_V5 => Some(size_of::<AdvLoggerInfoV5>()),
            ADV_LOGGER_INFO_VERSION_V6 => Some(size_of::<AdvLoggerInfoV6>()),
            _ => None,
        }
    }

    fn header_size(&self) -> usize {
        match self {
            AdvLoggerInfoRef::V5(_) => size_of::<AdvLoggerInfoV5>(),
            AdvLoggerInfoRef::V6(_) => size_of::<AdvLoggerInfoV6>(),
        }
    }

    fn log_buffer_offset(&self) -> u32 {
        match self {
            AdvLoggerInfoRef::V5(info) => info.log_buffer_offset,
            AdvLoggerInfoRef::V6(info) => info.v5.log_buffer_offset,
        }
    }

    fn log_buffer_size(&self) -> u32 {
        match self {
            AdvLoggerInfoRef::V5(info) => info.log_buffer_size,
            AdvLoggerInfoRef::V6(info) => info.v5.log_buffer_size,
        }
    }

    fn log_current_offset(&self) -> &AtomicU32 {
        match self {
            AdvLoggerInfoRef::V5(info) => &info.log_current_offset,
            AdvLoggerInfoRef::V6(info) => &info.v5.log_current_offset,
        }
    }

    fn discarded_size(&self) -> &AtomicU32 {
        match self {
            AdvLoggerInfoRef::V5(info) => &info.discarded_size,
            AdvLoggerInfoRef::V6(info) => &info.v5.discarded_size,
        }
    }

    fn timer_frequency(&self) -> &AtomicU64 {
        match self {
            AdvLoggerInfoRef::V5(info) => &info.timer_frequency,
            AdvLoggerInfoRef::V6(info) => &info.v5.timer_frequency,
        }
    }

    fn hw_port_disabled(&self) -> bool {
        match self {
            AdvLoggerInfoRef::V5(info) => info.hw_port_disabled,
            AdvLoggerInfoRef::V6(info) => info.v5.hw_port_disabled,
        }
    }

    fn hw_print_level(&self) -> u32 {
        match self {
            AdvLoggerInfoRef::V5(info) => info.hw_print_level,
            AdvLoggerInfoRef::V6(info) => info.v5.hw_print_level,
        }
    }

    fn new_logger_info_address(&self) -> Option<u64> {
        match self {
            AdvLoggerInfoRef::V5(_) => None,
            AdvLoggerInfoRef::V6(info) => Some(info.new_logger_info_address),
        }
    }

    fn full_size(&self) -> u32 {
        self.log_buffer_offset() + self.log_buffer_size()
    }

    fn as_ptr(&self) -> *const u8 {
        match *self {
            AdvLoggerInfoRef::V5(info) => info as *const AdvLoggerInfoV5 as *const u8,
            AdvLoggerInfoRef::V6(info) => info as *const AdvLoggerInfoV6 as *const u8,
        }
    }
}

/// Wrapper to allow for a read-only or read-write data region for the log.
enum LogData<'a> {
    /// A slice of bytes representing the log message.
    ReadOnly(&'a [u8]),
    /// A string slice representing the log message.
    ReadWrite(&'a UnsafeCell<[u8]>),
}

impl<'a> LogData<'a> {
    /// Returns the data as a slice.
    fn get(&self) -> &'a [u8] {
        match self {
            LogData::ReadOnly(slice) => slice,
            // SAFETY: The allocated memory is guaranteed to be valid, and the lifetime
            //         of the underlining data is tied to the lifetime of the `AdvancedLog`.
            LogData::ReadWrite(cell) => unsafe { &*cell.get() },
        }
    }

    /// Returns the data as a mutable slice.
    fn get_mut(&self) -> Result<*mut [u8]> {
        match self {
            LogData::ReadOnly(_) => Err(EfiError::AccessDenied),
            LogData::ReadWrite(cell) => Ok(cell.get()),
        }
    }
}

/// Implementation of the C struct ADVANCED_LOGGER_MESSAGE_ENTRY_V2 for heading
/// a memory log entry.
#[repr(C)]
#[repr(packed)]
#[derive(Debug, IntoBytes, FromBytes, Immutable, KnownLayout)]
struct AdvLoggerMessageEntry {
    /// Signature
    signature: u32,
    /// Major version of advanced logger message structure. Current = 2
    major_version: u8,
    /// Minor version of advanced logger message structure. Current = 0
    minor_version: u8,
    /// Error Level
    pub level: u32,
    /// Time stamp
    pub timestamp: u64,
    /// Boot phase that produced this message entry
    pub boot_phase: u16,
    /// Number of bytes in Message
    message_length: u16,
    /// Offset of Message from start of structure, used to calculate the address of the Message
    message_offset: u16,
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
    const fn new(boot_phase: u16, level: u32, timestamp: u64, message_length: u16) -> Self {
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
    const fn from_log_entry(entry: &LogEntry) -> Self {
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
        align_up(self.len(), 8).unwrap()
    }
}

/// Iterator for an advanced logger memory buffer log.
pub struct AdvLogIterator<'a> {
    log: &'a AdvancedLog<'a>,
    offset: usize,
}

/// Iterator for an Advanced Logger memory buffer.
impl<'a> AdvLogIterator<'a> {
    /// Creates a new log iterator from a given AdvLoggerInfo reference.
    fn new(log: &'a AdvancedLog) -> Self {
        AdvLogIterator { log, offset: log.header.log_buffer_offset() as usize }
    }
}

impl<'a> Iterator for AdvLogIterator<'a> {
    type Item = LogEntry<'a>;

    /// Provides the next advanced logger entry in the Advanced Logger memory buffer.
    fn next(&mut self) -> Option<Self::Item> {
        if self.offset + size_of::<AdvLoggerMessageEntry>()
            > self.log.header.log_current_offset().load(Ordering::Relaxed) as usize
        {
            None
        } else {
            // Get the data relative offset.
            let mut data_index = self.offset - self.log.header.log_buffer_offset() as usize;

            // SAFETY: We have verified the buffer through the header, read that
            //         to check the rest of the range.
            let header_slice = unsafe {
                let data: *const [u8] = self.log.data.get();
                (&*data).get(data_index..data_index + size_of::<AdvLoggerMessageEntry>())?
            };

            let entry_header = AdvLoggerMessageEntry::ref_from_bytes(header_slice).ok()?;
            data_index += size_of::<AdvLoggerMessageEntry>();

            if self.offset + size_of::<AdvLoggerMessageEntry>() + entry_header.message_length as usize
                > self.log.header.log_current_offset().load(Ordering::Relaxed) as usize
            {
                None
            } else {
                // SAFETY: We know that the buffer is valid through previous checks,
                //         and this structure should be well formed from the header
                //         information.
                let entry_data = unsafe {
                    let data: *const [u8] = self.log.data.get();
                    (&*data).get(data_index..data_index + entry_header.message_length as usize)?
                };

                // Move the offset up by the aligned total size.
                self.offset += entry_header.aligned_len();

                Some(LogEntry {
                    phase: entry_header.boot_phase,
                    level: entry_header.level,
                    timestamp: entry_header.timestamp,
                    data: entry_data,
                })
            }
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    extern crate std;
    use alloc::{boxed::Box, vec};
    use efi::PhysicalAddress;

    use super::*;

    const TEST_DATA_SIZE: usize = 128;

    fn create_buffer_v5(timer_frequency: u64, hw_port_disabled: bool) -> Box<[u8]> {
        let header_size = size_of::<AdvLoggerInfoV5>();
        let mut buffer = vec![0_u8; header_size + TEST_DATA_SIZE].into_boxed_slice();
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
            ptr::write(buffer.as_mut_ptr().cast::<AdvLoggerInfoV5>(), header);
        }

        buffer
    }

    fn create_buffer_v6(timer_frequency: u64, new_address: u64) -> Box<[u8]> {
        let header_size = size_of::<AdvLoggerInfoV6>();
        let mut buffer = vec![0_u8; header_size + TEST_DATA_SIZE].into_boxed_slice();
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
            ptr::write(buffer.as_mut_ptr().cast::<AdvLoggerInfoV6>(), header);
        }

        buffer
    }

    #[test]
    fn create_fill_check_test() {
        let mut buff_box = Box::new([0_u64; 0x2000]);
        let buffer = buff_box.as_mut();
        let address = buffer as *mut u64 as PhysicalAddress;
        let len = buffer.len() as u32;

        // SAFETY: We just allocated this memory so it's valid.
        let log = unsafe { AdvancedLog::initialize_memory_log(address, len) }.unwrap();

        // Fill the log.
        let mut entries: u32 = 0;
        loop {
            let data = entries.to_be_bytes();
            let entry: LogEntry<'_> = LogEntry { level: 0, phase: 0, timestamp: 0, data: &data };
            let log_entry = log.add_log_entry(entry);
            match log_entry {
                Ok(_) => {}
                Err(EfiError::OutOfResources) => {
                    assert!(log.discarded_size() > 0);
                    assert!(entries > 0);
                    break;
                }
                Err(status) => {
                    panic!("Unexpected add_log_entry returned unexpected status {status:#x?}.")
                }
            }
            entries += 1;
        }

        // check the contents.
        let mut iter = log.iter();
        for entry_num in 0..entries {
            let data = entry_num.to_be_bytes();
            let log_entry = iter.next().unwrap();
            assert_eq!(log_entry.get_message(), data);
        }

        assert!(iter.next().is_none());
    }

    #[test]
    fn adopt_buffer_test() {
        let buff_box = Box::new([0_u8; 0x10000]);
        let buffer = buff_box.as_ref();
        let address = buffer as *const u8 as PhysicalAddress;
        let len = buffer.len() as u32;

        // SAFETY: We just allocated this memory so it's valid.
        let log = unsafe { AdvancedLog::initialize_memory_log(address, len) }.unwrap();

        // Fill the log.
        for val in 0..50 {
            let data = (val as u32).to_be_bytes();
            let entry = LogEntry { level: 0, phase: 0, timestamp: 0, data: &data };
            log.add_log_entry(entry).unwrap();
        }

        // SAFETY: This is the same buffer as before, still valid.
        let log = unsafe { AdvancedLog::adopt_memory_log(address) }.unwrap();

        // Add more entries.
        for val in 50..100 {
            let data = (val as u32).to_be_bytes();
            let entry = LogEntry { level: 0, phase: 0, timestamp: 0, data: &data };
            log.add_log_entry(entry).unwrap();
        }

        // check the contents.
        assert!(log.discarded_size() == 0);
        let mut iter = log.iter();
        for entry_num in 0..100 {
            let data = (entry_num as u32).to_be_bytes();
            let log_entry = iter.next().unwrap();
            assert_eq!(log_entry.get_message(), data);
        }

        assert!(iter.next().is_none());
    }

    #[test]
    fn open_log_supports_v5_and_v6() {
        let buffer_v5 = create_buffer_v5(123, false);
        let log_v5 = AdvancedLog::open_log(&buffer_v5).unwrap();
        assert_eq!(log_v5.get_frequency(), 123);
        assert!(log_v5.hardware_write_enabled(DEBUG_LEVEL_INFO));
        assert!(log_v5.get_new_logger_info_address().is_none());

        let buffer_v6 = create_buffer_v6(456, 0x1122334455667788);
        let log_v6 = AdvancedLog::open_log(&buffer_v6).unwrap();
        assert_eq!(log_v6.get_frequency(), 456);
        assert!(log_v6.get_new_logger_info_address().is_some());
    }

    #[test]
    fn adopt_memory_log_accepts_v5_and_v6() {
        let mut buffer_v5 = create_buffer_v5(0, false);
        let address_v5 = buffer_v5.as_mut_ptr() as PhysicalAddress;
        // SAFETY: The memory was allocated successfully in this function and has been initialized
        // to contain a valid AdvLoggerInfoV5 header.
        let log_v5 = unsafe { AdvancedLog::adopt_memory_log(address_v5) }.unwrap();
        let entry_v5 = LogEntry { level: 0, phase: 0, timestamp: 0, data: b"v5" };
        log_v5.add_log_entry(entry_v5).unwrap();
        let mut iter = log_v5.iter();
        assert_eq!(iter.next().unwrap().get_message(), b"v5");

        let mut buffer_v6 = create_buffer_v6(0, 0);
        let address_v6 = buffer_v6.as_mut_ptr() as PhysicalAddress;
        // SAFETY: The memory was allocated successfully in this function and has been initialized
        // to contain a valid AdvLoggerInfoV6 header.
        let log_v6 = unsafe { AdvancedLog::adopt_memory_log(address_v6) }.unwrap();
        let entry_v6 = LogEntry { level: 0, phase: 0, timestamp: 0, data: b"v6" };
        log_v6.add_log_entry(entry_v6).unwrap();
        let mut iter = log_v6.iter();
        assert_eq!(iter.next().unwrap().get_message(), b"v6");
    }

    #[test]
    fn adopt_memory_log_rejects_log_buffer_offset_less_than_header_size() {
        let mut buffer = create_buffer_v5(0, false);
        let header = buffer.as_mut_ptr().cast::<AdvLoggerInfoV5>();

        // SAFETY: The buffer contains a valid AdvLoggerInfoV5 header, so mutating header fields is valid.
        unsafe {
            (*header).log_buffer_offset = (size_of::<AdvLoggerInfoV5>() as u32) - 1;
        }

        let address = buffer.as_mut_ptr() as PhysicalAddress;
        // SAFETY: The buffer structure is valid in memory allocated above.
        let result = unsafe { AdvancedLog::adopt_memory_log(address) };
        assert!(result.is_none());
    }

    #[test]
    fn adopt_memory_log_rejects_log_current_offset_before_buffer_start() {
        let mut buffer = create_buffer_v5(0, false);
        let header = buffer.as_mut_ptr().cast::<AdvLoggerInfoV5>();

        // SAFETY: The buffer contains a valid AdvLoggerInfoV5 header, so mutating header fields is valid.
        unsafe {
            (*header).log_current_offset.store((*header).log_buffer_offset - 1, Ordering::Relaxed);
        }

        let address = buffer.as_mut_ptr() as PhysicalAddress;
        // SAFETY: The buffer structure is valid in memory allocated above.
        let result = unsafe { AdvancedLog::adopt_memory_log(address) };
        assert!(result.is_none());
    }

    #[test]
    fn adopt_memory_log_rejects_log_current_offset_beyond_full_size() {
        let mut buffer = create_buffer_v5(0, false);
        let header = buffer.as_mut_ptr().cast::<AdvLoggerInfoV5>();

        // SAFETY: The buffer contains a valid AdvLoggerInfoV5 header, so mutating header fields is valid.
        unsafe {
            let full_size = (*header).log_buffer_offset + (*header).log_buffer_size;
            (*header).log_current_offset.store(full_size + 1, Ordering::Relaxed);
        }

        let address = buffer.as_mut_ptr() as PhysicalAddress;
        // SAFETY: The buffer structure is valid in memory allocated above.
        let result = unsafe { AdvancedLog::adopt_memory_log(address) };
        assert!(result.is_none());
    }

    #[test]
    fn open_log_rejects_unknown_version() {
        let mut buffer = create_buffer_v5(0, false);
        let header = buffer.as_mut_ptr().cast::<AdvLoggerInfoV5>();

        // SAFETY: The buffer contains a valid AdvLoggerInfoV5 header, so mutating the version field is in-bounds.
        unsafe {
            (*header).version = 7;
        }

        let result = AdvancedLog::open_log(&buffer);
        assert!(matches!(result, Err(EfiError::Unsupported)));
    }

    #[test]
    fn open_log_rejects_log_current_offset_before_buffer_start() {
        let mut buffer = create_buffer_v5(0, false);
        let header = buffer.as_mut_ptr().cast::<AdvLoggerInfoV5>();

        // SAFETY: The buffer contains a valid AdvLoggerInfoV5 header, so mutating header fields is valid.
        unsafe {
            (*header).log_current_offset.store((*header).log_buffer_offset - 1, Ordering::Relaxed);
        }

        let result = AdvancedLog::open_log(&buffer);
        assert!(matches!(result, Err(EfiError::InvalidParameter)));
    }

    #[test]
    fn open_log_rejects_log_current_offset_beyond_full_size() {
        let mut buffer = create_buffer_v5(0, false);
        let header = buffer.as_mut_ptr().cast::<AdvLoggerInfoV5>();

        // SAFETY: The buffer contains a valid AdvLoggerInfoV5 header, so mutating header fields is valid.
        unsafe {
            let full_size = (*header).log_buffer_offset + (*header).log_buffer_size;
            (*header).log_current_offset.store(full_size + 1, Ordering::Relaxed);
        }

        let result = AdvancedLog::open_log(&buffer);
        assert!(matches!(result, Err(EfiError::InvalidParameter)));
    }

    #[test]
    fn open_log_rejects_log_buffer_offset_less_than_header_size() {
        let mut buffer = create_buffer_v5(0, false);
        let header = buffer.as_mut_ptr().cast::<AdvLoggerInfoV5>();

        // SAFETY: The buffer contains a valid AdvLoggerInfoV5 header, so mutating header fields is valid.
        unsafe {
            (*header).log_buffer_offset = (size_of::<AdvLoggerInfoV5>() as u32) - 1;
        }

        let result = AdvancedLog::open_log(&buffer);
        assert!(matches!(result, Err(EfiError::InvalidParameter)));
    }
}
