//! This module defines everything used to extract performance records from HOBs.
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

use scroll::Pread;

use patina_sdk::{
    component::hob::{FromHob, Hob},
    guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE,
};

use crate::{
    error::Error,
    performance_record::{Iter, PerformanceRecordBuffer},
};

/// API to extract the performance data from HOB.
#[cfg_attr(test, automock)]
pub trait HobPerformanceDataExtractor {
    /// Extract the number of image loaded and the performance records from performance HOB.
    fn extract_hob_perf_data(&self) -> Result<(u32, PerformanceRecordBuffer), Error>;
}

/// Data inside an [`EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE`] guid hob.
#[derive(Debug, Default)]
pub struct HobPerformanceData {
    /// Number of images loaded.
    pub load_image_count: u32,
    /// Buffer containing performance records.
    pub records_data_buffer: Vec<u8>,
}

impl FromHob for HobPerformanceData {
    const HOB_GUID: r_efi::efi::Guid = EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE;

    fn parse(bytes: &[u8]) -> HobPerformanceData {
        let mut offset = 0;

        let Ok([size_of_all_entries, load_image_count, _hob_is_full]) = bytes.gread::<[u32; 3]>(&mut offset) else {
            log::error!("Performance: error while parsing HobPerformanceRecordBuffer, return default value.");
            return Self::default();
        };
        let records_data_buffer = bytes[offset..offset + size_of_all_entries as usize].to_vec();

        Self { load_image_count, records_data_buffer }
    }
}

impl HobPerformanceDataExtractor for Hob<'_, HobPerformanceData> {
    #[cfg(not(tarpaulin_include))]
    fn extract_hob_perf_data(&self) -> Result<(u32, PerformanceRecordBuffer), Error> {
        merge_hob_performance_buffer(self.iter())
    }
}

impl HobPerformanceDataExtractor for Option<Hob<'_, HobPerformanceData>> {
    // #[cfg(not(tarpaulin_include))]
    fn extract_hob_perf_data(&self) -> Result<(u32, PerformanceRecordBuffer), Error> {
        match self {
            Some(hob) => merge_hob_performance_buffer(hob.iter()),
            None => Ok((0, PerformanceRecordBuffer::new())),
        }
    }
}

fn merge_hob_performance_buffer<'a, T>(iter: T) -> Result<(u32, PerformanceRecordBuffer), Error>
where
    T: Iterator<Item = &'a HobPerformanceData>,
{
    let mut load_image_count = 0;
    let mut records = PerformanceRecordBuffer::new();

    for hob_performance_record_buffer in iter {
        load_image_count += hob_performance_record_buffer.load_image_count;
        for r in Iter::new(&hob_performance_record_buffer.records_data_buffer) {
            records.push_record(r)?;
        }
    }
    Ok((load_image_count, records))
}

#[cfg(test)]
pub mod test {
    use core::assert_eq;

    use patina_sdk::component::hob::FromHob;
    use scroll::Pwrite;

    use crate::performance_record::{GenericPerformanceRecord, PerformanceRecordBuffer};

    use super::{merge_hob_performance_buffer, HobPerformanceData};

    #[test]
    fn test_hob_performance_record_buffer_parse_from_hob() {
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

        let hob_perf_record_buffer = HobPerformanceData::parse(&buffer);

        assert_eq!(load_image_count, hob_perf_record_buffer.load_image_count);
        assert_eq!(perf_record_buffer.buffer(), hob_perf_record_buffer.records_data_buffer.as_slice());
    }

    #[test]
    fn test_hob_performance_record_buffer_parse_from_hob_invalid() {
        let buffer = [0_u8; 1];

        let hob_perf_record_buffer = HobPerformanceData::parse(&buffer);

        assert_eq!(0, hob_perf_record_buffer.load_image_count);
        assert!(hob_perf_record_buffer.records_data_buffer.is_empty());
    }

    #[test]
    fn test_merge_hob_performance_buffer() {
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
            HobPerformanceData { load_image_count: 1, records_data_buffer: perf_record_buffer_1.buffer().to_vec() },
            HobPerformanceData { load_image_count: 1, records_data_buffer: perf_record_buffer_2.buffer().to_vec() },
        ];

        let (loaded_image_count, perf_record_buffer) = merge_hob_performance_buffer(buffer.iter()).unwrap();

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
