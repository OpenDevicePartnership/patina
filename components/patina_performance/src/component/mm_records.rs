//! Patina MM Performance Record Collector
//!
//! Collects Management Mode (MM) performance records at Ready-to-Boot and adds them to the FBPT through the
//! [`PerformanceManager`] service.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::{string::String, vec::Vec};
use core::cell::Cell;

use patina::{
    component::{
        component,
        service::{
            Service,
            performance::PerformanceManager,
            uefi_services::event::{EventServices, EventServicesExt, Tpl},
        },
    },
    error::EfiError,
    performance::record::{GenericPerformanceRecord, PerformanceRecordHeader, print_record_details, record_type_name},
    uefi::event::READY_TO_BOOT_EVENT_GROUP_GUID,
};
use patina_mm::component::communicator::MmCommunication;

use crate::mm;

/// Collects MM performance records at Ready-to-Boot and adds them to the FBPT.
///
/// ## Example Usage
///
/// ```rust
/// use patina_performance::component::mm_records::*;
///
/// let component = MmRecordCollector::new();
/// ```
#[derive(Default)]
pub struct MmRecordCollector;

#[component]
impl MmRecordCollector {
    /// Creates a new instance of the component.
    pub const fn new() -> Self {
        Self
    }

    /// Requires [`MmCommunication`] as a real (not optional) dependency, so the dispatcher itself skips this
    /// component on platforms with no MM communication region, instead of dispatching unconditionally and
    /// branching on an `Option` at runtime.
    fn entry_point(
        self,
        performance: Service<dyn PerformanceManager>,
        mm_comm_service: Service<dyn MmCommunication>,
        events: Service<dyn EventServices>,
    ) -> Result<(), EfiError> {
        // The event group may fire more than once. Only the first signal should collect the records.
        let collected = Cell::new(false);
        events.on_event_group(READY_TO_BOOT_EVENT_GROUP_GUID, Tpl::Callback, move || {
            if collected.replace(true) {
                return;
            }
            if let Err(e) = process_mm_performance_records(&mm_comm_service, &performance) {
                log::error!("Performance: {e}");
            }
        })?;

        Ok(())
    }
}

/// Error types for MM performance record operations
#[derive(Debug)]
enum MmPerformanceError {
    /// MM communication failed to send or receive data
    Communication(patina_mm::component::communicator::Status),
    /// Failed to parse response data from MM
    ParseError,
    /// An MM operation returned a non-success EFI status code
    StatusError(patina::standard::efi::Status),
    /// An error occurred while processing performance record data
    RecordError(String),
}

impl core::fmt::Display for MmPerformanceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MmPerformanceError::Communication(status) => write!(f, "MmCommunication error: {status:?}"),
            MmPerformanceError::ParseError => write!(f, "Failed to parse MM response"),
            MmPerformanceError::StatusError(status) => {
                write!(f, "MM operation failed with status: 0x{:x}", status.as_usize())
            }
            MmPerformanceError::RecordError(msg) => write!(f, "Record processing error: {msg}"),
        }
    }
}

impl core::error::Error for MmPerformanceError {}

/// Fetches the total size of MM performance records
fn fetch_mm_record_size(comm_service: &Service<dyn MmCommunication>) -> Result<usize, MmPerformanceError> {
    let mut size_req_buf = [0u8; mm::SMM_COMM_HEADER_SIZE];
    mm::GetRecordSize::new()
        .write_into(&mut size_req_buf)
        .map_err(|()| MmPerformanceError::RecordError("Failed to write GetRecordSize request".into()))?;

    let size_resp_bytes = comm_service
        .communicate(1, &size_req_buf, mm::EFI_FIRMWARE_PERFORMANCE_GUID.as_guid())
        .map_err(MmPerformanceError::Communication)?;

    let (size_resp, _) = mm::GetRecordSize::read_from(&size_resp_bytes).map_err(|_| MmPerformanceError::ParseError)?;

    if size_resp.return_status != patina::standard::efi::Status::SUCCESS {
        return Err(MmPerformanceError::StatusError(size_resp.return_status));
    }

    Ok(size_resp.boot_record_size)
}

