//! Patina Performance Component
//!
//! Publishes the firmware performance data produced by the DXE Core to the rest of the UEFI environment. This crate
//! acts as the UEFI/ACPI translation layer for the performance implementation in the DXE Core.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::{
    component::{
        protocol::{
            EdkiiPerformanceMeasurementProtocol, create_performance_measurement_efiapi, set_performance_service,
        },
        table::find_previous_table_address,
    },
    mm,
};
use alloc::{boxed::Box, string::String, vec::Vec};
use core::ffi::c_void;
use patina::standard::efi::EVENT_GROUP_READY_TO_BOOT;
use patina::{
    UEFI_PAGE_SIZE,
    component::{
        component,
        service::{Service, perf_timer::ArchTimerFunctionality, performance::PerformanceManager},
    },
    error::EfiError,
    performance::{
        guid::{EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE_GUID, PERFORMANCE_PROTOCOL_GUID},
        measurement::PerformanceProperty,
        record::{GenericPerformanceRecord, PerformanceRecordHeader, print_record_details, record_type_name},
    },
    pi::{
        protocol::status_code,
        status_code::{EFI_PROGRESS_CODE, EFI_SOFTWARE_DXE_BS_DRIVER},
    },
    uefi::{
        boot_services::{BootServices, StandardBootServices, allocation::AllocType, tpl::Tpl},
        event::EventType,
        memory::EfiMemoryType,
        runtime_services::{RuntimeServices, StandardRuntimeServices},
    },
};
use patina_mm::component::communicator::MmCommunication;

use patina::function;

use patina::pi::event::END_OF_DXE_EVENT_GROUP_GUID;

/// Context parameter for the Ready-to-Boot event callback that fetches MM performance records.
type MmPerformanceEventContext<B> = Box<(B, Service<dyn PerformanceManager>, Service<dyn MmCommunication>)>;

/// Context parameter for the End-of-DXE event callback that publishes the FBPT.
type ReportFbptEventContext<B, R> = Box<(B, R, Service<dyn PerformanceManager>)>;

/// Performance component.
///
/// This component provides performance measurement capabilities in the UEFI boot environment, exposing the core
/// performance functionality exposed by the performance measurement service. This crate will package those function
/// into a UEFI protocol and provide the necessary event callbacks to publish the performance data to the rest of the
/// UEFI environment
///
/// ## Example Usage
///
/// ```rust
/// use patina_performance::component::*;
///
/// let component = Performance::new();
/// ```
#[derive(Default)]
pub struct Performance;

#[component]
impl Performance {
    /// Creates a new instance of the Performance component.
    pub const fn new() -> Self {
        Self
    }

    /// Entry point of [`Performance`]
    #[cfg_attr(coverage, coverage(off))] // This is tested via the generic version, see _entry_point.
    fn entry_point(
        self,
        boot_services: StandardBootServices,
        runtime_services: StandardRuntimeServices,
        timer: Service<dyn ArchTimerFunctionality>,
        performance: Service<dyn PerformanceManager>,
        mm_comm_service: Option<Service<dyn MmCommunication>>,
    ) -> Result<(), EfiError> {
        // Register the service so the EDK II Performance Measurement protocol function can reach it.
        set_performance_service(performance.clone()).unwrap_or_else(|e| {
            log::error!(
                "[{}]: Performance service was already registered. It should only be registered here! ({e})",
                function!()
            );
        });

        Self::_entry_point(boot_services, runtime_services, mm_comm_service, performance, timer)
    }

    /// Entry point that have generic parameter.
    fn _entry_point<B, R>(
        boot_services: B,
        runtime_services: R,
        mm_comm_service: Option<Service<dyn MmCommunication>>,
        performance: Service<dyn PerformanceManager>,
        timer: Service<dyn ArchTimerFunctionality>,
    ) -> Result<(), EfiError>
    where
        B: BootServices + Clone + 'static,
        R: RuntimeServices + Clone + 'static,
    {
        // Register EndOfDxe event to allocate the boot performance table and report the table address through status code.
        boot_services.create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(report_fbpt_event::<B, R>),
            Box::new((boot_services.clone(), runtime_services.clone(), performance.clone())),
            &END_OF_DXE_EVENT_GROUP_GUID,
        )?;

        // Install the protocol interfaces for DXE performance.
        boot_services.install_protocol_interface(
            None,
            Box::new(EdkiiPerformanceMeasurementProtocol {
                create_performance_measurement: create_performance_measurement_efiapi,
            }),
        )?;

