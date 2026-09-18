//! Functionality for managing performance measurements.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{ffi::c_void, mem, ops::BitOr};

use crate::standard::efi;
use crate::{bit, performance::record::known::KnownPerfId};

/// The attribute of the measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
pub enum PerfAttribute {
    /// A `PERF_START/PERF_START_EX` record.
    PerfStartEntry,
    /// A `PERF_END/PERF_END_EX` record.
    PerfEndEntry,
    /// A general performance record.
    PerfEntry,
}

/// Represents the `caller_identifier` used in performance measurements.
/// Due to legacy reasons, this can either be an handle or a pointer to a GUID.
pub enum CallerIdentifier {
    /// Caller identifier for perf measurement is a handle (legacy).
    Handle(efi::Handle),
    /// Caller identifier for perf measurement is a GUID pointer (new).
    Guid(efi::Guid),
}

impl CallerIdentifier {
    /// Performs basic checks on a pointer claiming to be a Guid.
    pub fn validate_guid(ptr: *const c_void) -> bool {
        // Check that pointer is not null and is properly aligned for a Guid.
        !ptr.is_null() && (ptr as usize).is_multiple_of(mem::align_of::<efi::Guid>())
    }
    /// Creates a `CallerIdentifier` from a raw pointer.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the pointer is valid and points to either an image handle or a GUID.
    pub unsafe fn from_ptr(ptr: *const c_void, is_guid: bool) -> Option<Self> {
        if is_guid {
            if !Self::validate_guid(ptr) {
                return None;
            }
            // SAFETY: The safety contract of this function ensures that `ptr` is a valid pointer to a GUID.
            // `validate_guid` performs basic validations but cannot guarantee safety.
            Some(CallerIdentifier::Guid(unsafe { *(ptr as *const efi::Guid) }))
        } else {
            Some(CallerIdentifier::Handle(ptr.cast_mut()))
        }
    }

    /// Checks if the `CallerIdentifier` is a GUID pointer.
    ///
    /// This is the case with newly-added performance IDs used for signaling events and callbacks
    /// that were not backwards-compatible with the existing `create_performance_measurement` interface.
    /// These ids are: `PerfEvent`, `PerfEventSignalStart`, `PerfEventSignalEnd`, `PerfCallbackStart`, `PerfCallbackEnd`,
    /// `PerfFunctionStart`, `PerfFunctionEnd`, `PerfInModuleStart`, `PerfInModuleEnd`, `PerfCrossModuleStart`, `PerfCrossModuleEnd`.
    pub fn perf_id_is_guid(perf_id: u16) -> bool {
        let perf_id = match KnownPerfId::try_from(perf_id) {
            Ok(id) => id,
            Err(()) => return false,
        };
        matches!(
            perf_id,
            KnownPerfId::PerfEvent
                | KnownPerfId::PerfEventSignalStart
                | KnownPerfId::PerfEventSignalEnd
                | KnownPerfId::PerfCallbackStart
                | KnownPerfId::PerfCallbackEnd
                | KnownPerfId::PerfFunctionStart
                | KnownPerfId::PerfFunctionEnd
                | KnownPerfId::PerfInModuleStart
                | KnownPerfId::PerfInModuleEnd
                | KnownPerfId::PerfCrossModuleStart
                | KnownPerfId::PerfCrossModuleEnd
        )
    }

    /// Returns the image handle if the `CallerIdentifier` is an image handle.
    pub fn as_handle(&self) -> Option<efi::Handle> {
        if let CallerIdentifier::Handle(h) = *self { Some(h) } else { None }
    }

    /// Returns the GUID if the `CallerIdentifier` is a GUID pointer.
    pub fn as_guid(&self) -> Option<&efi::Guid> {
        if let CallerIdentifier::Guid(ref g) = *self { Some(g) } else { None }
    }
}

/// Measurement enum that represents the different performance measurements that can be enabled.
#[derive(Debug, PartialEq)]
#[repr(u32)]
pub enum Measurement {
    /// Dispatch modules entry point execution
    StartImage = bit!(0),
    /// Load a dispatched module.
    LoadImage = bit!(1),
    /// Diver binding support function call.
    DriverBindingSupport = bit!(2),
    /// Diver binding start function call.
    DriverBindingStart = bit!(3),
    /// Diver binding stop function call.
    DriverBindingStop = bit!(4),
}

