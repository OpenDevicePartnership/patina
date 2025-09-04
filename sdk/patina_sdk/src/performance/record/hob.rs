//! This module implements the functionality necessary to extract performance records from HOBs.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

use alloc::vec::Vec;
use core::iter::Iterator;

use crate::{
    component::hob::{FromHob, Hob},
    guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE,
    performance::{
        error::Error,
        record::{Iter, PerformanceRecordBuffer},
    },
};

use scroll::Pread;

/// API to extract the performance data from HOB.
#[cfg_attr(any(test, feature = "mockall"), automock)]
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
    #[coverage(off)]
    fn extract_hob_perf_data(&self) -> Result<(u32, PerformanceRecordBuffer), Error> {
        merge_hob_performance_buffer(self.iter())
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
#[coverage(off)]
pub mod tests {
    use core::assert_eq;

    use scroll::Pwrite;

    use super::{HobPerformanceData, merge_hob_performance_buffer};
    use crate::{
        component::hob::FromHob,
        performance::record::{GenericPerformanceRecord, PerformanceRecordBuffer},
    };

    #[test]
    fn test_merge_hob_performance_buffer_with_none() {
        let buffer: Option<Vec<HobPerformanceData>> = None;

        let result = match buffer {
            Some(data) => merge_hob_performance_buffer(data.iter()),
            None => Ok((0, PerformanceRecordBuffer::new())),
        };

        assert!(result.is_ok());
        let (load_image_count, perf_record_buffer) = result.unwrap();
        assert_eq!(load_image_count, 0);
        assert!(perf_record_buffer.buffer().is_empty());
    }

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