        // Register ReadyToBoot event to update the boot performance table for MM performance data.
        // Only register if mm_comm_region is available
        if let Some(mm_comm_service) = mm_comm_service {
            // TODO: Replace direct usage of the boot services event services with a Patina service
            //       when available.
            boot_services.create_event_ex(
                EventType::NOTIFY_SIGNAL,
                Tpl::CALLBACK,
                Some(fetch_and_add_mm_performance_records::<B>),
                Box::new((boot_services.clone(), performance.clone(), mm_comm_service)),
                &EVENT_GROUP_READY_TO_BOOT,
            )?;
        } else {
            log::warn!(
                "Performance: MM communication service unavailable, skipping MM performance event registration."
            );
        }

        log::info!("Performance: Performance component initialized.");

        // Install configuration table for performance property.
        // SAFETY: `install_configuration_table` requires that the data match the GUID; PERFORMANCE_PROTOCOL_GUID matches `PerformanceProperty`.
        unsafe {
            boot_services.install_configuration_table(
                &PERFORMANCE_PROTOCOL_GUID,
                Box::new(PerformanceProperty::new(
                    timer.perf_frequency(),
                    timer.cpu_count_start(),
                    timer.cpu_count_end(),
                )),
            )?;
        };

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

    let clamped_size = core::cmp::min(total_size, mm::MAX_SMM_BOOT_RECORD_BYTES);
    if clamped_size == 0 {
        log::info!("Performance: MM reported 0 performance bytes.");
        return Ok(Vec::new());
    }

    let mut result = Vec::with_capacity(clamped_size);

    while result.len() < clamped_size {
        let remaining = clamped_size - result.len();
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

/// Adds MM performance records to the FBPT.
pub extern "efiapi" fn fetch_and_add_mm_performance_records<B>(
    event: patina::standard::efi::Event,
    ctx: MmPerformanceEventContext<B>,
) where
    B: BootServices + Clone + 'static,
{
    let (boot_services, performance, comm_service) = *ctx;
    let _ = boot_services.close_event(event);

    if let Err(e) = process_mm_performance_records(&comm_service, &performance) {
        log::error!("Performance: {e}");
    }
}

/// Reports the FBPT at End of DXE: queries the required size from the [`PerformanceMeasurement`] service, allocates the
/// publishing buffer, has the service serialize the table into it, reports it through a status code, and installs it as
/// a configuration table.
pub extern "efiapi" fn report_fbpt_event<B, R>(event: patina::standard::efi::Event, ctx: ReportFbptEventContext<B, R>)
where
    B: BootServices + Clone + 'static,
    R: RuntimeServices + Clone + 'static,
{
    let (boot_services, runtime_services, performance) = *ctx;
    let _ = boot_services.close_event(event);

    // Query the size required to publish the table, then allocate the memory ourselves.
    let size = match performance.published_table_size() {
        Ok(size) => size,
        Err(e) => {
            log::error!("Performance: Fail to get FBPT size: {e:?}");
            return;
        }
    };

    let Some(buffer) = allocate_fbpt_buffer(&boot_services, find_previous_table_address(&runtime_services), size)
    else {
        log::error!("Performance: Fail to allocate FBPT buffer.");
        return;
    };
    let fbpt_address = buffer.as_ptr() as usize;

    // Provide the allocated memory for the service to serialize the table into.
    if let Err(e) = performance.publish_table(buffer) {
        log::error!("Performance: Fail to serialize FBPT: {e:?}");
        free_fbpt_buffer(&boot_services, fbpt_address, size);
        return;
    }

    // SAFETY: `p` is the only mutable reference to the `StatusCodeRuntimeProtocol` in this scope.
    let Ok(p) = (unsafe { boot_services.locate_protocol::<status_code::StatusCodeProtocol>(None) }) else {
        log::error!("Performance: Fail to find status code protocol.");
        return;
    };

    let status = p.report_status_code_with_data(
        EFI_PROGRESS_CODE,
        EFI_SOFTWARE_DXE_BS_DRIVER,
        0,
        patina::guid::CALLER_ID.as_efi_guid(),
        *EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE_GUID.as_efi_guid(),
        fbpt_address,
    );
    if status.is_err() {
        log::error!("Performance: Fail to report FBPT status code.");
    }

    // SAFETY: This operation is valid because the expected configuration type of an entry with guid
    // `EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE_GUID` is a usize and the memory address is valid and points to an FBPT.
    let status = unsafe {
        boot_services.install_configuration_table_unchecked(
            &EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE_GUID,
            fbpt_address as *mut c_void,
        )
    };
    if status.is_err() {
        log::error!("Performance: Fail to install configuration table for FBPT firmware performance.");
    }
}

/// Allocates a reserved-memory buffer large enough to publish the FBPT.
///
/// The allocation prefers `previous_address` (the location used on the previous boot) so the table can be placed
/// consistently, falling back to any address below 4 GiB.
fn allocate_fbpt_buffer<B: BootServices>(
    boot_services: &B,
    previous_address: Option<usize>,
    size: usize,
) -> Option<&'static mut [u8]> {
    let pages = size.div_ceil(UEFI_PAGE_SIZE);
    let alloc_size = pages * UEFI_PAGE_SIZE;

    let address = previous_address
        .and_then(|address| {
            boot_services.allocate_pages(AllocType::Address(address), EfiMemoryType::ReservedMemoryType, pages).ok()
        })
        .or_else(|| {
            // `AllocType::MaxAddress` requests any physical address below the given bound (u32::MAX = 4 GiB); the
            // firmware chooses the actual address.
            boot_services
                .allocate_pages(AllocType::MaxAddress(u32::MAX as usize), EfiMemoryType::ReservedMemoryType, pages)
                .ok()
        })?;

    // SAFETY: `pages` pages (`alloc_size` bytes) were just allocated at `address` as reserved memory.
    Some(unsafe { core::slice::from_raw_parts_mut(address as *mut u8, alloc_size) })
}

/// Frees the FBPT buffer allocated by `allocate_fbpt_buffer`.
fn free_fbpt_buffer<B: BootServices>(boot_services: &B, buffer: usize, size: usize) {
    let pages = size.div_ceil(UEFI_PAGE_SIZE);
    let address = buffer;

    // SAFETY: `buffer` was allocated by `allocate_fbpt_buffer`, which used `boot_services.allocate_pages` to allocate
    //         this buffer, so it is safe to free using `boot_services.free_pages`.
    if let Err(e) = unsafe { boot_services.free_pages(address, pages) } {
        log::error!("Performance: Failed to free FBPT buffer at {address:#x}: {e:?}");
    }
}

#[cfg(test)]
mod tests {
    use crate::component::protocol::EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID;

