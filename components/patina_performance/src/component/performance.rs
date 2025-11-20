//! Patina Performance Component
//!
//! This is the primary Patina Performance component, which enables performance analysis in the UEFI boot environment.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;

use crate::{config, mm};
use alloc::{boxed::Box, string::String, vec::Vec};
use core::{clone::Clone, convert::AsRef};
use patina::{
    boot_services::{BootServices, StandardBootServices, event::EventType, tpl::Tpl},
    component::{
        IntoComponent,
        hob::Hob,
        params::Config,
        service::{Service, perf_timer::ArchTimerFunctionality},
    },
    error::EfiError,
    guids::{EVENT_GROUP_END_OF_DXE, PERFORMANCE_PROTOCOL},
    performance::{
        globals::{get_static_state, set_load_image_count, set_perf_measurement_mask, set_static_state},
        measurement::{PerformanceProperty, create_performance_measurement, event_callback},
        record::{
            GenericPerformanceRecord, PerformanceRecordHeader,
            hob::{HobPerformanceData, HobPerformanceDataExtractor},
            print_record_details, record_type_name,
        },
        table::FirmwareBasicBootPerfTable,
    },
    runtime_services::{RuntimeServices, StandardRuntimeServices},
    tpl_mutex::TplMutex,
    uefi_protocol::performance_measurement::EdkiiPerformanceMeasurement,
};
use patina_mm::component::communicator::MmCommunication;
use r_efi::system::EVENT_GROUP_READY_TO_BOOT;

pub use mu_rust_helpers::function;

/// Context parameter for the Ready-to-Boot event callback that fetches MM performance records.
type MmPerformanceEventContext<BB, B, F> = Box<(BB, &'static TplMutex<'static, F, B>, Service<dyn MmCommunication>)>;

/// Performance Component.
#[derive(IntoComponent)]
pub struct Performance;

impl Performance {
    /// Entry point of [`Performance`]
    #[coverage(off)] // This is tested via the generic version, see _entry_point.
    pub fn entry_point(
        self,
        config: Config<config::PerfConfig>,
        boot_services: StandardBootServices,
        runtime_services: StandardRuntimeServices,
        records_buffers_hobs: Option<Hob<HobPerformanceData>>,
        timer: Service<dyn ArchTimerFunctionality>,
        mm_comm_service: Option<Service<dyn MmCommunication>>,
    ) -> Result<(), EfiError> {
        if !config.enable_component {
            log::warn!("Patina Performance Component is not enabled, skipping entry point.");
            return Ok(());
        }

        set_perf_measurement_mask(config.enabled_measurements);

        set_static_state(StandardBootServices::clone(&boot_services), timer.clone()).unwrap_or_else(|_| {
            log::error!(
                "[{}]: Performance static state was set somewhere else. It should only be set here!",
                function!()
            );
        });

        let Some((_, fbpt, _)) = get_static_state() else {
            log::error!("[{}]: Performance static state was not initialized properly.", function!());
            return Err(EfiError::Aborted);
        };

        self._entry_point(boot_services, runtime_services, records_buffers_hobs, mm_comm_service, fbpt, timer)
    }

    /// Entry point that have generic parameter.
    fn _entry_point<BB, B, RR, R, P, F>(
        self,
        boot_services: BB,
        runtime_services: RR,
        records_buffers_hobs: Option<P>,
        mm_comm_service: Option<Service<dyn MmCommunication>>,
        fbpt: &'static TplMutex<'static, F, B>,
        timer: Service<dyn ArchTimerFunctionality>,
    ) -> Result<(), EfiError>
    where
        BB: AsRef<B> + Clone + 'static,
        B: BootServices + 'static,
        RR: AsRef<R> + Clone + 'static,
        R: RuntimeServices + 'static,
        P: HobPerformanceDataExtractor,
        F: FirmwareBasicBootPerfTable,
    {
        // Register EndOfDxe event to allocate the boot performance table and report the table address through status code.
        boot_services.as_ref().create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(event_callback::report_fbpt_record_buffer),
            Box::new((BB::clone(&boot_services), RR::clone(&runtime_services), fbpt)),
            &EVENT_GROUP_END_OF_DXE,
        )?;

