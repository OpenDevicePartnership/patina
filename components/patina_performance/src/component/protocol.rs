//! Patina Performance Protocol
//!
//! Defines the interface for the performance measurement UEFI protocol, and the [`MeasurementProtocolPublisher`]
//! component that installs it. The actual record building and state tracking is delegated to the
//! [`PerformanceManager`] service owned by the DXE Core. This component only bridges that service to the C ABI
//! required by the protocol.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::{c_char, c_void};

use alloc::string::ToString;
use patina::standard::efi;
use patina::{
    BinaryGuid, Char8Str,
    component::{
        component,
        service::{
            Service,
            cell::ServiceCell,
            performance::PerformanceManager,
            uefi_services::protocol::{ProtocolServices, ProtocolServicesExt},
        },
    },
    error::EfiError,
    function,
    performance::{
        error::Error,
        measurement::{CallerIdentifier, PerfAttribute},
        record::known::KnownPerfId,
    },
    protocol::ProtocolInterface,
};

/// GUID for the EDKII Performance Measurement Protocol.
pub const EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID: BinaryGuid =
    BinaryGuid::from_string("C85D06BE-5F75-48CE-A80F-1236BA3B87B1");

/// Function to create performance record with event description and a timestamp.
pub type CreateMeasurementUefi = unsafe extern "efiapi" fn(
    caller_identifier: *const c_void,
    guid: Option<&efi::Guid>,
    string: *const c_char,
    ticker: u64,
    address: usize,
    identifier: u32,
    attribute: PerfAttribute,
) -> efi::Status;

/// EDKII defined Performance Measurement Protocol structure.
#[repr(C)]
pub struct EdkiiPerformanceMeasurementProtocol {
    /// Function to create performance record with event description and a timestamp.
    pub create_performance_measurement: CreateMeasurementUefi,
}

// SAFETY: EdkiiPerformanceMeasurementProtocol implements the EDK II Performance Measurement protocol interface.
// The PROTOCOL_GUID matches the EDK II defined value. The protocol structure layout matches the protocol
// interface requirements.
unsafe impl ProtocolInterface for EdkiiPerformanceMeasurementProtocol {
    const PROTOCOL_GUID: BinaryGuid = EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID;
}

/// The protocol instance installed for C drivers. Its single function has no context parameter, so the
/// struct carries no state of its own, see [`PERF_SERVICE`] for how [`create_performance_measurement_efiapi`]
/// uses the injected [`PerformanceManager`] service.
static PROTOCOL: EdkiiPerformanceMeasurementProtocol =
    EdkiiPerformanceMeasurementProtocol { create_performance_measurement: create_performance_measurement_efiapi };

/// Bridges the [`PerformanceManager`] service injected into [`MeasurementProtocolPublisher`] to
/// [`create_performance_measurement_efiapi`], whose signature is fixed by the EDK II protocol definition and has no
/// context parameter to carry the service through directly.
static PERF_SERVICE: ServiceCell<dyn PerformanceManager> = ServiceCell::new();

/// Registers the performance service used by the EDK II Performance Measurement protocol function.
///
/// ## Errors
///
/// Returns an error string if the service was already registered.
pub(crate) fn set_performance_service(service: Service<dyn PerformanceManager>) -> Result<(), &'static str> {
    PERF_SERVICE.publish(service).map_err(|_| "Performance service already set")
}

/// Installs the EDK II Performance Measurement protocol so C drivers can create performance measurements, and
/// registers the injected [`PerformanceManager`] service so the protocol's context-free callback can use it.
///
/// ## Example Usage
///
/// ```rust
/// use patina_performance::component::protocol::*;
///
/// let component = MeasurementProtocolPublisher::new();
/// ```
#[derive(Default)]
pub struct MeasurementProtocolPublisher;

#[component]
impl MeasurementProtocolPublisher {
    /// Creates a new instance of the component.
    pub const fn new() -> Self {
        Self
    }

