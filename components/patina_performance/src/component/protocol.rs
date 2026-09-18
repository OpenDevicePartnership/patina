//! Patina Performance Protocol
//!
//! Defines the interface for the performance measurement UEFI protocol. The protocol is produced by this component;
//! the actual record building and state tracking is delegated to the [`PerformanceManager`] service owned by the
//! DXE Core.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    cell::OnceCell,
    ffi::{c_char, c_void},
    sync::atomic::{AtomicBool, Ordering},
};

use alloc::string::ToString;
use patina::standard::efi;
use patina::{
    BinaryGuid, Char8Str,
    component::service::{Service, performance::PerformanceManager},
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

/// Global holder for the performance service so the C-ABI protocol function can reach it.
///
/// The EDK II Performance Measurement protocol exposes a bare `extern "efiapi"` function pointer that cannot capture
/// the injected [`Service`]. The service is therefore stashed here once during component initialization and read back
/// by [`create_performance_measurement_efiapi`].
struct ServiceHolder {
    service: OnceCell<Service<dyn PerformanceManager>>,
    initializing: AtomicBool,
}

// SAFETY: All writes go through `set`, which is serialized by the `initializing` flag. Reads via `get` only observe a
// fully-initialized value. `Service` is itself `Send + Sync`.
unsafe impl Sync for ServiceHolder {}

impl ServiceHolder {
    const fn new() -> Self {
        Self { service: OnceCell::new(), initializing: AtomicBool::new(false) }
    }

    fn set(&self, service: Service<dyn PerformanceManager>) -> Result<(), &'static str> {
        if self.initializing.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            let result = self.service.set(service).map_err(|_| "Performance service already set");
            self.initializing.store(false, Ordering::Release);
            return result;
        }
        Err("Performance service is currently being set elsewhere")
    }

    fn get(&self) -> Option<&Service<dyn PerformanceManager>> {
        if self.initializing.load(Ordering::Acquire) { None } else { self.service.get() }
    }
}

static PERF_SERVICE: ServiceHolder = ServiceHolder::new();

/// Registers the performance service used by the EDK II Performance Measurement protocol function.
///
/// ## Errors
///
/// Returns an error string if the service was already registered.
pub(crate) fn set_performance_service(service: Service<dyn PerformanceManager>) -> Result<(), &'static str> {
    PERF_SERVICE.set(service)
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
    use patina::component::service::performance::MockPerformanceManager;

    fn mock_service() -> Service<dyn PerformanceManager> {
        Service::mock(Box::new(MockPerformanceManager::new()))
    }

    #[test]
    fn test_service_holder_set_get_lifecycle() {
        let holder = ServiceHolder::new();

        // Nothing is registered yet.
        assert!(holder.get().is_none());

        // First registration succeeds and is observable.
        assert!(holder.set(mock_service()).is_ok());
        assert!(holder.get().is_some());

        // A second registration is rejected.
        assert_eq!(holder.set(mock_service()), Err("Performance service already set"));

        // While a registration is in flight, `set` is rejected and `get` reports nothing.
        holder.initializing.store(true, Ordering::Release);
        assert_eq!(holder.set(mock_service()), Err("Performance service is currently being set elsewhere"));
        assert!(holder.get().is_none());
    }
}