/// Fetches a chunk of MM performance record data
fn fetch_mm_record_chunk(
    comm_service: &Service<dyn MmCommunication>,
    offset: usize,
    chunk_size: usize,
) -> Result<Vec<u8>, MmPerformanceError> {
    let mut data_req = mm::GetRecordDataByOffset::new_default(offset);
    data_req.boot_record_data_size = chunk_size;

    let buffer_size = mm::SMM_COMM_HEADER_SIZE + chunk_size;
    let mut data_req_buf = alloc::vec![0u8; buffer_size];

    data_req
        .write_into(&mut data_req_buf)
        .map_err(|()| MmPerformanceError::RecordError("Failed to write GetRecordDataByOffset request".into()))?;

    let data_resp_bytes = comm_service
        .communicate(1, &data_req_buf, mm::EFI_FIRMWARE_PERFORMANCE_GUID.as_guid())
        .map_err(MmPerformanceError::Communication)?;

    let (data_resp, _) =
        mm::GetRecordDataByOffset::read_from_default(&data_resp_bytes).map_err(|_| MmPerformanceError::ParseError)?;

    if data_resp.return_status != patina::standard::efi::Status::SUCCESS {
        return Err(MmPerformanceError::StatusError(data_resp.return_status));
    }

    let actual_size = core::cmp::min(chunk_size, data_resp.boot_record_data().len());
    Ok(data_resp.boot_record_data().get(..actual_size).ok_or(MmPerformanceError::ParseError)?.to_vec())
}

/// Fetches all MM performance record data using chunked requests
fn fetch_all_mm_record_data(comm_service: &Service<dyn MmCommunication>) -> Result<Vec<u8>, MmPerformanceError> {
    let total_size = fetch_mm_record_size(comm_service)?;

    if total_size > mm::MAX_SMM_BOOT_RECORD_BYTES {
        log::warn!(
            "Performance: MM reported {} boot record bytes which exceeds our safety cap ({}), clamping.",
            total_size,
            mm::MAX_SMM_BOOT_RECORD_BYTES
        );
    }

    let min_size = core::cmp::min(total_size, mm::MAX_SMM_BOOT_RECORD_BYTES);
    if min_size == 0 {
        log::info!("Performance: MM reported 0 performance bytes.");
        return Ok(Vec::new());
    }

    let mut result = Vec::with_capacity(min_size);

    while result.len() < min_size {
        let remaining = min_size - result.len();
        let chunk_size = core::cmp::min(mm::SMM_FETCH_CHUNK_BYTES, remaining);
        let chunk = fetch_mm_record_chunk(comm_service, result.len(), chunk_size)?;
        result.extend_from_slice(&chunk);
    }

    Ok(result)
}

/// Iterator over performance records from raw byte data
struct PerformanceRecordIterator<'a> {
    bytes: &'a [u8],
}

impl<'a> PerformanceRecordIterator<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

impl<'a> Iterator for PerformanceRecordIterator<'a> {
    type Item = Result<&'a GenericPerformanceRecord, MmPerformanceError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.bytes.len() < PerformanceRecordHeader::SIZE {
            return None;
        }

        let header = match PerformanceRecordHeader::try_from(self.bytes) {
            Ok(h) => h,
            Err(err) => {
                self.bytes = self.bytes.get(1..).unwrap_or(&[]);
                return Some(Err(MmPerformanceError::RecordError(err.into())));
            }
        };

        let rec_len = header.length as usize;
        if rec_len < PerformanceRecordHeader::SIZE {
            self.bytes = self.bytes.get(PerformanceRecordHeader::SIZE..).unwrap_or(&[]);
            return Some(Err(MmPerformanceError::RecordError(alloc::format!(
                "Record reports too small length {} (< {})",
                rec_len,
                PerformanceRecordHeader::SIZE
            ))));
        }

        if rec_len > self.bytes.len() {
            let available = self.bytes.len();
            self.bytes = &[];
            return Some(Err(MmPerformanceError::RecordError(alloc::format!(
                "Truncated record (needed {rec_len}, had {available})"
            ))));
        }

        let record_bytes = self.bytes.get(..rec_len)?;
        let record = match GenericPerformanceRecord::ref_from_bytes(record_bytes) {
            Ok(record) => record,
            Err(err) => {
                self.bytes = &[];
                return Some(Err(MmPerformanceError::RecordError(alloc::format!("Failed to parse record: {err:?}"))));
            }
        };

        self.bytes = self.bytes.get(rec_len..).unwrap_or(&[]);
        Some(Ok(record))
    }
}

