#[cfg(test)]
use mockall::automock;

use core::debug_assert;

use alloc::vec::Vec;
use r_efi::efi;
use scroll::Pread;
use uefi_sdk::{
    component::hob::{FromHob, Hob},
    guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE,
};

use crate::performance_record::{Iter, PerformanceRecordBuffer};

/// ...
#[cfg_attr(test, automock)]
pub trait PeiPerformanceDataExtractor {
    /// ...
    fn extract_pei_perf_data(&self) -> Result<(u32, PerformanceRecordBuffer), efi::Status>;
}

#[derive(Debug, Default)]
pub struct PeiPerformanceRecordBuffer {
    pub load_image_count: u32,
    pub records_data_buffer: Vec<u8>,
}

impl FromHob for PeiPerformanceRecordBuffer {
    const HOB_GUID: r_efi::efi::Guid = EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE;

    fn parse(bytes: &[u8]) -> PeiPerformanceRecordBuffer {
        let mut offset = 0;

        let Ok([size_of_all_entries, load_image_count, _hob_is_full]) = bytes.gread::<[u32; 3]>(&mut offset) else {
            debug_assert!(false);
            return Self::default();
        };
        let records_data_buffer = bytes[offset..offset + size_of_all_entries as usize].to_vec();

        Self { load_image_count, records_data_buffer }
    }
}

impl PeiPerformanceDataExtractor for Hob<'_, PeiPerformanceRecordBuffer> {
    fn extract_pei_perf_data(&self) -> Result<(u32, PerformanceRecordBuffer), efi::Status> {
        let mut pei_load_image_count = 0;
        let mut pei_records = PerformanceRecordBuffer::new();

        for pei_performance_record_buffer in self.iter() {
            pei_load_image_count += pei_performance_record_buffer.load_image_count;
            for r in Iter::new(&pei_performance_record_buffer.records_data_buffer) {
                pei_records.push_record(r)?;
            }
        }
        Ok((pei_load_image_count, pei_records))
    }
}
