//! UEFI Advanced Logger Writer Support
//!
//! This module provides write-only access to an Advanced Logger memory log buffer.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{cell::UnsafeCell, mem::size_of, ptr, slice, sync::atomic::Ordering};
use patina::{
    base::align_up,
    error::{EfiError, Result},
};
use r_efi::efi;
use zerocopy::IntoBytes;

use crate::memory_log::{AdvLoggerInfo, AdvLoggerInfoRef, AdvLoggerMessageEntry, LogEntry};

/// A write-only handle to an advanced logger memory log. This provides the
/// ability to allocate and write log entries to the memory log buffer.
pub struct AdvancedLogWriter {
    /// The header of the memory log.
    pub header: AdvLoggerInfoRef<'static>,
    /// The data portion of the memory log.
    data: &'static UnsafeCell<[u8]>,
}

// SAFETY: The only interior mutability is the UnsafeCell for the data region of
//         the log. Safety here is checked by the allocation logic in add_log_entry
//         which relies on atomics to safely allocate portions of the buffer.
unsafe impl Send for AdvancedLogWriter {}
// SAFETY: See the Send safety comment
unsafe impl Sync for AdvancedLogWriter {}

impl AdvancedLogWriter {
    /// Initializes an `AdvancedLogWriter` from an existing advanced logger buffer at the
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