/// Processes MM performance records and adds them to the FBPT
fn process_mm_performance_records(
    comm_service: &Service<dyn MmCommunication>,
    performance: &Service<dyn PerformanceManager>,
) -> Result<(), MmPerformanceError> {
    let record_data = fetch_all_mm_record_data(comm_service)?;

    if record_data.is_empty() {
        return Ok(());
    }

    log::info!("Performance: Processing {} bytes of MM performance data", record_data.len());

    let record_iter = PerformanceRecordIterator::new(&record_data);
    let mut record_count = 0;
    let mut success_count = 0;
    let mut error_count = 0;

    for record_result in record_iter {
        match record_result {
            Ok(record) => {
                record_count += 1;

                // Copy packed header fields into locals to avoid unaligned references.
                let record_type = record.header.record_type;
                let length = record.header.length;
                let revision = record.header.revision;

                log::debug!(
                    "Performance: MM record #{} - type: 0x{:04X} ({}), length: {}, revision: {}, data_len: {}",
                    record_count,
                    record_type,
                    record_type_name(record_type),
                    length,
                    revision,
                    record.data.len()
                );
                // Print detailed record information based on type
                print_record_details(record_type, record_count, &record.data);

                if let Err(e) = performance.add_generic_record(record) {
                    error_count += 1;
                    log::error!("Performance: Failed adding MM record #{record_count}: {e:?}");
                } else {
                    success_count += 1;
                }
            }
            Err(e) => {
                log::warn!("Performance: {e}");
                continue;
            }
        }
    }

    log::debug!(
        "Performance: MM record summary - total: {record_count}, added: {success_count}, failed: {error_count}"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use core::cell::Cell as StdCell;
    use patina::component::service::{performance::MockPerformanceManager, uefi_services::event::MockEventServices};
    use patina_mm::component::communicator::Status;
    use zerocopy::IntoBytes;

    const TEST_PERFORMANCE_RECORD_TYPE: u16 = 0x1010;
    const TEST_PERFORMANCE_RECORD_LENGTH: u8 = 34;
    const TEST_PERFORMANCE_RECORD_REVISION: u8 = 1;
    const TEST_RECORD_ID_BASE: u16 = 1;
    const TEST_TIMESTAMP_BASE: u64 = 100;
    const TEST_MULTI_CHUNK_RECORD_COUNT: usize = 40;
    const TEST_MM_COMM_FUNCTION_ID_SIZE: u64 = 1;
    const TEST_MM_COMM_FUNCTION_ID_DATA: u64 = 3;
    const TEST_MM_COMM_RESPONSE_SIZE: usize = 40;
    const TEST_SMM_FETCH_CHUNK_BYTES: usize = mm::SMM_FETCH_CHUNK_BYTES;
    const TEST_MM_COMM_DATA_RESPONSE_SIZE: usize = TEST_MM_COMM_RESPONSE_SIZE + TEST_SMM_FETCH_CHUNK_BYTES;

    /// Creates a test performance record with the specified ID and timestamp
    macro_rules! create_test_record {
        ($id:expr, $timestamp:expr) => {{
            let mut record = [0u8; TEST_PERFORMANCE_RECORD_LENGTH as usize];
            record[0..2].copy_from_slice(&TEST_PERFORMANCE_RECORD_TYPE.to_le_bytes());
            record[2] = TEST_PERFORMANCE_RECORD_LENGTH;
            record[3] = TEST_PERFORMANCE_RECORD_REVISION;
            record[4..6].copy_from_slice(&$id.to_le_bytes());
            record[6..10].copy_from_slice(&0u32.to_le_bytes());
            record[10..18].copy_from_slice(&$timestamp.to_le_bytes());
            record
        }};
    }

    /// Creates a test MM communication size response
    macro_rules! create_size_response {
        ($boot_record_size:expr) => {{
            let mut response = alloc::vec![0u8; TEST_MM_COMM_RESPONSE_SIZE];
            response[0..8].copy_from_slice(&TEST_MM_COMM_FUNCTION_ID_SIZE.to_le_bytes());
            response[16..24].copy_from_slice(&$boot_record_size.to_le_bytes());
            response
        }};
    }

    fn mock_events_registering_once(fires: u32) -> MockEventServices {
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().once().returning(move |_, _, mut callback| {
            for _ in 0..fires {
                callback();
            }
            Ok(patina::component::service::uefi_services::event::Event::from_raw(
                core::ptr::NonNull::<core::ffi::c_void>::dangling().as_ptr(),
            )
            .unwrap())
        });
        events
    }

    #[test]
    fn test_mm_record_collector_entry_point_registers_ready_to_boot_event() {
        let mut events = MockEventServices::new();
        events
            .expect_create_event_for_group()
            .once()
            .withf(|group, tpl, _callback| {
                assert_eq!(group, &READY_TO_BOOT_EVENT_GROUP_GUID);
                assert_eq!(tpl, &Tpl::Callback);
                true
            })
            .returning(|_, _, _| {
                Ok(patina::component::service::uefi_services::event::Event::from_raw(
                    core::ptr::NonNull::<core::ffi::c_void>::dangling().as_ptr(),
                )
                .unwrap())
            });

        struct ZeroSizeComm;
        impl MmCommunication for ZeroSizeComm {
            fn communicate<'a>(
                &self,
                _id: u8,
                _data_buffer: &[u8],
                _recipient: patina::Guid<'a>,
            ) -> Result<Vec<u8>, Status> {
                Ok(create_size_response!(0u64))
            }
        }

        let result = MmRecordCollector::new().entry_point(
            Service::mock(Box::new(MockPerformanceManager::new())),
            Service::mock(Box::new(ZeroSizeComm)),
            Service::mock(Box::new(events)),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_mm_record_collector_runs_once_with_zero_records() {
        struct ZeroSizeComm;
        impl MmCommunication for ZeroSizeComm {
            fn communicate<'a>(
                &self,
                _id: u8,
                _data_buffer: &[u8],
                _recipient: patina::Guid<'a>,
            ) -> Result<Vec<u8>, Status> {
                Ok(create_size_response!(0u64))
            }
        }

        let records = alloc::sync::Arc::new(core::sync::atomic::AtomicUsize::new(0));
        let records_clone = records.clone();
        let mut performance = MockPerformanceManager::new();
        performance.expect_add_generic_record().returning(move |_| {
            records_clone.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            Ok(())
        });

        // Fires the ready-to-boot group twice. The collector must only process it once.
        let events = mock_events_registering_once(2);

        let result = MmRecordCollector::new().entry_point(
            Service::mock(Box::new(performance)),
            Service::mock(Box::new(ZeroSizeComm)),
            Service::mock(Box::new(events)),
        );

        assert!(result.is_ok());
        assert_eq!(records.load(core::sync::atomic::Ordering::Relaxed), 0);
    }

    #[test]
    fn test_mm_record_collector_runs_once_with_one_record() {
        struct OneRecordComm {
            step: StdCell<u8>,
        }
        impl MmCommunication for OneRecordComm {
            fn communicate<'a>(
                &self,
                _id: u8,
                _data_buffer: &[u8],
                _recipient: patina::Guid<'a>,
            ) -> Result<Vec<u8>, Status> {
                if self.step.get() == 0 {
                    self.step.set(1);
                    Ok(create_size_response!(u64::from(TEST_PERFORMANCE_RECORD_LENGTH)))
                } else {
                    self.step.set(2);
                    let record = create_test_record!(TEST_RECORD_ID_BASE, TEST_TIMESTAMP_BASE + 23);
                    let mut response = alloc::vec![0u8; TEST_MM_COMM_DATA_RESPONSE_SIZE];
                    response[0..8].copy_from_slice(&TEST_MM_COMM_FUNCTION_ID_DATA.to_le_bytes());
                    response[16..24].copy_from_slice(&(record.len() as u64).to_le_bytes());
                    response[TEST_MM_COMM_RESPONSE_SIZE..TEST_MM_COMM_RESPONSE_SIZE + record.len()]
                        .copy_from_slice(&record);
                    Ok(response)
                }
            }
        }

        let records = alloc::sync::Arc::new(core::sync::atomic::AtomicUsize::new(0));
        let records_clone = records.clone();
        let mut performance = MockPerformanceManager::new();
        performance.expect_add_generic_record().returning(move |_| {
            records_clone.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            Ok(())
        });

        // Fires the ready-to-boot group twice. The collector must only process it once.
        let events = mock_events_registering_once(2);

        let result = MmRecordCollector::new().entry_point(
            Service::mock(Box::new(performance)),
            Service::mock(Box::new(OneRecordComm { step: StdCell::new(0) })),
            Service::mock(Box::new(events)),
        );

        assert!(result.is_ok());
        assert_eq!(records.load(core::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn test_mm_record_collector_multi_chunk() {
        const TOTAL_RECORD_BYTES: usize = TEST_PERFORMANCE_RECORD_LENGTH as usize * TEST_MULTI_CHUNK_RECORD_COUNT;

        let mut all_records = Vec::with_capacity(TOTAL_RECORD_BYTES);
        for i in 0..TEST_MULTI_CHUNK_RECORD_COUNT {
            let record = create_test_record!(TEST_RECORD_ID_BASE + i as u16, TEST_TIMESTAMP_BASE + i as u64);
            all_records.extend_from_slice(&record);
        }

        struct MultiChunks {
            buf: Vec<u8>,
        }
        impl MmCommunication for MultiChunks {
            fn communicate<'a>(&self, _id: u8, data: &[u8], _: patina::Guid<'a>) -> Result<Vec<u8>, Status> {
                let mut f = [0u8; 8];
                f.copy_from_slice(&data[0..8]);
                match u64::from_le_bytes(f) {
                    fid if fid == TEST_MM_COMM_FUNCTION_ID_SIZE => Ok(create_size_response!(self.buf.len() as u64)),
                    fid if fid == TEST_MM_COMM_FUNCTION_ID_DATA => {
                        let mut ask_buffer = [0u8; 8];
                        ask_buffer.copy_from_slice(&data[16..24]);
                        let ask = u64::from_le_bytes(ask_buffer) as usize;
                        let mut offset_buffer = [0u8; 8];
                        offset_buffer.copy_from_slice(&data[32..40]);
                        let offset = u64::from_le_bytes(offset_buffer) as usize;
                        let remaining: usize = self.buf.len() - offset;
                        let take = core::cmp::min(ask, remaining);
                        let mut r = alloc::vec![0u8; TEST_MM_COMM_RESPONSE_SIZE + ask];
                        r[0..8].copy_from_slice(&TEST_MM_COMM_FUNCTION_ID_DATA.to_le_bytes());
                        r[16..24].copy_from_slice(&(take as u64).to_le_bytes());
                        r[TEST_MM_COMM_RESPONSE_SIZE..TEST_MM_COMM_RESPONSE_SIZE + take]
                            .copy_from_slice(&self.buf[offset..offset + take]);
                        Ok(r)
                    }
                    _ => Err(Status::InvalidDataBuffer),
                }
            }
        }

        let records = alloc::sync::Arc::new(core::sync::atomic::AtomicUsize::new(0));
        let records_clone = records.clone();
        let mut performance = MockPerformanceManager::new();
        performance.expect_add_generic_record().returning(move |_| {
            records_clone.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            Ok(())
        });

        let events = mock_events_registering_once(1);

        let result = MmRecordCollector::new().entry_point(
            Service::mock(Box::new(performance)),
            Service::mock(Box::new(MultiChunks { buf: all_records })),
            Service::mock(Box::new(events)),
        );

        assert!(result.is_ok());
        assert_eq!(records.load(core::sync::atomic::Ordering::Relaxed), TEST_MULTI_CHUNK_RECORD_COUNT);
    }

    /// Verifies that malformed record data doesn't cause infinite loops.
    #[test]
    fn test_mm_record_collector_iterator_infinite_loop_does_not_occur_truncation() {
        // Truncated record - header claims more bytes of data than are actually available
        // Claims 100 bytes, but only 6 bytes are present (4-byte header + 2 extra bytes)
        let truncated_header =
            PerformanceRecordHeader::new(TEST_PERFORMANCE_RECORD_TYPE, 100, TEST_PERFORMANCE_RECORD_REVISION);

        let mut truncated_data = alloc::vec![0u8; 6];
        truncated_data[..PerformanceRecordHeader::SIZE].copy_from_slice(truncated_header.as_bytes());

        let iter = PerformanceRecordIterator::new(&truncated_data);
        let mut iterations = 0;
        let mut error_occurred = false;

        for result in iter {
            iterations += 1;
            assert!(iterations < 10, "Iterator did not terminate - infinite loop detected!");

            if result.is_err() {
                error_occurred = true;
            }
        }

        assert!(error_occurred, "Expected error for truncated record");
        assert_eq!(iterations, 1, "Should terminate after one error");
    }

    #[test]
    fn test_mm_record_collector_iterator_infinite_loop_does_not_occur_invalid_len() {
        // Invalid: length=1 < header size=4
        let invalid_length_header =
            PerformanceRecordHeader::new(TEST_PERFORMANCE_RECORD_TYPE, 1, TEST_PERFORMANCE_RECORD_REVISION);
        let mut invalid_length_data = alloc::vec![0u8; 20];
        invalid_length_data[..PerformanceRecordHeader::SIZE].copy_from_slice(invalid_length_header.as_bytes());

        let iter = PerformanceRecordIterator::new(&invalid_length_data);
        let mut iterations = 0;
        let mut error_occurred = false;

        for result in iter {
            iterations += 1;
            assert!(iterations < 10, "Iterator did not terminate - infinite loop detected!");

            if result.is_err() {
                error_occurred = true;
            }
        }

        assert!(error_occurred, "Expected error for invalid length");
        assert!(iterations <= 5, "Should terminate quickly without infinite loop");
    }
}