    fn entry_point(
        self,
        performance: Service<dyn PerformanceManager>,
        protocols: Service<dyn ProtocolServices>,
    ) -> Result<(), EfiError> {
        set_performance_service(performance).unwrap_or_else(|e| {
            log::error!(
                "[{}]: Performance service was already registered. It should only be registered here! ({e})",
                function!()
            );
        });

        protocols.install_protocol::<EdkiiPerformanceMeasurementProtocol>(None, &PROTOCOL)?;

        Ok(())
    }
}

#[cfg_attr(coverage, coverage(off))]
// EDK II Performance Measurement Protocol implementation.
//
/// Skip coverage as the record-building logic it delegates to is tested in the DXE Core service.
///
/// # Safety
/// `string` must be a valid C string pointer.
/// `caller_identifier` must be a valid image handle or GUID pointer.
pub(crate) unsafe extern "efiapi" fn create_performance_measurement_efiapi(
    caller_identifier: *const c_void,
    guid: Option<&efi::Guid>,
    string: *const c_char,
    ticker: u64,
    address: usize,
    identifier: u32,
    attribute: PerfAttribute,
) -> efi::Status {
    // SAFETY: The caller ensures that `string` is a valid, NUL-terminated CHAR8 pointer (or NULL).
    let string =
        unsafe { string.as_ref().map(|s| Char8Str::from_ptr(core::ptr::from_ref::<c_char>(s).cast()).to_string()) };

    // To conform with UEFI spec, `identifier` must be a u32 when passed in.
    // However, FPDT performance measurement IDs are always u16.
    if identifier > u32::from(u16::MAX) {
        log::error!("Performance: Invalid identifier passed to create_performance_measurement_efiapi: {identifier}");
        return efi::Status::INVALID_PARAMETER;
    }

    let perf_id = match KnownPerfId::normalize_perf_id(
        identifier as u16,
        caller_identifier.cast_mut(),
        string.as_ref(),
        attribute,
    ) {
        Ok(perf_id) => perf_id,
        Err(status) => return status,
    };

    let is_guid = CallerIdentifier::perf_id_is_guid(perf_id);
    // SAFETY: This is enforced by the safety contract of this function.
    // `from_ptr` performs basic validation on the pointer, but cannot guarantee safety.
    let caller_identifier = unsafe {
        match CallerIdentifier::from_ptr(caller_identifier, is_guid) {
            Some(v) => v,
            None => return efi::Status::INVALID_PARAMETER,
        }
    };

    let Some(service) = PERF_SERVICE.get() else {
        log::error!("Performance: create_performance_measurement_efiapi called before service registration.");
        return efi::Status::NOT_READY;
    };

    match service.create_measurement(caller_identifier, guid, string.as_deref(), ticker, address, perf_id, attribute) {
        Ok(()) => efi::Status::SUCCESS,
        Err(Error::OutOfResources) => efi::Status::OUT_OF_RESOURCES,
        Err(Error::Efi(status_code)) => {
            log::error!(
                "Performance: Something went wrong in create_performance_measurement. status_code: {status_code:?}"
            );
            status_code.into()
        }
        Err(error) => {
            log::error!("Performance: Something went wrong in create_performance_measurement. Error: {error}");
            efi::Status::ABORTED
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina::component::service::{
        performance::MockPerformanceManager,
        uefi_services::protocol::{Handle, MockProtocolServices},
    };

    fn mock_service() -> Service<dyn PerformanceManager> {
        Service::mock(Box::new(MockPerformanceManager::new()))
    }

    #[test]
    fn test_protocol_measurement_publisher_entry_point_installs_protocol() {
        let mut protocols = MockProtocolServices::new();
        protocols
            .expect_install_interface()
            .once()
            .withf(|handle, guid, _interface| {
                assert_eq!(&None, handle);
                assert_eq!(guid, &EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID);
                true
            })
            .returning(|_, _, _| Ok(Handle::from_raw(1_usize as efi::Handle).unwrap()));

        let result =
            MeasurementProtocolPublisher::new().entry_point(mock_service(), Service::mock(Box::new(protocols)));

        assert!(result.is_ok());
    }
}
