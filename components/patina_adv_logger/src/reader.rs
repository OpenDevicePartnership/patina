//! UEFI Advanced Logger Reader Support
//!
//! This module provides read-only access to an Advanced Logger memory log buffer
//! and an iterator for traversing log entries.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{mem::size_of, slice, sync::atomic::Ordering};
use patina::error::{EfiError, Result};
use r_efi::efi;
use zerocopy::FromBytes;

use crate::memory_log::{AdvLoggerInfoRef, AdvLoggerMessageEntry, LogEntry};

/// A read-only handle to an advanced logger memory log. This provides the
/// ability to iterate over log entries without modifying the underlying buffer.
pub struct AdvancedLogReader<'a> {
    /// The header of the memory log.
    pub(crate) header: AdvLoggerInfoRef<'a>,
    /// The data portion of the memory log (read-only).
    data: &'a [u8],
}

impl AdvancedLogReader<'static> {
    /// Creates a read-only `AdvancedLogReader` from a physical address pointing to
    /// an existing advanced logger buffer.
    ///
    /// ### Safety
    ///
    /// The caller must ensure that the provided address points to a valid `AdvLoggerInfo`
    /// structure and that the memory is properly sized and initialized.
    pub unsafe fn from_address(address: efi::PhysicalAddress) -> Option<Self> {
        // SAFETY: The safety requirements for this function transfer to the function called here.
        let header = unsafe { AdvLoggerInfoRef::from_address(address)? };
        let data_size = header.log_buffer_size();
        let data_start = (address + header.log_buffer_offset() as u64) as *const u8;
        // SAFETY: The caller must ensure that the memory is properly sized and initialized.
        let data = unsafe { slice::from_raw_parts(data_start, data_size as usize) };

        Some(Self { header, data })
    }
}

impl<'a> AdvancedLogReader<'a> {
    /// Opens a log from a byte slice. This is used for parsing serialized log buffers
    /// such as those read from a file.
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

        Ok(Self { header, data: data_slice })
    }

    /// Returns an iterator over the log entries.
    pub fn iter(&self) -> AdvLogIterator<'_> {
        AdvLogIterator::new(self)
    }

    /// Returns the timer frequency.
    pub fn get_frequency(&self) -> u64 {
        self.header.timer_frequency().load(Ordering::Relaxed)
    }

    /// Returns whether hardware port writing is enabled for the given level.
    pub fn hardware_write_enabled(&self, level: u32) -> bool {
        !self.header.hw_port_disabled() && (level & self.header.hw_print_level() != 0)
    }

    /// Returns the address of the log buffer header.
    pub fn get_address(&self) -> efi::PhysicalAddress {
        self.header.as_ptr() as efi::PhysicalAddress
    }

    /// Returns the new logger info address if one has been set.
    pub fn get_new_logger_info_address(&self) -> Option<efi::PhysicalAddress> {
        self.header
            .new_logger_info_address()
            .and_then(|address| if address == 0 { None } else { Some(address as efi::PhysicalAddress) })
    }
}

/// Iterator for an advanced logger memory buffer log.
pub struct AdvLogIterator<'a> {
    log: &'a AdvancedLogReader<'a>,
    offset: usize,
}

/// Iterator for an Advanced Logger memory buffer.
impl<'a> AdvLogIterator<'a> {
    /// Creates a new log iterator from a given AdvancedLogReader reference.
    fn new(log: &'a AdvancedLogReader) -> Self {
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

            let header_slice = self.log.data.get(data_index..data_index + size_of::<AdvLoggerMessageEntry>())?;

            let entry_header = AdvLoggerMessageEntry::ref_from_bytes(header_slice).ok()?;
            data_index += size_of::<AdvLoggerMessageEntry>();

            if self.offset + size_of::<AdvLoggerMessageEntry>() + entry_header.message_length as usize
                > self.log.header.log_current_offset().load(Ordering::Relaxed) as usize
            {
                None
            } else {
                let entry_data = self.log.data.get(data_index..data_index + entry_header.message_length as usize)?;

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
    use core::{mem::size_of, sync::atomic::Ordering};

    use super::*;
    use crate::memory_log::*;

    #[test]
    fn open_log_supports_v5_and_v6() {
        let buffer_v5 = create_buffer_v5(123, false);
        let log_v5 = AdvancedLogReader::open_log(&buffer_v5).unwrap();
        assert_eq!(log_v5.get_frequency(), 123);
        assert!(log_v5.hardware_write_enabled(DEBUG_LEVEL_INFO));
        assert!(log_v5.get_new_logger_info_address().is_none());

        let buffer_v6 = create_buffer_v6(456, 0x1122334455667788);
        let log_v6 = AdvancedLogReader::open_log(&buffer_v6).unwrap();
        assert_eq!(log_v6.get_frequency(), 456);
        assert!(log_v6.get_new_logger_info_address().is_some());
    }

    #[test]
    fn open_log_rejects_unknown_version() {
        let mut buffer = create_buffer_v5(0, false);
        let header = buffer.as_mut_ptr().cast::<AdvLoggerInfoV5>();

        // SAFETY: The buffer contains a valid AdvLoggerInfoV5 header, so mutating the version field is in-bounds.
        unsafe {
            (*header).version = 7;
        }

        let result = AdvancedLogReader::open_log(&buffer);
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

        let result = AdvancedLogReader::open_log(&buffer);
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

        let result = AdvancedLogReader::open_log(&buffer);
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

        let result = AdvancedLogReader::open_log(&buffer);
        assert!(matches!(result, Err(EfiError::InvalidParameter)));
    }
}