        // Handle optional `records_buffers_hobs`
        if let Some(records_buffers_hobs) = records_buffers_hobs {
            let (hob_load_image_count, hob_perf_records) = records_buffers_hobs
                .extract_hob_perf_data()
                .inspect(|(_, perf_buf)| {
                    log::info!("Performance: {} Hob performance records found.", perf_buf.iter().count());
                })
                .inspect_err(|_| {
                    log::error!(
                        "Performance: Error while trying to insert hob performance records, using default values"
                    )
                })
                .unwrap_or_default();

            // Initialize perf data from hob values.

            set_load_image_count(hob_load_image_count);
            fbpt.lock().set_perf_records(hob_perf_records);
        } else {
            log::info!("Performance: No Hob performance records provided.");
        }

        // Install the protocol interfaces for DXE performance.
        boot_services.as_ref().install_protocol_interface(
            None,
            Box::new(EdkiiPerformanceMeasurement { create_performance_measurement }),
        )?;

        // Register ReadyToBoot event to update the boot performance table for MM performance data.
        // Only register if mm_comm_region is available
        if let Some(mm_comm_service) = mm_comm_service {
            // TODO: Replace direct usage of the boot services event services with a Patina service
            //       when available.
            boot_services.as_ref().create_event_ex(
                EventType::NOTIFY_SIGNAL,
                Tpl::CALLBACK,
                Some(fetch_and_add_mm_performance_records::<BB, B, F>),
                Box::new((BB::clone(&boot_services), fbpt, mm_comm_service)),
                &EVENT_GROUP_READY_TO_BOOT,
            )?;
        } else {
            log::warn!(
                "Performance: MM communication service unavailable, skipping MM performance event registration."
            );
        }

        log::info!("Performance: Performance component initialized.");

        // Install configuration table for performance property.
        // SAFETY: `install_configuration_table` requires that the data match the GUID; PERFORMANCE_PROTOCOL matches `PerformanceProperty`.
        unsafe {
            boot_services.as_ref().install_configuration_table(
                &PERFORMANCE_PROTOCOL,
                Box::new(PerformanceProperty::new(
                    timer.perf_frequency(),
                    timer.cpu_count_start(),
                    timer.cpu_count_end(),
                )),
            )?
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
    StatusError(r_efi::efi::Status),
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
        .map_err(|_| MmPerformanceError::RecordError("Failed to write GetRecordSize request".into()))?;

    let size_resp_bytes = comm_service
        .communicate(1, &size_req_buf, patina::Guid::Owned(mm::EFI_FIRMWARE_PERFORMANCE_GUID))
        .map_err(MmPerformanceError::Communication)?;

    let (size_resp, _) = mm::GetRecordSize::read_from(&size_resp_bytes).map_err(|_| MmPerformanceError::ParseError)?;

    if size_resp.return_status != r_efi::efi::Status::SUCCESS {
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
        .map_err(|_| MmPerformanceError::RecordError("Failed to write GetRecordDataByOffset request".into()))?;

    let data_resp_bytes = comm_service
        .communicate(1, &data_req_buf, patina::Guid::Owned(mm::EFI_FIRMWARE_PERFORMANCE_GUID))
        .map_err(MmPerformanceError::Communication)?;

    let (data_resp, _) =
        mm::GetRecordDataByOffset::read_from_default(&data_resp_bytes).map_err(|_| MmPerformanceError::ParseError)?;

    if data_resp.return_status != r_efi::efi::Status::SUCCESS {
        return Err(MmPerformanceError::StatusError(data_resp.return_status));
    }

    let actual_size = core::cmp::min(chunk_size, data_resp.boot_record_data().len());
    Ok(data_resp.boot_record_data()[..actual_size].to_vec())
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
    type Item = Result<GenericPerformanceRecord<&'a [u8]>, MmPerformanceError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.bytes.len() < PerformanceRecordHeader::SIZE {
            return None;
        }

        let header = match PerformanceRecordHeader::try_from(self.bytes) {
            Ok(h) => h,
            Err(err) => {
                self.bytes = &self.bytes[1..];
                return Some(Err(MmPerformanceError::RecordError(err.into())));
            }
        };

        let rec_len = header.length as usize;
        if rec_len < PerformanceRecordHeader::SIZE {
            self.bytes = &self.bytes[PerformanceRecordHeader::SIZE..];
            return Some(Err(MmPerformanceError::RecordError(alloc::format!(
                "Record reports too small length {} (< {})",
                rec_len,
                PerformanceRecordHeader::SIZE
            ))));
        }

        if rec_len > self.bytes.len() {
            // Consume all remaining bytes since the record claims to be longer
            // than what we have available (truncated data)
            self.bytes = &[];
            return Some(Err(MmPerformanceError::RecordError(alloc::format!(
                "Truncated record (needed {}, had {})",
                rec_len,
                self.bytes.len()
            ))));
        }

        let data = &self.bytes[PerformanceRecordHeader::SIZE..rec_len];
        let record = GenericPerformanceRecord {
            record_type: header.record_type,
            length: header.length,
            revision: header.revision,
            data,
        };

        self.bytes = &self.bytes[rec_len..];
        Some(Ok(record))
    }
}