    use super::*;
    use core::{
        assert_eq,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use patina::standard::efi;

    use patina::{
        c_ptr::{CMutPtr, CPtr},
        component::service::{IntoService, Service},
        performance::{
            error::Error,
            measurement::{CallerIdentifier, PerfAttribute},
        },
        protocol::ProtocolInterface,
        uefi::{boot_services::MockBootServices, runtime_services::MockRuntimeServices},
    };
    use patina_mm::component::communicator::{MmCommunication, Status};
    use std::sync::Arc;

    // Some constants shared between tests
    const TEST_EVENT_HANDLE: efi::Event = 1_usize as efi::Event;
    const TEST_EVENT_HANDLE_2: efi::Event = 2_usize as efi::Event;
    const TEST_EFI_HANDLE: efi::Handle = 1 as efi::Handle;
    const TEST_PERFORMANCE_RECORD_TYPE: u16 = 0x1010;
    const TEST_PERFORMANCE_RECORD_LENGTH: u8 = 34;
    const TEST_PERFORMANCE_RECORD_REVISION: u8 = 1;
    const TEST_RECORD_ID_BASE: u16 = 1;
    const TEST_TIMESTAMP_BASE: u64 = 100;
    const TEST_MULTI_CHUNK_RECORD_COUNT: usize = 40;
    const TEST_MM_COMM_FUNCTION_ID_SIZE: u64 = 1;
    const TEST_MM_COMM_FUNCTION_ID_DATA: u64 = 3;
    const TEST_MM_COMM_RESPONSE_SIZE: usize = 40;

    // Chunk size for MM communication
    const TEST_SMM_FETCH_CHUNK_BYTES: usize = mm::SMM_FETCH_CHUNK_BYTES;

    // Calculated sizes for MM communication buffers
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
            let mut response = vec![0u8; TEST_MM_COMM_RESPONSE_SIZE];
            response[0..8].copy_from_slice(&TEST_MM_COMM_FUNCTION_ID_SIZE.to_le_bytes());
            response[16..24].copy_from_slice(&$boot_record_size.to_le_bytes());
            response
        }};
    }

    /// Creates a test MM communication data response
    macro_rules! create_data_response {
        ($data:expr) => {{
            let mut response = vec![0u8; TEST_MM_COMM_DATA_RESPONSE_SIZE];
            response[0..8].copy_from_slice(&TEST_MM_COMM_FUNCTION_ID_DATA.to_le_bytes());
            response[16..24].copy_from_slice(&($data.len() as u64).to_le_bytes());
            response[TEST_MM_COMM_RESPONSE_SIZE..TEST_MM_COMM_RESPONSE_SIZE + $data.len()].copy_from_slice(&$data);
            response
        }};
    }

    #[derive(IntoService)]
    #[service(dyn ArchTimerFunctionality)]
    struct MockTimer {}

    impl ArchTimerFunctionality for MockTimer {
        fn perf_frequency(&self) -> u64 {
            100
        }
        fn cpu_count(&self) -> u64 {
            200
        }
    }

    /// Test implementation of the performance service. Counts the records ingested through `add_generic_record` via a
    /// shared atomic so tests can assert on them, and reports a fixed table address.
    struct MockPerf {
        records: Arc<AtomicUsize>,
    }

    impl MockPerf {
        fn new(records: Arc<AtomicUsize>) -> Self {
            Self { records }
        }
    }

    impl PerformanceManager for MockPerf {
        fn create_measurement(
            &self,
            _caller_identifier: CallerIdentifier,
            _guid: Option<&efi::Guid>,
            _string: Option<&str>,
            _ticker: u64,
            _address: usize,
            _perf_id: u16,
            _attribute: PerfAttribute,
        ) -> Result<(), Error> {
            Ok(())
        }
        fn add_generic_record(&self, _record: &GenericPerformanceRecord) -> Result<(), Error> {
            self.records.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn published_table_size(&self) -> Result<usize, Error> {
            Ok(64)
        }
        fn publish_table(&self, _buffer: &'static mut [u8]) -> Result<(), Error> {
            Ok(())
        }
    }

    #[test]
    fn test_entry_point() {
        let mut boot_services = MockBootServices::new();
        boot_services.expect_raise_tpl().return_const(Tpl::APPLICATION);
        boot_services.expect_restore_tpl().return_const(());

        // Test that the protocol in installed.
        boot_services
            .expect_install_protocol_interface::<EdkiiPerformanceMeasurementProtocol, Box<_>>()
            .once()
            .withf_st(|handle, _protocol_interface| {
                assert_eq!(&None, handle);
                assert_eq!(
                    EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID.into_inner(),
                    EdkiiPerformanceMeasurementProtocol::PROTOCOL_GUID
                );
                true
            })
            .returning(|_, protocol_interface| Ok((TEST_EFI_HANDLE, protocol_interface.metadata())));

        // Test that an event to report the fbpt at the end of dxe is created.
        boot_services
            .expect_create_event_ex::<Box<(MockBootServices, MockRuntimeServices, Service<dyn PerformanceManager>)>>()
            .once()
            .withf_st(|event_type, notify_tpl, notify_function, _notify_context, event_group| {
                assert_eq!(&EventType::NOTIFY_SIGNAL, event_type);
                assert_eq!(&Tpl::CALLBACK, notify_tpl);
                assert_eq!(
                    report_fbpt_event::<MockBootServices, MockRuntimeServices> as *const () as usize,
                    notify_function.unwrap() as usize
                );
                assert_eq!(&END_OF_DXE_EVENT_GROUP_GUID, event_group);
                true
            })
            .return_const_st(Ok(TEST_EVENT_HANDLE));

        boot_services.expect_install_configuration_table::<Box<PerformanceProperty>>().once().return_const(Ok(()));

        let runtime_services = MockRuntimeServices::new();

        let perf: Service<dyn PerformanceManager> =
            Service::mock(Box::new(MockPerf::new(Arc::new(AtomicUsize::new(0)))));

        let _ = Performance::_entry_point(
            boot_services,
            runtime_services,
            None,
            perf,
            Service::mock(Box::new(MockTimer {})),
        );
    }

    #[test]
    fn test_entry_point_with_mm_service_registers_ready_to_boot_event() {
        struct FakeComm;
        impl MmCommunication for FakeComm {
            fn communicate<'a>(
                &self,
                _id: u8,
                _data_buffer: &[u8],
                _recipient: patina::Guid<'a>,
            ) -> Result<Vec<u8>, Status> {
                Ok(Vec::new())
            }
        }

        // Mock for _entry_point - handles event creation and protocol installation
        let mut entry_point_mock = MockBootServices::new();
        entry_point_mock
            .expect_create_event_ex::<Box<(MockBootServices, MockRuntimeServices, Service<dyn PerformanceManager>)>>()
            .once()
            .return_const_st(Ok(TEST_EVENT_HANDLE));
        entry_point_mock
            .expect_create_event_ex::<MmPerformanceEventContext<MockBootServices>>()
            .once()
            .withf_st(|_, _, f, _, group| {
                (f.unwrap() as usize) == fetch_and_add_mm_performance_records::<MockBootServices> as *const () as usize
                    && group == &EVENT_GROUP_READY_TO_BOOT
            })
            .return_const_st(Ok(TEST_EVENT_HANDLE_2));
        entry_point_mock
            .expect_install_protocol_interface::<EdkiiPerformanceMeasurementProtocol, Box<_>>()
            .once()
            .returning(|_, protocol_interface| Ok((TEST_EFI_HANDLE, protocol_interface.metadata())));
        entry_point_mock.expect_install_configuration_table::<Box<PerformanceProperty>>().once().return_const(Ok(()));

        let runtime_services = MockRuntimeServices::new();

        let perf: Service<dyn PerformanceManager> =
            Service::mock(Box::new(MockPerf::new(Arc::new(AtomicUsize::new(0)))));
        let mm_service: Service<dyn MmCommunication> = Service::mock(Box::new(FakeComm));
        let timer: Service<dyn ArchTimerFunctionality> = Service::mock(Box::new(MockTimer {}));
        let _ = Performance::_entry_point(entry_point_mock, runtime_services, Some(mm_service), perf, timer);
    }

    #[test]
    fn test_report_fbpt_event_publishes_table() {
        static REPORT_STATUS_CODE_CALLED: AtomicBool = AtomicBool::new(false);

        extern "efiapi" fn report_status_code(
            _a: u32,
            _b: u32,
            _c: u32,
            _d: *const efi::Guid,
            _e: *const patina::pi::protocol::status_code::EfiStatusCodeData,
        ) -> efi::Status {
            REPORT_STATUS_CODE_CALLED.store(true, Ordering::Relaxed);
            efi::Status::SUCCESS
        }
        let mut status_code_runtime_protocol =
            Box::new(patina::pi::protocol::status_code::StatusCodeProtocol { report_status_code });
        let status_code_runtime_protocol_ptr = status_code_runtime_protocol.as_mut_ptr();

        let mut boot_services = MockBootServices::new();
        boot_services.expect_close_event().once().return_const(Ok(()));

        // The component allocates the publishing buffer itself; hand back a real leaked page-sized buffer.
        let leaked_buffer = Box::leak(std::vec![0u8; UEFI_PAGE_SIZE].into_boxed_slice());
        let leaked_buffer_addr = leaked_buffer.as_mut_ptr() as usize;
        boot_services.expect_allocate_pages().once().returning(move |_, _, _| Ok(leaked_buffer_addr));

        boot_services.expect_install_configuration_table_unchecked().once().return_const(Ok(()));
        boot_services
            .expect_locate_protocol()
            .once()
            // SAFETY: Test code - creating a mutable reference to test protocol pointer for mocking.
            .returning_st(move |_| Ok(unsafe { &mut *status_code_runtime_protocol_ptr }));

        let mut runtime_services = MockRuntimeServices::new();
        runtime_services
            .expect_get_variable::<crate::component::table::FirmwarePerformanceVariable>()
            .once()
            .returning(|_, _, _| Err(efi::Status::NOT_FOUND));

        let perf: Service<dyn PerformanceManager> =
            Service::mock(Box::new(MockPerf::new(Arc::new(AtomicUsize::new(0)))));

        report_fbpt_event::<MockBootServices, MockRuntimeServices>(
            TEST_EVENT_HANDLE,
            Box::new((boot_services, runtime_services, perf)),
        );

        assert!(REPORT_STATUS_CODE_CALLED.load(Ordering::Relaxed));
    }

    #[test]
    fn test_ready_to_boot_callback_runs_with_service_zero_records() {
        struct ZeroSizeComm;
        impl MmCommunication for ZeroSizeComm {
            fn communicate<'a>(
                &self,
                _id: u8,
                data_buffer: &[u8],
                _recipient: patina::Guid<'a>,
            ) -> Result<Vec<u8>, Status> {
                if data_buffer.len() < core::mem::size_of::<u64>() {
                    return Err(Status::InvalidDataBuffer);
                }
                let mut fid = [0u8; core::mem::size_of::<u64>()];
                fid.copy_from_slice(&data_buffer[0..core::mem::size_of::<u64>()]);
                if u64::from_le_bytes(fid) == TEST_MM_COMM_FUNCTION_ID_SIZE {
                    // Return a size response with function id and zero boot_record_size
                    return Ok(create_size_response!(0u64));
                }
                Err(Status::InvalidDataBuffer)
            }
        }

        // Mock for callback - handles close_event
        let mut callback_mock = MockBootServices::new();
        callback_mock.expect_close_event().once().return_const(Ok(()));

        let records = Arc::new(AtomicUsize::new(0));
        let perf: Service<dyn PerformanceManager> = Service::mock(Box::new(MockPerf::new(records.clone())));
        let mm_service: Service<dyn MmCommunication> = Service::mock(Box::new(ZeroSizeComm));
        fetch_and_add_mm_performance_records::<MockBootServices>(
            TEST_EVENT_HANDLE,
            Box::new((callback_mock, perf, mm_service)),
        );

        assert_eq!(records.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_ready_to_boot_callback_runs_with_service_one_record() {
        use core::cell::Cell;
        struct OneRecordComm {
            step: Cell<u8>,
        }
        impl OneRecordComm {
            fn new() -> Self {
                Self { step: Cell::new(0) }
            }
        }
        impl MmCommunication for OneRecordComm {
            fn communicate<'a>(
                &self,
                _id: u8,
                data_buffer: &[u8],
                _recipient: patina::Guid<'a>,
            ) -> Result<Vec<u8>, Status> {
                if data_buffer.len() < core::mem::size_of::<u64>() {
                    return Err(Status::InvalidDataBuffer);
                }
                let mut func_id_buffer = [0u8; core::mem::size_of::<u64>()];
                func_id_buffer.copy_from_slice(&data_buffer[0..core::mem::size_of::<u64>()]);
                match (u64::from_le_bytes(func_id_buffer), self.step.get()) {
                    (fid, 0) if fid == TEST_MM_COMM_FUNCTION_ID_SIZE => {
                        // size query
                        self.step.set(1);
                        Ok(create_size_response!(u64::from(TEST_PERFORMANCE_RECORD_LENGTH)))
                    }
                    (fid, 1) if fid == TEST_MM_COMM_FUNCTION_ID_DATA => {
                        // data query
                        self.step.set(2);
                        let record = create_test_record!(TEST_RECORD_ID_BASE, TEST_TIMESTAMP_BASE + 23);
                        Ok(create_data_response!(record))
                    }
                    _ => Err(Status::InvalidDataBuffer),
                }
            }
        }

        // Mock for callback - handles close_event
        let mut callback_mock = MockBootServices::new();
        callback_mock.expect_close_event().once().return_const(Ok(()));

        let records = Arc::new(AtomicUsize::new(0));
        let perf: Service<dyn PerformanceManager> = Service::mock(Box::new(MockPerf::new(records.clone())));
        let mm_service: Service<dyn MmCommunication> = Service::mock(Box::new(OneRecordComm::new()));
        fetch_and_add_mm_performance_records::<MockBootServices>(
            TEST_EVENT_HANDLE,
            Box::new((callback_mock, perf, mm_service)),
        );

        assert_eq!(records.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_ready_to_boot_callback_runs_with_service_multi_chunk() {
        use core::cell::Cell;

        const TOTAL_RECORD_BYTES: usize = TEST_PERFORMANCE_RECORD_LENGTH as usize * TEST_MULTI_CHUNK_RECORD_COUNT;

        let mut all_records = Vec::with_capacity(TOTAL_RECORD_BYTES);
        for i in 0..TEST_MULTI_CHUNK_RECORD_COUNT {
            let record = create_test_record!(TEST_RECORD_ID_BASE + i as u16, TEST_TIMESTAMP_BASE + i as u64);
            all_records.extend_from_slice(&record);
        }

        // We'll store exact bytes and let mock slice them
        struct MultiChunks {
            buf: Vec<u8>, // concatenated records
            fetches: Cell<u8>,
        }
        impl MmCommunication for MultiChunks {
            fn communicate<'a>(&self, _id: u8, data: &[u8], _: patina::Guid<'a>) -> Result<Vec<u8>, Status> {
                if data.len() < core::mem::size_of::<u64>() {
                    return Err(Status::InvalidDataBuffer);
                }
                let mut f = [0u8; core::mem::size_of::<u64>()];
                f.copy_from_slice(&data[0..core::mem::size_of::<u64>()]);
                match u64::from_le_bytes(f) {
                    fid if fid == TEST_MM_COMM_FUNCTION_ID_SIZE => {
                        // size request
                        Ok(create_size_response!(self.buf.len() as u64))
                    }
                    fid if fid == TEST_MM_COMM_FUNCTION_ID_DATA => {
                        // data request
                        if data.len() < TEST_MM_COMM_RESPONSE_SIZE {
                            return Err(Status::InvalidDataBuffer);
                        }
                        let mut ask_buffer = [0u8; core::mem::size_of::<u64>()];
                        ask_buffer.copy_from_slice(&data[16..24]);
                        let ask = u64::from_le_bytes(ask_buffer) as usize;
                        let mut offset_buffer = [0u8; core::mem::size_of::<u64>()];
                        offset_buffer.copy_from_slice(&data[32..40]);
                        let offset = u64::from_le_bytes(offset_buffer) as usize;
                        if offset > self.buf.len() {
                            return Err(Status::InvalidDataBuffer);
                        }
                        let remaining: usize = self.buf.len() - offset;
                        let take = core::cmp::min(ask, remaining);
                        let mut r = vec![0u8; TEST_MM_COMM_RESPONSE_SIZE + ask];
                        r[0..8].copy_from_slice(&TEST_MM_COMM_FUNCTION_ID_DATA.to_le_bytes());
                        r[16..24].copy_from_slice(&(take as u64).to_le_bytes()); // actual valid bytes
                        r[TEST_MM_COMM_RESPONSE_SIZE..TEST_MM_COMM_RESPONSE_SIZE + take]
                            .copy_from_slice(&self.buf[offset..offset + take]);
                        self.fetches.set(self.fetches.get() + 1);
                        Ok(r)
                    }
                    _ => Err(Status::InvalidDataBuffer),
                }
            }
        }

        // Mock for callback - handles close_event
        let mut callback_mock = MockBootServices::new();
        callback_mock.expect_close_event().once().return_const(Ok(()));

        let records = Arc::new(AtomicUsize::new(0));
        let perf: Service<dyn PerformanceManager> = Service::mock(Box::new(MockPerf::new(records.clone())));
        let mm_service: Service<dyn MmCommunication> =
            Service::mock(Box::new(MultiChunks { buf: all_records, fetches: Cell::new(0) }));
        fetch_and_add_mm_performance_records::<MockBootServices>(
            TEST_EVENT_HANDLE,
            Box::new((callback_mock, perf, mm_service)),
        );

        assert_eq!(records.load(Ordering::Relaxed), TEST_MULTI_CHUNK_RECORD_COUNT);
    }

    /// Verifies that malformed record data doesn't cause infinite loops.
    #[test]
    fn test_performance_record_iterator_infinite_loop_does_not_occur_truncation() {
        use zerocopy::IntoBytes;

        // Truncated record - header claims more bytes of data than are actually available
        // Claims 100 bytes, but only 6 bytes are present (4-byte header + 2 extra bytes)
        let truncated_header =
            PerformanceRecordHeader::new(TEST_PERFORMANCE_RECORD_TYPE, 100, TEST_PERFORMANCE_RECORD_REVISION);

        let mut truncated_data = vec![0u8; 6];
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
    fn test_performance_record_iterator_infinite_loop_does_not_occur_invalid_len() {
        use zerocopy::IntoBytes;

        // Invalid: length=1 < header size=4
        let invalid_length_header =
            PerformanceRecordHeader::new(TEST_PERFORMANCE_RECORD_TYPE, 1, TEST_PERFORMANCE_RECORD_REVISION);
        let mut invalid_length_data = vec![0u8; 20];
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