impl Measurement {
    /// [u32] representation of the measurement.
    pub const fn as_u32(&self) -> u32 {
        match self {
            Measurement::StartImage => Measurement::StartImage as u32,
            Measurement::LoadImage => Measurement::LoadImage as u32,
            Measurement::DriverBindingSupport => Measurement::DriverBindingSupport as u32,
            Measurement::DriverBindingStart => Measurement::DriverBindingStart as u32,
            Measurement::DriverBindingStop => Measurement::DriverBindingStop as u32,
        }
    }
}

/// Implement bitwise OR for measurements (`Measurement | Measurement`).
impl BitOr for Measurement {
    type Output = u32;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.as_u32() | rhs.as_u32()
    }
}

/// Implement bitwise OR for measurements and u32 (`Measurement | u32`).
impl BitOr<u32> for Measurement {
    type Output = u32;

    fn bitor(self, rhs: u32) -> Self::Output {
        self.as_u32() | rhs
    }
}

/// Implement bitwise OR for u32 and measurements (`u32 | Measurement`).
impl BitOr<Measurement> for u32 {
    type Output = u32;

    fn bitor(self, rhs: Measurement) -> Self::Output {
        self | rhs.as_u32()
    }
}

/// Performance property structure used to store performance related properties.
#[repr(C)]
pub struct PerformanceProperty {
    revision: u32,
    reserved: u32,
    frequency: u64,
    timer_start_value: u64,
    timer_end_value: u64,
}

