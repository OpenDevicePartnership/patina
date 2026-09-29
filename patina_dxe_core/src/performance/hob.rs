//! Extraction of performance records carried over from earlier boot phases via HOBs.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::vec::Vec;
use core::iter::Iterator;

use patina::{
    component::hob::FromHob,
    performance::{
        error::Error,
        record::{Iter, PerformanceRecordBuffer},
    },
};

use scroll::Pread;

/// Data inside an [`patina::performance::guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE_GUID`] guid hob.
#[derive(Debug, Default)]
pub(crate) struct HobPerformanceData {
    /// Number of images loaded.
    pub load_image_count: u32,
    /// Buffer containing performance records.
    pub records_data_buffer: Vec<u8>,
}

impl FromHob for HobPerformanceData {
    const HOB_GUID: patina::BinaryGuid = patina::BinaryGuid::from_string("3B387BFD-7ABC-4CF2-A0CA-B6A16C1B1B25");

    fn parse(bytes: &[u8]) -> HobPerformanceData {
        let mut offset = 0;

        let Ok([size_of_all_entries, load_image_count, _hob_is_full]) = bytes.gread::<[u32; 3]>(&mut offset) else {
            log::error!("Performance: error while parsing HobPerformanceRecordBuffer, return default value.");
            return Self::default();
        };
        let records_data_buffer = bytes
            .get(offset..offset + size_of_all_entries as usize)
            .unwrap_or_else(|| {
                debug_assert!(false, "Performance: records_data_buffer slice out of bounds");
                &[]
            })
            .to_vec();

        Self { load_image_count, records_data_buffer }
    }
}

/// Merges the performance records from an iterator of [`HobPerformanceData`] into a single
/// [`PerformanceRecordBuffer`], returning the total load-image count and the merged records.
pub(crate) fn merge_hob_performance_buffer<'a, T>(iter: T) -> Result<(u32, PerformanceRecordBuffer), Error>
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
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use core::assert_eq;

    use scroll::Pwrite;

    use super::{HobPerformanceData, merge_hob_performance_buffer};
    use crate::performance::push_generic_record;
    use patina::{component::hob::FromHob, performance::record::PerformanceRecordBuffer};

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
        push_generic_record(&mut perf_record_buffer, 1, 1, &[1_u8, 2, 3, 4, 5]);

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
        push_generic_record(&mut perf_record_buffer_1, 1, 1, &[1_u8, 2, 3, 4, 5]);

        let mut perf_record_buffer_2 = PerformanceRecordBuffer::new();
        push_generic_record(&mut perf_record_buffer_2, 1, 1, &[10_u8, 20, 30, 40, 50]);

        let buffer = [
            HobPerformanceData { load_image_count: 1, records_data_buffer: perf_record_buffer_1.buffer().to_vec() },
            HobPerformanceData { load_image_count: 1, records_data_buffer: perf_record_buffer_2.buffer().to_vec() },
        ];

        let (loaded_image_count, perf_record_buffer) = merge_hob_performance_buffer(buffer.iter()).unwrap();

        let mut expected_perf_record_buffer = PerformanceRecordBuffer::new();
        push_generic_record(&mut expected_perf_record_buffer, 1, 1, &[1_u8, 2, 3, 4, 5]);
        push_generic_record(&mut expected_perf_record_buffer, 1, 1, &[10_u8, 20, 30, 40, 50]);

        assert_eq!(2, loaded_image_count);
        assert_eq!(expected_perf_record_buffer.buffer(), perf_record_buffer.buffer());
    }
}
