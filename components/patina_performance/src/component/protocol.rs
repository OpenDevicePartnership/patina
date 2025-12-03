//! Patina Performance Protocol
//!
//! Defines the interface for the performance measurement UEFI protocol.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    ffi::{CStr, c_char, c_void},
    sync::atomic::{AtomicBool, Ordering},
};

use alloc::string::ToString;
use patina::{
    performance::{
        error::Error,
        measurement::{CallerIdentifier, create_performance_measurement},
        record::known::{KnownPerfId, KnownPerfToken},
    },
    uefi_protocol::performance_measurement::PerfAttribute,
};
use r_efi::efi;

#[coverage(off)]
// EDK II Performance Measurement Protocol implementation.
//
/// Skip coverage as this function is tested via the generic version, (_create_performance_measurement).
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
    // SAFETY: The caller ensures that string is a valid C string pointer (or NULL).
    let string = unsafe { string.as_ref().map(|s| CStr::from_ptr(s).to_string_lossy().to_string()) };

    // To conform with UEFI spec, `identifier` must be a u32 when passed in.
    // However, FPDT performance measurement IDs are always u16.
    if identifier > u16::MAX as u32 {
        log::error!("Performance: Invalid identifier passed to create_performance_measurement_efiapi: {identifier}",);
        return efi::Status::INVALID_PARAMETER;
    }

    let mut perf_id = identifier as u16;
    let is_known_id = KnownPerfId::try_from(perf_id).is_ok();
    let is_known_token = string.as_ref().is_some_and(|s| KnownPerfToken::try_from(s.as_str()).is_ok());
    if attribute != PerfAttribute::PerfEntry {
        if perf_id != 0 && is_known_id && is_known_token {
            return efi::Status::INVALID_PARAMETER;
        } else if perf_id != 0 && !is_known_id && !is_known_token {
            // By convention, a start measurement should have its lower 4 bits as 0.
            if attribute == PerfAttribute::PerfStartEntry && ((perf_id & 0x000F) != 0) {
                perf_id &= 0xFFF0;
            // By convention, an end measurement should have its lower 4 bits not as 0.
            } else if attribute == PerfAttribute::PerfEndEntry && ((perf_id & 0x000F) == 0) {
                perf_id += 1;
            }
        } else if perf_id == 0 {
            match KnownPerfId::try_from_perf_info(caller_identifier as efi::Handle, string.as_ref(), attribute) {
                Ok(known_perf_id) => perf_id = known_perf_id.as_u16(),
                Err(status) => return status,
            }
        }
    }

    let is_guid = CallerIdentifier::perf_id_is_guid(perf_id);
    // SAFETY: This is enforced by the safety contract of this function.
    // `from_ptr` performs basic validation on the pointer, but cannot guarantee safety.
    let caller_identifier = unsafe {
        match CallerIdentifier::from_ptr(caller_identifier, is_guid) {
            Some(v) => v,
            None => return efi::Status::INVALID_PARAMETER,
        }
    };
    match create_performance_measurement(
        caller_identifier,
        guid,
        string.as_deref(),
        ticker,
        address,
        perf_id,
        attribute,
    ) {
        Ok(_) => efi::Status::SUCCESS,
        Err(Error::OutOfResources) => {
            static HAS_BEEN_LOGGED: AtomicBool = AtomicBool::new(false);
            if HAS_BEEN_LOGGED.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                log::info!("Performance: FBPT is full, can't add more performance records !");
            };
            efi::Status::OUT_OF_RESOURCES
        }
        Err(Error::Efi(status_code)) => {
            log::error!(
                "Performance: Something went wrong in create_performance_measurement. status_code: {status_code:?}"
            );
            status_code.into()
        }
        Err(error) => {
            log::error!("Performance: Something went wrong in create_performance_measurement. Error: {error}",);
            efi::Status::ABORTED
        }
    }
}