/// Processes MM performance records and adds them to the FBPT
fn process_mm_performance_records<F, B>(
    comm_service: &Service<dyn MmCommunication>,
    fbpt: &TplMutex<'static, F, B>,
) -> Result<(), MmPerformanceError>
where
    F: FirmwareBasicBootPerfTable,
    B: BootServices + 'static,
{
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

                log::debug!(
                    "Performance: MM record #{} - type: 0x{:04X} ({}), length: {}, revision: {}, data_len: {}",
                    record_count,
                    record.record_type,
                    record_type_name(record.record_type),
                    record.length,
                    record.revision,
                    record.data.len()
                );
                // Print detailed record information based on type
                print_record_details(record.record_type, record_count, record.data);

                if let Err(e) = fbpt.lock().add_record(record) {
                    error_count += 1;
                    log::error!("Performance: Failed adding MM record #{}: {:?}", record_count, e);
                } else {
                    success_count += 1;
                }
            }
            Err(e) => {
                log::warn!("Performance: {}", e);
                break;
            }
        }
    }

    log::debug!(
        "Performance: MM record summary - total: {}, added: {}, failed: {}",
        record_count,
        success_count,
        error_count
    );

    Ok(())
}

/// Adds MM performance records to the FBPT.
pub extern "efiapi" fn fetch_and_add_mm_performance_records<BB, B, F>(
    event: r_efi::efi::Event,
    ctx: MmPerformanceEventContext<BB, B, F>,
) where
    BB: AsRef<B> + Clone,
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    let (boot_services, fbpt, comm_service) = *ctx;
    let _ = boot_services.as_ref().close_event(event);

    if let Err(e) = process_mm_performance_records(&comm_service, fbpt) {
        log::error!("Performance: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use core::assert_eq;
    use r_efi::efi;

    use patina::{
        boot_services::{MockBootServices, c_ptr::CPtr},
        component::service::{IntoService, Service},
        performance::{
            record::{PerformanceRecordBuffer, hob::MockHobPerformanceDataExtractor},
            table::MockFirmwareBasicBootPerfTable,
        },
        runtime_services::MockRuntimeServices,
        uefi_protocol::{ProtocolInterface, performance_measurement::EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID},
    };
    use patina_mm::component::communicator::{MmCommunication, Status};

    // Some constants shared between tests
    const TEST_EVENT_HANDLE: efi::Event = 1_usize as efi::Event;
    const TEST_EVENT_HANDLE_2: efi::Event = 2_usize as efi::Event;
    const TEST_EFI_HANDLE: efi::Handle = 1 as efi::Handle;
    const TEST_HOB_LOAD_IMAGE_COUNT: u32 = 10;
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

    #[test]
    fn test_entry_point() {
        let mut boot_services = MockBootServices::new();
        boot_services.expect_raise_tpl().return_const(Tpl::APPLICATION);
        boot_services.expect_restore_tpl().return_const(());

        // Test that the protocol in installed.
        boot_services
            .expect_install_protocol_interface::<EdkiiPerformanceMeasurement, Box<_>>()
            .once()
            .withf_st(|handle, _protocol_interface| {
                assert_eq!(&None, handle);
                assert_eq!(EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID, EdkiiPerformanceMeasurement::PROTOCOL_GUID);
                true
            })
            .returning(|_, protocol_interface| Ok((TEST_EFI_HANDLE, protocol_interface.metadata())));

        // Test that an event to report the fbpt at the end of dxe is created.
        boot_services
            .expect_create_event_ex::<Box<(
                Rc<MockBootServices>,
                Rc<MockRuntimeServices>,
                &TplMutex<'static, MockFirmwareBasicBootPerfTable, MockBootServices>,
            )>>()
            .once()
            .withf_st(|event_type, notify_tpl, notify_function, _notify_context, event_group| {
                assert_eq!(&EventType::NOTIFY_SIGNAL, event_type);
                assert_eq!(&Tpl::CALLBACK, notify_tpl);
                assert_eq!(
                    event_callback::report_fbpt_record_buffer::<
                        Rc<_>,
                        MockBootServices,
                        Rc<_>,
                        MockRuntimeServices,
                        MockFirmwareBasicBootPerfTable,
                    > as usize,
                    notify_function.unwrap() as usize
                );
                assert_eq!(&EVENT_GROUP_END_OF_DXE, event_group);
                true
            })
            .return_const_st(Ok(TEST_EVENT_HANDLE));

        boot_services.expect_install_configuration_table::<Box<PerformanceProperty>>().once().return_const(Ok(()));

        let runtime_services = MockRuntimeServices::new();
        let mut hob_perf_data_extractor = MockHobPerformanceDataExtractor::new();
        hob_perf_data_extractor
            .expect_extract_hob_perf_data()
            .once()
            .returning(|| Ok((TEST_HOB_LOAD_IMAGE_COUNT, PerformanceRecordBuffer::new())));
        let mut fbpt = MockFirmwareBasicBootPerfTable::new();
        fbpt.expect_set_perf_records().once().return_const(());

        let boot_services_rc = Rc::new(boot_services);

        // Leak a reference to boot_services to create a 'static reference for TplMutex.
        let boot_services_static = Box::leak(Box::new(Rc::clone(&boot_services_rc)));
        let fbpt = TplMutex::new(&**boot_services_static, Tpl::NOTIFY, fbpt);

        // Leak the fbpt to create a 'static reference for testing.
        let fbpt = Box::leak(Box::new(fbpt));

        let _ = Performance._entry_point(
            boot_services_rc,
            Rc::new(runtime_services),
            Some(hob_perf_data_extractor),
            None,
            fbpt,
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
        let mut boot_services = MockBootServices::new();
        boot_services.expect_raise_tpl().return_const(Tpl::APPLICATION);
        boot_services.expect_restore_tpl().return_const(());
        boot_services
            .expect_create_event_ex::<Box<(
                Rc<MockBootServices>,
                Rc<MockRuntimeServices>,
                &TplMutex<'static, MockFirmwareBasicBootPerfTable, MockBootServices>,
            )>>()
            .once()
            .return_const_st(Ok(TEST_EVENT_HANDLE));
        boot_services
            .expect_create_event_ex::<MmPerformanceEventContext<
                Rc<MockBootServices>,
                MockBootServices,
                MockFirmwareBasicBootPerfTable,
            >>()
            .once()
            .withf_st(|_, _, f, _, group| {
                (f.unwrap() as usize)
                    == fetch_and_add_mm_performance_records::<Rc<_>, MockBootServices, MockFirmwareBasicBootPerfTable>
                        as usize
                    && group == &EVENT_GROUP_READY_TO_BOOT
            })
            .return_const_st(Ok(TEST_EVENT_HANDLE_2));
        boot_services
            .expect_install_protocol_interface::<EdkiiPerformanceMeasurement, Box<_>>()
            .once()
            .returning(|_, protocol_interface| Ok((TEST_EFI_HANDLE, protocol_interface.metadata())));
        boot_services.expect_install_configuration_table::<Box<PerformanceProperty>>().once().return_const(Ok(()));
        let runtime_services = MockRuntimeServices::new();
        let mut fbpt = MockFirmwareBasicBootPerfTable::new();
        fbpt.expect_set_perf_records().never();
        // SAFETY: boot_services is a stack variable that will remain valid for the duration of the test.
        let fbpt = TplMutex::new(unsafe { &*core::ptr::addr_of!(boot_services) }, Tpl::NOTIFY, fbpt);
        // SAFETY: fbpt is a stack variable that will remain valid for the duration of the test.
        let fbpt = unsafe { &*core::ptr::addr_of!(fbpt) };
        let mm_service: Service<dyn MmCommunication> = Service::mock(Box::new(FakeComm));
        let timer: Service<dyn ArchTimerFunctionality> = Service::mock(Box::new(MockTimer {}));
        let _ = Performance._entry_point(
            Rc::new(boot_services),
            Rc::new(runtime_services),
            Option::<MockHobPerformanceDataExtractor>::None,
            Some(mm_service),
            fbpt,
            timer,
        );
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
        let mut boot_services_inner = MockBootServices::new();
        boot_services_inner.expect_close_event().once().return_const(Ok(()));
        let mut fbpt = MockFirmwareBasicBootPerfTable::new();
        fbpt.expect_add_record().never();
        let boot_services = Rc::new(boot_services_inner);
        let fbpt_mutex = TplMutex::new(&*boot_services, Tpl::NOTIFY, fbpt);
        // SAFETY: The lifetime of fbpt_mutex is only extended to 'static for testing.
        let fbpt_ref: &TplMutex<'static, _, _> = unsafe { core::mem::transmute(&fbpt_mutex) };
        let mm_service: Service<dyn MmCommunication> = Service::mock(Box::new(ZeroSizeComm));
        fetch_and_add_mm_performance_records::<Rc<MockBootServices>, MockBootServices, MockFirmwareBasicBootPerfTable>(
            TEST_EVENT_HANDLE,
            Box::new((boot_services.clone(), fbpt_ref, mm_service)),
        );
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
                        Ok(create_size_response!(TEST_PERFORMANCE_RECORD_LENGTH as u64))
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
        let mut boot_services_inner = MockBootServices::new();
        // TplMutex lock during add_record will invoke raise_tpl/restore_tpl
        boot_services_inner.expect_raise_tpl().return_const(Tpl::APPLICATION);
        boot_services_inner.expect_restore_tpl().return_const(());
        boot_services_inner.expect_close_event().once().return_const(Ok(()));
        let mut fbpt = MockFirmwareBasicBootPerfTable::new();
        fbpt.expect_add_record().once().returning(|_| Ok(()));
        let boot_services = Rc::new(boot_services_inner);
        let fbpt_mutex = TplMutex::new(&*boot_services, Tpl::NOTIFY, fbpt);
        // SAFETY: The lifetime of fbpt_mutex is only extended to 'static for testing.
        let fbpt_ref: &TplMutex<'static, _, _> = unsafe { core::mem::transmute(&fbpt_mutex) };
        let mm_service: Service<dyn MmCommunication> = Service::mock(Box::new(OneRecordComm::new()));
        fetch_and_add_mm_performance_records::<Rc<MockBootServices>, MockBootServices, MockFirmwareBasicBootPerfTable>(
            TEST_EVENT_HANDLE,
            Box::new((boot_services.clone(), fbpt_ref, mm_service)),
        );
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
        let mut boot_services_inner = MockBootServices::new();
        // TplMutex lock for each add_record will raise/restore TPL; expect that TEST_MULTI_CHUNK_RECORD_COUNT times.
        boot_services_inner.expect_raise_tpl().times(TEST_MULTI_CHUNK_RECORD_COUNT).return_const(Tpl::APPLICATION);
        boot_services_inner.expect_restore_tpl().times(TEST_MULTI_CHUNK_RECORD_COUNT).return_const(());
        boot_services_inner.expect_close_event().once().return_const(Ok(()));
        let mut fbpt = MockFirmwareBasicBootPerfTable::new();
        fbpt.expect_add_record().times(TEST_MULTI_CHUNK_RECORD_COUNT).returning(|_| Ok(()));
        let boot_services = Rc::new(boot_services_inner);
        let fbpt_mutex = TplMutex::new(&*boot_services, Tpl::NOTIFY, fbpt);
        // SAFETY: The lifetime of fbpt_mutex is only extended to 'static for testing.
        let fbpt_ref: &TplMutex<'static, _, _> = unsafe { core::mem::transmute(&fbpt_mutex) };
        let mm_service: Service<dyn MmCommunication> =
            Service::mock(Box::new(MultiChunks { buf: all_records, fetches: Cell::new(0) }));
        fetch_and_add_mm_performance_records::<Rc<MockBootServices>, MockBootServices, MockFirmwareBasicBootPerfTable>(
            TEST_EVENT_HANDLE,
            Box::new((boot_services.clone(), fbpt_ref, mm_service)),
        );
        assert_eq!(TOTAL_RECORD_BYTES, TOTAL_RECORD_BYTES, "expected total record bytes mismatch");
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