        Some(Self { header, data: UnsafeCell::from_mut(data) })
    }

    /// Initializes a new Advanced Log buffer at the provided address with the
    /// specified length.
    ///
    /// ### Safety
    ///
    /// The caller is responsible for ensuring that the provided address is appropriately
    /// allocated and accessible.
    #[allow(dead_code)]
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
            //         AdvancedLogWriter instance.
            unsafe { Self::adopt_memory_log(address) }
        }
    }

    /// Adds a log entry to the memory log buffer.
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

        // SAFETY: The space has been allocated. It should now be safe to write
        // data so long as it sticks to the range of the allocated entry. Get only
        // the allocated slice to maintain safety.
        let entry_slice = unsafe {
            let data: *mut [u8] = self.data.get();
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

    /// Returns whether hardware port writing is enabled for the given level.
    pub fn hardware_write_enabled(&self, level: u32) -> bool {
        !self.header.hw_port_disabled() && (level & self.header.hw_print_level() != 0)
    }

    /// Returns the timer frequency.
    pub fn get_frequency(&self) -> u64 {
        self.header.timer_frequency().load(Ordering::Relaxed)
    }

    /// Sets the timer frequency if not already set.
    pub fn set_frequency(&self, frequency: u64) {
        // try to swap, assuming the value is initially 0. If this fails, just continue.
        let _ = self.header.timer_frequency().compare_exchange(0, frequency, Ordering::Relaxed, Ordering::Relaxed);
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

    /// Returns the number of discarded bytes.
    #[allow(dead_code)]
    pub fn discarded_size(&self) -> u32 {
        self.header.discarded_size().load(Ordering::Relaxed)
    }
}

#[cfg(all(test, feature = "reader"))]
#[coverage(off)]
mod tests {
    extern crate std;
    use alloc::boxed::Box;
    use core::{mem::size_of, sync::atomic::Ordering};
    use efi::PhysicalAddress;

    use super::*;
    use crate::{memory_log::*, reader::AdvancedLogReader};

    #[test]
    fn create_fill_check_test() {
        let mut buff_box = Box::new([0_u64; 0x2000]);
        let buffer = buff_box.as_mut();
        let address = buffer as *mut u64 as PhysicalAddress;
        let len = buffer.len() as u32;

        // SAFETY: We just allocated this memory so it's valid.
        let writer = unsafe { AdvancedLogWriter::initialize_memory_log(address, len) }.unwrap();

        // Fill the log.
        let mut entries: u32 = 0;
        loop {
            let data = entries.to_be_bytes();
            let entry: LogEntry<'_> = LogEntry { level: 0, phase: 0, timestamp: 0, data: &data };
            let log_entry = writer.add_log_entry(entry);
            match log_entry {
                Ok(_) => {}
                Err(EfiError::OutOfResources) => {
                    assert!(writer.discarded_size() > 0);
                    assert!(entries > 0);
                    break;
                }
                Err(status) => {
                    panic!("Unexpected add_log_entry returned unexpected status {status:#x?}.")
                }
            }
            entries += 1;
        }

        // Use reader to verify the contents.
        // SAFETY: The buffer is still valid and was just written to.
        let reader = unsafe { AdvancedLogReader::from_address(address) }.unwrap();
        let mut iter = reader.iter();
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
        let writer = unsafe { AdvancedLogWriter::initialize_memory_log(address, len) }.unwrap();

        // Fill the log.
        for val in 0..50 {
            let data = (val as u32).to_be_bytes();
            let entry = LogEntry { level: 0, phase: 0, timestamp: 0, data: &data };
            writer.add_log_entry(entry).unwrap();
        }

        // SAFETY: This is the same buffer as before, still valid.
        let writer = unsafe { AdvancedLogWriter::adopt_memory_log(address) }.unwrap();

        // Add more entries.
        for val in 50..100 {
            let data = (val as u32).to_be_bytes();
            let entry = LogEntry { level: 0, phase: 0, timestamp: 0, data: &data };
            writer.add_log_entry(entry).unwrap();
        }

        // Use reader to verify the contents.
        // SAFETY: The buffer is still valid.
        let reader = unsafe { AdvancedLogReader::from_address(address) }.unwrap();
        assert!(writer.discarded_size() == 0);
        let mut iter = reader.iter();
        for entry_num in 0..100 {
            let data = (entry_num as u32).to_be_bytes();
            let log_entry = iter.next().unwrap();
            assert_eq!(log_entry.get_message(), data);
        }

        assert!(iter.next().is_none());
    }

    #[test]
    fn adopt_memory_log_accepts_v5_and_v6() {
        let mut buffer_v5 = create_buffer_v5(0, false);
        let address_v5 = buffer_v5.as_mut_ptr() as PhysicalAddress;
        // SAFETY: The memory was allocated successfully in this function and has been initialized
        // to contain a valid AdvLoggerInfoV5 header.
        let writer_v5 = unsafe { AdvancedLogWriter::adopt_memory_log(address_v5) }.unwrap();
        let entry_v5 = LogEntry { level: 0, phase: 0, timestamp: 0, data: b"v5" };
        writer_v5.add_log_entry(entry_v5).unwrap();

        // SAFETY: Buffer is still valid.
        let reader_v5 = unsafe { AdvancedLogReader::from_address(address_v5) }.unwrap();
        let mut iter = reader_v5.iter();
        assert_eq!(iter.next().unwrap().get_message(), b"v5");

        let mut buffer_v6 = create_buffer_v6(0, 0);
        let address_v6 = buffer_v6.as_mut_ptr() as PhysicalAddress;
        // SAFETY: The memory was allocated successfully in this function and has been initialized
        // to contain a valid AdvLoggerInfoV6 header.
        let writer_v6 = unsafe { AdvancedLogWriter::adopt_memory_log(address_v6) }.unwrap();
        let entry_v6 = LogEntry { level: 0, phase: 0, timestamp: 0, data: b"v6" };
        writer_v6.add_log_entry(entry_v6).unwrap();

        // SAFETY: Buffer is still valid.
        let reader_v6 = unsafe { AdvancedLogReader::from_address(address_v6) }.unwrap();
        let mut iter = reader_v6.iter();
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
        let result = unsafe { AdvancedLogWriter::adopt_memory_log(address) };
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
        let result = unsafe { AdvancedLogWriter::adopt_memory_log(address) };
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
        let result = unsafe { AdvancedLogWriter::adopt_memory_log(address) };
        assert!(result.is_none());
    }
}