impl PerformanceProperty {
    /// Creates a new `PerformanceProperty` with the specified frequency, timer start value, and timer end value.
    ///
    /// # Arguments
    /// - `frequency`: The frequency of the performance measurement.
    /// - `timer_start_value`: The start value of the timer.
    /// - `timer_end_value`: The end value of the timer.
    ///
    /// # Returns
    /// A new instance of `PerformanceProperty`.
    ///
    /// # Example
    /// ```rust
    /// use patina::performance::measurement::PerformanceProperty;
    /// let performance_property = PerformanceProperty::new(1000, 0, 100);
    /// ```
    pub fn new(frequency: u64, timer_start_value: u64, timer_end_value: u64) -> Self {
        Self { revision: 0x1, reserved: 0, frequency, timer_start_value, timer_end_value }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    use alloc::boxed::Box;
    use core::ptr;

    use crate::performance::record::{
        PerformanceRecord,
        extended::{GuidEventRecord, GuidQwordEventRecord, GuidQwordStringEventRecord},
    };

    #[test]
    fn test_validate_guid_caller_identifier() {
        let valid_guid = efi::Guid::from_bytes(&[1; 16]);
        let valid_guid_ptr = &raw const valid_guid as *const c_void;

        #[allow(clippy::manual_dangling_ptr)]
        let invalid_guid_ptr = 0x1_usize as *const c_void; // Misaligned pointer.
        let null_guid_ptr = ptr::null(); // Null pointer.

        assert!(CallerIdentifier::validate_guid(valid_guid_ptr));
        assert!(!CallerIdentifier::validate_guid(invalid_guid_ptr));
        assert!(!CallerIdentifier::validate_guid(null_guid_ptr));

        // SAFETY: Test code - valid pointer to a GUID.
        let caller_id_guid = unsafe { CallerIdentifier::from_ptr(valid_guid_ptr, true) }.unwrap();
        assert!(matches!(caller_id_guid, CallerIdentifier::Guid(_)));

        // Any value is valid as a handle.
        // SAFETY: Test code - valid pointer to a handle.
        let caller_id_handle = unsafe { CallerIdentifier::from_ptr(0x2_usize as *const c_void, false) }.unwrap();
        assert!(matches!(caller_id_handle, CallerIdentifier::Handle(_)));

        // SAFETY: Test code - invalid pointer to a GUID.
        assert!(unsafe { CallerIdentifier::from_ptr(invalid_guid_ptr, true) }.is_none());
    }

    #[test]
    fn test_perf_id_is_guid() {
        // PerfEvent uses a GUID caller identifier.
        let guid_perf_id = KnownPerfId::PerfEvent;
        assert!(CallerIdentifier::perf_id_is_guid(guid_perf_id as u16));

        // ModuleStart uses a handle caller identifier.
        let non_guid_perf_id = KnownPerfId::ModuleStart;
        assert!(!CallerIdentifier::perf_id_is_guid(non_guid_perf_id as u16));

        // Unknown perf ID.
        let unknown_perf_id = 0xFFFF;
        assert!(!CallerIdentifier::perf_id_is_guid(unknown_perf_id));
    }

    #[test]
    fn test_measurement() {
        let start_image = Measurement::StartImage;
        let load_image = Measurement::LoadImage;
        let driver_binding_support = Measurement::DriverBindingSupport;
        let driver_binding_start = Measurement::DriverBindingStart;
        let driver_binding_stop = Measurement::DriverBindingStop;

        assert_eq!(start_image.as_u32(), 1);
        assert_eq!(load_image.as_u32(), 2);
        assert_eq!(driver_binding_support.as_u32(), 4);
        assert_eq!(driver_binding_start.as_u32(), 8);
        assert_eq!(driver_binding_stop.as_u32(), 16);

        let combined = start_image | load_image | driver_binding_support;
        assert_eq!(combined, 7);
    }

    /// Validates that each `KnownPerfId` maps to the FPDT record type expected by the EDK2
    /// Dp.c parser (ShellPkg/DynamicCommand/DpDynamicCommand/Dp.c). A mismatch causes
    /// ASSERT(FALSE) in the C parser at runtime.
    #[test]
    fn test_known_perf_id_record_types_match_edk2_dp() {
        let guid = crate::BinaryGuid::ZERO;

        // Expected mappings derived from the switch/case in Dp.c:
        //   FPDT_GUID_EVENT_TYPE           (0x1010): MODULE_START_ID, MODULE_END_ID
        //   FPDT_GUID_QWORD_EVENT_TYPE     (0x1013): MODULE_LOADIMAGE_*, MODULE_DB_START, MODULE_DB_SUPPORT_*, MODULE_DB_STOP_*
        //   FPDT_GUID_QWORD_STRING_EVENT   (0x1014): MODULE_DB_END_ID only
        //   FPDT_DYNAMIC_STRING_EVENT_TYPE (0x1011): any (no assert)
        //   FPDT_DUAL_GUID_STRING_EVENT    (0x1012): any (no assert)
        let expected: &[(u16, u16)] = &[
            (KnownPerfId::ModuleStart.as_u16(), GuidEventRecord::TYPE),
            (KnownPerfId::ModuleEnd.as_u16(), GuidEventRecord::TYPE),
            (KnownPerfId::ModuleLoadImageStart.as_u16(), GuidQwordEventRecord::TYPE),
            (KnownPerfId::ModuleLoadImageEnd.as_u16(), GuidQwordEventRecord::TYPE),
            (KnownPerfId::ModuleDbStart.as_u16(), GuidQwordEventRecord::TYPE),
            (KnownPerfId::ModuleDbEnd.as_u16(), GuidQwordStringEventRecord::TYPE),
            (KnownPerfId::ModuleDbSupportStart.as_u16(), GuidQwordEventRecord::TYPE),
            (KnownPerfId::ModuleDbSupportEnd.as_u16(), GuidQwordEventRecord::TYPE),
            (KnownPerfId::ModuleDbStopStart.as_u16(), GuidQwordEventRecord::TYPE),
            (KnownPerfId::ModuleDbStopEnd.as_u16(), GuidQwordEventRecord::TYPE),
        ];

        for &(perf_id, expected_type) in expected {
            let record: Box<dyn PerformanceRecord> = match KnownPerfId::try_from(perf_id).unwrap() {
                KnownPerfId::ModuleStart | KnownPerfId::ModuleEnd => {
                    Box::new(GuidEventRecord::new(perf_id, 0, 0, guid))
                }
                KnownPerfId::ModuleLoadImageStart | KnownPerfId::ModuleLoadImageEnd => {
                    Box::new(GuidQwordEventRecord::new(perf_id, 0, 0, guid, 0))
                }
                KnownPerfId::ModuleDbStart
                | KnownPerfId::ModuleDbSupportStart
                | KnownPerfId::ModuleDbSupportEnd
                | KnownPerfId::ModuleDbStopStart
                | KnownPerfId::ModuleDbStopEnd => Box::new(GuidQwordEventRecord::new(perf_id, 0, 0, guid, 0)),
                KnownPerfId::ModuleDbEnd => Box::new(GuidQwordStringEventRecord::new(perf_id, 0, 0, guid, 0, "")),
                _ => continue,
            };
            assert_eq!(
                record.record_type(),
                expected_type,
                "KnownPerfId 0x{:02X} should produce record type 0x{:04X}, got 0x{:04X}",
                perf_id,
                expected_type,
                record.record_type()
            );
        }
    }
}
