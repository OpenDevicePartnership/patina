//! This module defines everything used to extract performance records from the PEI phase.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

#[cfg(test)]
use mockall::automock;

use alloc::vec::Vec;
use core::iter::Iterator;

use r_efi::efi;
use scroll::Pread;

use uefi_sdk::{
    component::hob::{FromHob, Hob},
    guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE,
};

use crate::performance_record::{Iter, PerformanceRecordBuffer};

/// API to extract the performance data from the PEI phase.
#[cfg_attr(test, automock)]
pub trait PeiPerformanceDataExtractor {
    /// Extract the number of image loaded and the performance records from the PEI phase.
    fn extract_pei_perf_data(&self) -> Result<(u32, PerformanceRecordBuffer), efi::Status>;
}

/// Data inside an [`EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE`] guid hob.
#[derive(Debug, Default)]
pub struct PeiPerformanceData {
    /// Number of images loaded.
    pub load_image_count: u32,
    /// Buffer containing performance records.
    pub records_data_buffer: Vec<u8>,
}

impl FromHob for PeiPerformanceData {
    const HOB_GUID: r_efi::efi::Guid = EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE;

    fn parse(bytes: &[u8]) -> PeiPerformanceData {
        let mut offset = 0;

        let Ok([size_of_all_entries, load_image_count, _hob_is_full]) = bytes.gread::<[u32; 3]>(&mut offset) else {
            log::error!("Performance Lib: error while parsing PeiPerformanceRecordBuffer, return default value.");
            return Self::default();
        };
        let records_data_buffer = bytes[offset..offset + size_of_all_entries as usize].to_vec();

        Self { load_image_count, records_data_buffer }
    }
}

impl PeiPerformanceDataExtractor for Hob<'_, PeiPerformanceData> {
    #[cfg(not(tarpaulin_include))]
    fn extract_pei_perf_data(&self) -> Result<(u32, PerformanceRecordBuffer), efi::Status> {
        merge_pei_performance_buffer(self.iter())
    }
}

fn merge_pei_performance_buffer<'a, T>(iter: T) -> Result<(u32, PerformanceRecordBuffer), efi::Status>
where
    T: Iterator<Item = &'a PeiPerformanceData>,
{
    let mut pei_load_image_count = 0;
    let mut pei_records = PerformanceRecordBuffer::new();

    for pei_performance_record_buffer in iter {
        pei_load_image_count += pei_performance_record_buffer.load_image_count;
        for r in Iter::new(&pei_performance_record_buffer.records_data_buffer) {
            pei_records.push_record(r)?;
        }
    }
    Ok((pei_load_image_count, pei_records))
}

#[cfg(test)]
pub mod test {
    use core::assert_eq;

    use scroll::Pwrite;
    use uefi_sdk::component::hob::FromHob;

    use crate::performance_record::{GenericPerformanceRecord, PerformanceRecordBuffer};

    use super::{merge_pei_performance_buffer, PeiPerformanceData};

    #[test]
    fn test_pei_performance_record_buffer_parse_from_hob() {
        let mut buffer = [0_u8; 32];
        let mut offset = 0;

        let mut perf_record_buffer = PerformanceRecordBuffer::new();
        perf_record_buffer
            .push_record(GenericPerformanceRecord { record_type: 1, length: 5, revision: 1, data: [1_u8, 2, 3, 4, 5] })
            .unwrap();

        let size_of_all_entries = perf_record_buffer.size() as u32;
        let load_image_count = 12_u32;
        let hob_is_full = 0_u32;

        buffer.gwrite(size_of_all_entries, &mut offset).unwrap();
        buffer.gwrite(load_image_count, &mut offset).unwrap();
        buffer.gwrite(hob_is_full, &mut offset).unwrap();
        buffer.gwrite(perf_record_buffer.buffer(), &mut offset).unwrap();

        let pei_perf_record_buffer = PeiPerformanceData::parse(&buffer);

        assert_eq!(load_image_count, pei_perf_record_buffer.load_image_count);
        assert_eq!(perf_record_buffer.buffer(), pei_perf_record_buffer.records_data_buffer.as_slice());
    }

    #[test]
    fn test_pei_performance_record_buffer_parse_from_hob_invalid() {
        let buffer = [0_u8; 1];

        let pei_perf_record_buffer = PeiPerformanceData::parse(&buffer);

        assert_eq!(0, pei_perf_record_buffer.load_image_count);
        assert!(pei_perf_record_buffer.records_data_buffer.is_empty());
    }

    #[test]
    fn test_merge_pei_performance_buffer() {
        let mut perf_record_buffer_1 = PerformanceRecordBuffer::new();
        perf_record_buffer_1
            .push_record(GenericPerformanceRecord { record_type: 1, length: 5, revision: 1, data: [1_u8, 2, 3, 4, 5] })
            .unwrap();

        let mut perf_record_buffer_2 = PerformanceRecordBuffer::new();
        perf_record_buffer_2
            .push_record(GenericPerformanceRecord {
                record_type: 1,
                length: 9,
                revision: 1,
                data: [10_u8, 20, 30, 40, 50],
            })
            .unwrap();

        let buffer = [
            PeiPerformanceData { load_image_count: 1, records_data_buffer: perf_record_buffer_1.buffer().to_vec() },
            PeiPerformanceData { load_image_count: 1, records_data_buffer: perf_record_buffer_2.buffer().to_vec() },
        ];

        let (loaded_image_count, perf_record_buffer) = merge_pei_performance_buffer(buffer.iter()).unwrap();

        let mut expected_perf_record_buffer = PerformanceRecordBuffer::new();
        expected_perf_record_buffer
            .push_record(GenericPerformanceRecord { record_type: 1, length: 9, revision: 1, data: [1_u8, 2, 3, 4, 5] })
            .unwrap();
        expected_perf_record_buffer
            .push_record(GenericPerformanceRecord {
                record_type: 1,
                length: 9,
                revision: 1,
                data: [10_u8, 20, 30, 40, 50],
            })
            .unwrap();

        assert_eq!(2, loaded_image_count);
        assert_eq!(expected_perf_record_buffer.buffer(), perf_record_buffer.buffer());
    }
}
