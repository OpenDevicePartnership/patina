//! UEFI Advanced Logger Memory Log Support
//!
//! This module provides a definitions and routines to access a Advanced Logger
//! memory log structure.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use core::{
    ffi::c_void,
    mem::size_of,
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};
use r_efi::efi;

// { 0x4d60cfb5, 0xf481, 0x4a98, {0x9c, 0x81, 0xbf, 0xf8, 0x64, 0x60, 0xc4, 0x3e }}
pub const ADV_LOGGER_HOB_GUID: efi::Guid =
    efi::Guid::from_fields(0x4d60cfb5, 0xf481, 0x4a98, 0x9c, 0x81, &[0xbf, 0xf8, 0x64, 0x60, 0xc4, 0x3e]);

// UEFI Debug Levels
pub const DEBUG_LEVEL_ERROR: u32 = 0x80000000;
pub const DEBUG_LEVEL_WARNING: u32 = 0x00000002;
pub const DEBUG_LEVEL_INFO: u32 = 0x00000040;
pub const DEBUG_LEVEL_VERBOSE: u32 = 0x00400000;

// Phase definitions.
pub const ADVANCED_LOGGER_PHASE_DXE: u16 = 4;

/// A struct for carrying log entry data through this module.
pub struct LogEntry<'a> {
    pub phase: u16,
    pub level: u32,
    pub timestamp: u64,
    pub data: &'a [u8],
}

/// Implementation of the C struct ADVANCED_LOGGER_INFO for tracking in-memory
/// logging structure for Advanced Logger.
#[derive(Debug)]
#[repr(C)]
pub struct AdvLoggerInfo {
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
    log_current_offset: u32,
    /// Number of bytes of messages missed
    discarded_size: u32,
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
    timer_frequency: u64,
    /// Ticks when Time Acquired
    ticks_at_time: u64,
    /// UEFI Time Field
    time: efi::Time,
    /// Logging level to be printed at hw port
    hw_print_level: u32,
}

impl AdvLoggerInfo {
    /// Signature for the AdvLoggerInfo structure.
    pub const SIGNATURE: u32 = 0x474F4C41; // "ALOG"

    /// Version of the current AdvLoggerInfo structure.
    pub const VERSION: u16 = 5;

    fn new(
        log_buffer_size: u32,
        hw_port_disabled: bool,
        timer_frequency: u64,
        ticks_at_time: u64,
        time: efi::Time,
        hw_print_level: u32,
    ) -> Self {
        Self {
            signature: Self::SIGNATURE,
            version: Self::VERSION,
            reserved1: [0, 0, 0],
            log_buffer_offset: size_of::<AdvLoggerInfo>() as u32,
            reserved2: 0,
            log_current_offset: size_of::<AdvLoggerInfo>() as u32,
            discarded_size: 0,
            log_buffer_size,
            in_permanent_ram: true,
            at_runtime: false,
            gone_virtual: false,
            hw_port_initialized: false,
            hw_port_disabled,
            reserved3: [false, false, false],
            timer_frequency,
            ticks_at_time,
            time,
            hw_print_level,
        }
    }

    pub unsafe fn adopt_memory_log(address: efi::PhysicalAddress) -> Option<&'static Self> {
        let log_info = address as *mut Self;
        if (*log_info).signature != Self::SIGNATURE
            || (*log_info).version != Self::VERSION
            || (*log_info).log_buffer_offset < size_of::<AdvLoggerInfo>() as u32
        {
            None
        } else {
            log_info.as_ref()
        }
    }

    pub unsafe fn initialize_memory_log(address: efi::PhysicalAddress, length: u32) -> Option<&'static Self> {
        let log_info = address as *mut Self;
        if log_info.is_null() {
            None
        } else {
            ptr::write(log_info, AdvLoggerInfo::new(length, false, 0, 0, efi::Time::default(), 0));
            log_info.as_ref()
        }
    }

    pub fn add_log_entry(&self, log_entry: LogEntry) -> Option<&AdvLoggerMessageEntry> {
        let data_offset = size_of::<AdvLoggerMessageEntry>() as u16;
        let message_size = data_offset as u32 + log_entry.data.len() as u32;
        // Align up to the next 8 byte.
        let message_size = (message_size + 7) & !7;

        // SAFETY: We know this value is valid, but a atomic is needed for sharing
        //         across environments. This gives us internal mutability of the log.
        let atomic_offset = unsafe { AtomicU32::from_ptr(&self.log_current_offset as *const u32 as *mut u32) };

        // try to swap in the updated value. if this grows beyond the buffer, fall out.
        // Using relaxed here as we only want the atomic swap and are not concerned
        // with ordering. The loop should still use the atomic swap and update each
        // iteration.
        let mut current_offset = atomic_offset.load(Ordering::Relaxed);
        while current_offset + message_size <= self.log_buffer_size {
            match atomic_offset.compare_exchange(
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
        if current_offset + message_size > self.log_buffer_size {
            // SAFETY: We know this value is valid, but a atomic is needed for sharing
            //         across environments. This gives us internal mutability of the log.
            let discarded_size = unsafe { AtomicU32::from_ptr(&self.discarded_size as *const u32 as *mut u32) };
            // Add the discarded value. No ordering needed as this is a single
            // operation.
            discarded_size.fetch_add(message_size, Ordering::Relaxed);
            return None;
        }

        // Convert the newly allocated to usable data.
        let address = unsafe { (self as *const AdvLoggerInfo).byte_offset(current_offset as isize) };
        unsafe { AdvLoggerMessageEntry::init_from_memory(address as *mut c_void, message_size, log_entry) }
    }

    pub fn hardware_write_enabled(&self, level: u32) -> bool {
        !self.hw_port_disabled && (level & self.hw_print_level != 0)
    }

    pub fn iter(&self) -> AdvLogIterator {
        AdvLogIterator::new(self)
    }
}

/// Implementation of the C struct ADVANCED_LOGGER_MESSAGE_ENTRY_V2 for heading
/// a memory log entry.
#[repr(C)]
#[repr(packed)]
#[derive(Debug)]
pub struct AdvLoggerMessageEntry {
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
    pub const MINOR_VERSION: u8 = 0;

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

    /// Initializes an AdvLoggerMessageEntry given a memory address and length.
    ///
    /// This routine will create a AdvLoggerMessageEntry at the given address with
    /// the contents provided by log_entry.
    ///
    /// SAFETY: This routine will directly alter the given memory address up to
    /// the provided length. The caller is responsible for ensuring this memory
    /// range is valid.
    ///
    pub unsafe fn init_from_memory(address: *const c_void, length: u32, log_entry: LogEntry) -> Option<&'static Self> {
        debug_assert!(
            size_of::<Self>() + log_entry.data.len() <= length as usize,
            "Advanced logger entry initialized in an insufficiently sized buffer!"
        );

        if size_of::<Self>() + log_entry.data.len() > length as usize {
            return None;
        }

        // Write the header.
        let adv_entry = address as *mut AdvLoggerMessageEntry;
        ptr::write_volatile(
            adv_entry,
            Self::new(log_entry.phase, log_entry.level, log_entry.timestamp, log_entry.data.len() as u16),
        );

        // write the data.
        let message = adv_entry.offset(1) as *mut u8;
        ptr::copy(log_entry.data.as_ptr(), message, log_entry.data.len());

        adv_entry.as_ref()
    }

    /// Returns the data array of the message entry.
    pub fn get_message(&self) -> &'static [u8] {
        let message = unsafe { (self as *const Self).offset(1) } as *mut u8;

        // SAFETY: Assurances should be made during creation that this buffer
        //         offset is sufficient and accurate.
        let data = unsafe { core::slice::from_raw_parts(message, self.message_length as usize) };
        data
    }

    /// Returns the length of the entire log entry.
    pub fn len(&self) -> usize {
        size_of::<Self>() + self.message_length as usize
    }

    /// Returns the aligned length of the entire log entry.
    pub fn aligned_len(&self) -> usize {
        (self.len() + 7) & !7
    }
}

/// Iterator for an advanced logger memory buffer log.
pub struct AdvLogIterator<'a> {
    log_info: &'a AdvLoggerInfo,
    offset: usize,
}

/// Iterator for an Advanced Logger memory buffer.
impl<'a> AdvLogIterator<'a> {
    /// Creates a new log iterator from a given AdvLoggerInfo reference.
    const fn new(log_info: &'a AdvLoggerInfo) -> Self {
        AdvLogIterator { log_info, offset: log_info.log_buffer_offset as usize }
    }
}

impl<'a> Iterator for AdvLogIterator<'a> {
    type Item = &'a AdvLoggerMessageEntry;

    /// Provides the next advanced logger entry in the Advanced Logger memory buffer.
    fn next(&mut self) -> Option<Self::Item> {
        if self.offset + size_of::<AdvLoggerMessageEntry>() > self.log_info.log_current_offset as usize {
            None
        } else {
            let entry = unsafe { (self.log_info as *const AdvLoggerInfo).byte_add(self.offset) }
                as *const AdvLoggerMessageEntry;
            unsafe { entry.as_ref() }.map(|entry| {
                self.offset += entry.aligned_len();
                entry
            })
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use alloc::boxed::Box;
    use efi::PhysicalAddress;

    use super::*;

    #[test]
    fn create_fill_check_test() {
        let buff_box = Box::new([0_u8; 0x10000]);
        let buffer = buff_box.as_ref();
        let address = buffer as *const u8 as PhysicalAddress;
        let len = buffer.len() as u32;

        let log = unsafe { AdvLoggerInfo::initialize_memory_log(address, len) };

        // Fill the log.
        let mut entries: u32 = 0;
        loop {
            let data = entries.to_be_bytes();
            let entry = LogEntry { level: 0, phase: 0, timestamp: 0, data: &data };
            let log_entry = log.unwrap().add_log_entry(entry);
            if log_entry.is_none() {
                assert!(log.unwrap().discarded_size > 0);
                assert!(entries > 0);
                break;
            }
            entries += 1;
            let log_entry = log_entry.unwrap();
            assert_eq!(log_entry.get_message(), data);
        }

        // check the contents.
        let mut iter = log.unwrap().iter();
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

        let log = unsafe { AdvLoggerInfo::initialize_memory_log(address, len) };

        // Fill the log.
        for val in 0..50 {
            let data = (val as u32).to_be_bytes();
            let entry = LogEntry { level: 0, phase: 0, timestamp: 0, data: &data };
            let log_entry = log.unwrap().add_log_entry(entry).unwrap();
            assert_eq!(log_entry.get_message(), data);
        }

        // adopt the log.
        let log = unsafe { AdvLoggerInfo::adopt_memory_log(address) }.unwrap();

        // Add more entries.
        for val in 50..100 {
            let data = (val as u32).to_be_bytes();
            let entry = LogEntry { level: 0, phase: 0, timestamp: 0, data: &data };
            let log_entry = log.add_log_entry(entry).unwrap();
            assert_eq!(log_entry.get_message(), data);
        }

        // check the contents.
        assert!(log.discarded_size == 0);
        let mut iter = log.iter();
        for entry_num in 0..100 {
            let data = (entry_num as u32).to_be_bytes();
            let log_entry = iter.next().unwrap();
            assert_eq!(log_entry.get_message(), data);
        }

        assert!(iter.next().is_none());
    }
}
