//! Provides Pointer HID support.
//!
//! This module handles the core logic for processing pointer input from HID
//! devices.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
pub(crate) mod absolute_pointer;

use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};

use r_efi::{efi, protocols};

use hidparser::{
    ReportDescriptor, ReportField, VariableField,
    report_data_types::{ReportId, Usage},
};

use patina::{
    boot_services::{BootServices, c_ptr::PtrMetadata, tpl::Tpl},
    tpl_mutex::TplMutex,
};

use crate::hid_io::{HidIo, HidReportReceiver};

use self::absolute_pointer::AbsolutePointerFfi;

// Usages supported by this module.
const GENERIC_DESKTOP_X: u32 = 0x00010030;
const GENERIC_DESKTOP_Y: u32 = 0x00010031;
const GENERIC_DESKTOP_Z: u32 = 0x00010032;
const GENERIC_DESKTOP_WHEEL: u32 = 0x00010038;
const BUTTON_MIN: u32 = 0x00090001;
const BUTTON_MAX: u32 = 0x00090020;
const DIGITIZER_SWITCH_MIN: u32 = 0x000d0042;
const DIGITIZER_SWITCH_MAX: u32 = 0x000d0046;
const DIGITIZER_CONTACT_COUNT: u32 = 0x000d0054;

// Number of points on the X/Y axis for this implementation.
const AXIS_RESOLUTION: u64 = 1024;
const CENTER: u64 = AXIS_RESOLUTION / 2;

/// Mutable pointer state updated during report processing.
#[derive(Debug)]
pub(crate) struct PointerState {
    pub(crate) state_changed: bool,
    pub(crate) current_state: protocols::absolute_pointer::State,
    contact_count: Option<usize>,
}

impl PointerState {
    // Creates a new PointerState centered at the default position.
    fn new() -> Self {
        let mut state = Self { state_changed: false, current_state: Default::default(), contact_count: None };
        state.reset();
        state
    }

    /// Resets the pointer state to the default values.
    pub(crate) fn reset(&mut self) {
        self.current_state = Default::default();
        self.current_state.current_x = CENTER;
        self.current_state.current_y = CENTER;
        self.state_changed = false;
        self.contact_count = None;
    }

    // Helper routine that handles projecting relative and absolute axis reports onto the fixed
    // absolute report axis that this driver produces.
    fn resolve_axis(current_value: u64, field: &VariableField, report: &[u8]) -> Option<u64> {
        if field.attributes.relative {
            let new_value = current_value as i64 + field.field_value(report)?;
            Some(new_value.clamp(0, AXIS_RESOLUTION as i64) as u64)
        } else {
            let mut new_value = field.field_value(report)?;
            new_value = new_value.checked_sub(i32::from(field.logical_minimum) as i64)?;
            new_value = (new_value * AXIS_RESOLUTION as i64 * 1000) / (field.field_range()? as i64 * 1000);
            Some(new_value.clamp(0, AXIS_RESOLUTION as i64) as u64)
        }
    }

    // Updates the axis value from the given report field.
    fn axis_handler(&mut self, field: &VariableField, report: &[u8]) {
        let current_value = match field.usage.into() {
            GENERIC_DESKTOP_X => &mut self.current_state.current_x,
            GENERIC_DESKTOP_Y => &mut self.current_state.current_y,
            GENERIC_DESKTOP_Z | GENERIC_DESKTOP_WHEEL => &mut self.current_state.current_z,
            _ => return,
        };
        if let Some(new_value) = Self::resolve_axis(*current_value, field, report)
            && *current_value != new_value
        {
            *current_value = new_value;
            self.state_changed = true;
        }
    }

    // Updates button state from the given report field.
    fn button_handler(&mut self, field: &VariableField, report: &[u8]) {
        let shift = match field.usage.into() {
            x @ BUTTON_MIN..=BUTTON_MAX => x - BUTTON_MIN,
            x @ DIGITIZER_SWITCH_MIN..=DIGITIZER_SWITCH_MAX => x - DIGITIZER_SWITCH_MIN,
            _ => return,
        };

        if let Some(button_value) = field.field_value(report) {
            let button_value = button_value as u32;

            if shift > u32::BITS {
                return;
            }
            let button_value = button_value << shift;

            let new_buttons = self.current_state.active_buttons
                & !(1 << shift) // zero the relevant bit in the button state field.
                | button_value; // or in the current button state into that bit position.

            if new_buttons != self.current_state.active_buttons {
                self.current_state.active_buttons = new_buttons;
                self.state_changed = true;
            }
        }
    }

    // Updates the contact count from the given report field.
    fn contact_count_handler(&mut self, field: &VariableField, report: &[u8]) {
        if let Some(contact_count) = field.field_value(report) {
            if let Ok(contact_count) = usize::try_from(contact_count) {
                self.contact_count = Some(contact_count);
            } else {
                log::debug!("Ignoring negative contact_count: {}", contact_count);
            }
        }
    }
}

// Function pointer type for per-field report processing.
type ReportHandler = fn(&mut PointerState, field: &VariableField, report: &[u8]);

// Maps a given HID report field to a routine that handles input from it.
struct PointerInputFieldHandler {
    field: VariableField,
    report_handler: ReportHandler,
}

// Defines a report and the fields of interest within it.
#[derive(Default)]
struct PointerInputReportSpec {
    report_id: Option<ReportId>,
    report_size: usize,
    relevant_fields: Vec<PointerInputFieldHandler>,
}

// Defines counters for determining how many contact points need to be handled
#[derive(Default, Clone)]
struct UsageUpdateCounter {
    x: usize,
    y: usize,
    z: usize,
    button: usize,
    switch: usize,
}

// Core pointer data processing logic, independent of UEFI boot services.
pub(crate) struct PointerProcessor {
    input_reports: BTreeMap<Option<ReportId>, PointerInputReportSpec>,
    pub(crate) supported_usages: BTreeSet<Usage>,
    report_id_present: bool,
}

impl PointerProcessor {
    // Creates a new processor with empty report maps.
    fn new() -> Self {
        Self { input_reports: BTreeMap::new(), supported_usages: BTreeSet::new(), report_id_present: false }
    }

    // Parses a report descriptor and registers relevant field handlers.
    fn process_descriptor(&mut self, descriptor: ReportDescriptor) -> Result<(), efi::Status> {
        let multiple_reports = descriptor.input_reports.len() > 1;
        log::trace!("pointer::process_descriptor: {:?} input report(s) in descriptor", descriptor.input_reports.len(),);

        for report in &descriptor.input_reports {
            let mut report_data = PointerInputReportSpec { report_id: report.report_id, ..Default::default() };

            self.report_id_present = report.report_id.is_some();

            if multiple_reports && !self.report_id_present {
                return Err(efi::Status::DEVICE_ERROR);
            }

            report_data.report_size = report.size_in_bits.div_ceil(8);

            for field in &report.fields {
                if let ReportField::Variable(field) = field {
                    let handler: Option<(ReportHandler, bool)> = match field.usage.into() {
                        // Contact count is processed first to ensure that if counters are present, we can enforce
                        // them when processing the rest of the fields in the report.
                        DIGITIZER_CONTACT_COUNT => Some((PointerState::contact_count_handler, true)),
                        GENERIC_DESKTOP_X | GENERIC_DESKTOP_Y | GENERIC_DESKTOP_Z | GENERIC_DESKTOP_WHEEL => {
                            Some((PointerState::axis_handler, false))
                        }
                        BUTTON_MIN..=BUTTON_MAX => Some((PointerState::button_handler, false)),
                        DIGITIZER_SWITCH_MIN..=DIGITIZER_SWITCH_MAX => Some((PointerState::button_handler, false)),
                        _ => None,
                    };
                    if let Some((report_handler, insert_first)) = handler {
                        let entry = PointerInputFieldHandler { field: field.clone(), report_handler };
                        if insert_first {
                            report_data.relevant_fields.insert(0, entry);
                        } else {
                            report_data.relevant_fields.push(entry);
                        }
                        self.supported_usages.insert(field.usage);
                    }
                }
            }

            if !report_data.relevant_fields.is_empty() {
                self.input_reports.insert(report_data.report_id, report_data);
            }
        }
        if !self.input_reports.is_empty() {
            log::trace!(
                "pointer::process_descriptor: {:?} usable report(s) with {:?} supported usage(s)",
                self.input_reports.len(),
                self.supported_usages.len(),
            );
            Ok(())
        } else {
            Err(efi::Status::UNSUPPORTED)
        }
    }

    // Processes an incoming HID report and updates pointer state.
    fn process_report(&self, report: &[u8], state: &mut PointerState) {
        if report.is_empty() {
            return;
        }

        log::trace!("pointer::process_report: {:?} bytes", report.len());

        let (report_id, report) = match self.report_id_present {
            true => (Some(ReportId::from(&report[0..1])), &report[1..]),
            false => (None, &report[0..]),
        };

        if report.is_empty() {
            return;
        }

        if let Some(report_data) = self.input_reports.get(&report_id) {
            state.contact_count = None;
            let mut counters = UsageUpdateCounter::default();

            if report.len() != report_data.report_size {
                log::trace!(
                    "receive_report: unexpected report length for report_id: {:?}. expected {:?}, actual {:?}",
                    report_id,
                    report_data.report_size,
                    report.len()
                );
            }

            for field in &report_data.relevant_fields {
                let counter = match field.field.usage.into() {
                    DIGITIZER_CONTACT_COUNT => None,
                    GENERIC_DESKTOP_X => Some(&mut counters.x),
                    GENERIC_DESKTOP_Y => Some(&mut counters.y),
                    GENERIC_DESKTOP_Z | GENERIC_DESKTOP_WHEEL => Some(&mut counters.z),
                    BUTTON_MIN..=BUTTON_MAX => Some(&mut counters.button),
                    DIGITIZER_SWITCH_MIN..=DIGITIZER_SWITCH_MAX => Some(&mut counters.switch),
                    _ => continue,
                };

                if let Some(counter) = counter {
                    if state.contact_count.is_some_and(|c| *counter >= c) {
                        continue;
                    }
                    *counter += 1;
                }

                (field.report_handler)(state, &field.field, report);
            }
        }
    }
}

/// Pointer HID handler that processes reports and produces UEFI AbsolutePointer state.
pub struct PointerHidHandler<T: BootServices + Clone + 'static> {
    boot_services: &'static T,
    controller: efi::Handle,
    absolute_pointer_key: Option<PtrMetadata<'static, Box<AbsolutePointerFfi<T>>>>,
    pub(crate) processor: PointerProcessor,
    pub(crate) state: TplMutex<PointerState, T>,
}

impl<T: BootServices + Clone + 'static> PointerHidHandler<T> {
    /// Creates a fully initialized Pointer HID handler for the given controller.
    ///
    /// Returns a boxed handler because protocol installation stores raw pointers to `self`.
    /// Boxing first ensures those pointers remain valid (the handler is never moved after boxing).
    pub fn new(
        boot_services: &'static T,
        controller: efi::Handle,
        hid_io: &dyn HidIo,
    ) -> Result<Box<Self>, efi::Status> {
        let mut processor = PointerProcessor::new();
        let descriptor = hid_io.get_report_descriptor()?;
        processor.process_descriptor(descriptor)?;

        let mut handler = Box::new(Self {
            boot_services,
            controller,
            absolute_pointer_key: None,
            processor,
            state: TplMutex::new((*boot_services).clone(), Tpl::NOTIFY, PointerState::new()),
        });

        let key = AbsolutePointerFfi::install(boot_services, controller, &mut *handler)?;
        handler.absolute_pointer_key = Some(key);
        Ok(handler)
    }

    #[cfg(test)]
    pub fn new_for_test(boot_services: &'static T) -> Self {
        Self {
            boot_services,
            controller: core::ptr::null_mut(),
            absolute_pointer_key: None,
            processor: PointerProcessor::new(),
            state: TplMutex::new((*boot_services).clone(), Tpl::NOTIFY, PointerState::new()),
        }
    }
}

impl<T: BootServices + Clone + 'static> HidReportReceiver for PointerHidHandler<T> {
    fn receive_report(&mut self, report: &[u8], _hid_io: &dyn HidIo) {
        log::trace!("pointer::receive_report: {:?} bytes", report.len());
        self.processor.process_report(report, &mut self.state.lock());
    }
}

impl<T: BootServices + Clone + 'static> Drop for PointerHidHandler<T> {
    fn drop(&mut self) {
        if let Some(key) = self.absolute_pointer_key.take()
            && let Err(status) = AbsolutePointerFfi::<T>::uninstall(self.boot_services, self.controller, key)
        {
            log::error!("PointerHidHandler::drop: Failed to uninstall absolute_pointer: {:?}", status);
        }
    }
}

#[cfg(test)]
mod test {
    use hidparser::{
        ReportDescriptor, ReportField, VariableField,
        report_data_types::{ReportAttributes, Usage},
    };
    use r_efi::efi;

    use super::*;

    // Creates an absolute VariableField at the given bit range with the given usage.
    fn absolute_field(usage: u32, bits: core::ops::Range<u32>, logical_max: i32) -> VariableField {
        VariableField {
            bits,
            usage: Usage::from(usage),
            logical_minimum: 0.into(),
            logical_maximum: logical_max.into(),
            attributes: ReportAttributes { relative: false, ..Default::default() },
            ..Default::default()
        }
    }

    // Creates a relative VariableField at the given bit range with the given usage.
    fn relative_field(usage: u32, bits: core::ops::Range<u32>, logical_min: i32, logical_max: i32) -> VariableField {
        VariableField {
            bits,
            usage: Usage::from(usage),
            logical_minimum: logical_min.into(),
            logical_maximum: logical_max.into(),
            attributes: ReportAttributes { relative: true, ..Default::default() },
            ..Default::default()
        }
    }

    // Builds a minimal ReportDescriptor with the given fields, no report ID.
    fn descriptor_with_fields(fields: Vec<ReportField>, size_in_bits: usize) -> ReportDescriptor {
        ReportDescriptor {
            input_reports: alloc::vec![hidparser::Report { report_id: None, size_in_bits, fields }],
            output_reports: alloc::vec![],
            bad_input_reports: alloc::vec![],
            bad_output_reports: alloc::vec![],
            features: alloc::vec![],
            bad_features: alloc::vec![],
        }
    }

    // --- PointerState handler tests ---

    #[test]
    fn reset_sets_center_and_clears_state() {
        let mut state = PointerState::new();
        state.current_state.current_x = 100;
        state.state_changed = true;
        state.contact_count = Some(5);

        state.reset();

        assert_eq!(state.current_state.current_x, CENTER);
        assert_eq!(state.current_state.current_y, CENTER);
        assert!(!state.state_changed);
        assert_eq!(state.contact_count, None);
    }

    #[test]
    fn axis_handler_updates_absolute_x() {
        let mut state = PointerState::new();
        // 8-bit field at bits 0..8, logical range 0..255
        let field = absolute_field(GENERIC_DESKTOP_X, 0..8, 255);
        // Report value 128 → projects to 514 on 0..1024 axis (128/255 * 1024 ≈ 514)
        let report = [128u8];

        state.axis_handler(&field, &report);

        assert!(state.state_changed);
        assert_eq!(state.current_state.current_x, 514);
    }

    #[test]
    fn axis_handler_updates_absolute_y() {
        let mut state = PointerState::new();
        let field = absolute_field(GENERIC_DESKTOP_Y, 0..8, 255);
        let report = [0u8];

        state.axis_handler(&field, &report);

        assert!(state.state_changed);
        assert_eq!(state.current_state.current_y, 0);
    }

    #[test]
    fn axis_handler_updates_relative_z() {
        let mut state = PointerState::new();
        // 8-bit signed relative field, range -127..127
        let field = relative_field(GENERIC_DESKTOP_Z, 0..8, -127, 127);
        // Report value 10 (relative)
        let report = [10u8];

        state.axis_handler(&field, &report);

        assert!(state.state_changed);
        assert_eq!(state.current_state.current_z, 10);
    }

    #[test]
    fn same_value_does_not_set_state_changed() {
        let mut state = PointerState::new();
        let field = absolute_field(GENERIC_DESKTOP_X, 0..8, 255);
        // Value that maps to CENTER (512)
        let report = [128u8];
        state.axis_handler(&field, &report);
        assert!(state.state_changed);

        // Reset flag and send same value again.
        state.state_changed = false;
        state.axis_handler(&field, &report);
        assert!(!state.state_changed);
    }

    #[test]
    fn button_handler_sets_button_bit() {
        let mut state = PointerState::new();
        // Button 1 (usage 0x00090001): 1-bit field at bit 0
        let field = absolute_field(BUTTON_MIN, 0..1, 1);
        let report = [0x01u8];

        state.button_handler(&field, &report);

        assert!(state.state_changed);
        assert_eq!(state.current_state.active_buttons, 1);
    }

    #[test]
    fn button_handler_clears_button_bit() {
        let mut state = PointerState::new();
        state.current_state.active_buttons = 1;
        let field = absolute_field(BUTTON_MIN, 0..1, 1);
        let report = [0x00u8];

        state.button_handler(&field, &report);

        assert!(state.state_changed);
        assert_eq!(state.current_state.active_buttons, 0);
    }

    #[test]
    fn contact_count_handler_sets_contact_count() {
        let mut state = PointerState::new();
        let field = absolute_field(DIGITIZER_CONTACT_COUNT, 0..8, 255);
        let report = [3u8];

        state.contact_count_handler(&field, &report);

        assert_eq!(state.contact_count, Some(3));
    }

    // --- process_descriptor tests ---

    #[test]
    fn process_descriptor_with_x_and_y_succeeds() {
        let mut processor = PointerProcessor::new();

        let fields = alloc::vec![
            ReportField::Variable(absolute_field(GENERIC_DESKTOP_X, 0..8, 255)),
            ReportField::Variable(absolute_field(GENERIC_DESKTOP_Y, 8..16, 255)),
        ];
        let descriptor = descriptor_with_fields(fields, 16);

        assert_eq!(processor.process_descriptor(descriptor), Ok(()));
        assert!(processor.supported_usages.contains(&Usage::from(GENERIC_DESKTOP_X)));
        assert!(processor.supported_usages.contains(&Usage::from(GENERIC_DESKTOP_Y)));
        assert_eq!(processor.input_reports.len(), 1);
    }

    #[test]
    fn process_descriptor_with_no_relevant_fields_returns_unsupported() {
        let mut processor = PointerProcessor::new();

        let descriptor = descriptor_with_fields(alloc::vec![], 0);

        assert_eq!(processor.process_descriptor(descriptor), Err(efi::Status::UNSUPPORTED));
    }

    #[test]
    fn process_descriptor_places_contact_count_first() {
        let mut processor = PointerProcessor::new();

        let fields = alloc::vec![
            ReportField::Variable(absolute_field(GENERIC_DESKTOP_X, 0..8, 255)),
            ReportField::Variable(absolute_field(DIGITIZER_CONTACT_COUNT, 8..16, 255)),
        ];
        let descriptor = descriptor_with_fields(fields, 16);

        processor.process_descriptor(descriptor).unwrap();

        let report_data = processor.input_reports.values().next().unwrap();
        let first_usage: u32 = report_data.relevant_fields[0].field.usage.into();
        assert_eq!(first_usage, DIGITIZER_CONTACT_COUNT);
    }

    #[test]
    fn process_descriptor_multiple_reports_without_ids_returns_error() {
        let mut processor = PointerProcessor::new();
        let descriptor = ReportDescriptor {
            input_reports: alloc::vec![
                hidparser::Report {
                    report_id: None,
                    size_in_bits: 8,
                    fields: alloc::vec![ReportField::Variable(absolute_field(GENERIC_DESKTOP_X, 0..8, 255))],
                },
                hidparser::Report {
                    report_id: None,
                    size_in_bits: 8,
                    fields: alloc::vec![ReportField::Variable(absolute_field(GENERIC_DESKTOP_Y, 0..8, 255))],
                },
            ],
            output_reports: alloc::vec![],
            bad_input_reports: alloc::vec![],
            bad_output_reports: alloc::vec![],
            features: alloc::vec![],
            bad_features: alloc::vec![],
        };
        assert_eq!(processor.process_descriptor(descriptor), Err(efi::Status::DEVICE_ERROR));
    }

    #[test]
    fn process_descriptor_ignores_array_fields() {
        let mut processor = PointerProcessor::new();
        let descriptor = ReportDescriptor {
            input_reports: alloc::vec![hidparser::Report {
                report_id: None,
                size_in_bits: 8,
                fields: alloc::vec![ReportField::Array(hidparser::ArrayField { bits: 0..8, ..Default::default() })],
            }],
            output_reports: alloc::vec![],
            bad_input_reports: alloc::vec![],
            bad_output_reports: alloc::vec![],
            features: alloc::vec![],
            bad_features: alloc::vec![],
        };
        // No relevant variable fields → UNSUPPORTED
        assert_eq!(processor.process_descriptor(descriptor), Err(efi::Status::UNSUPPORTED));
    }

    #[test]
    fn process_descriptor_with_button_and_switch_fields() {
        let mut processor = PointerProcessor::new();
        let fields = alloc::vec![
            ReportField::Variable(absolute_field(GENERIC_DESKTOP_X, 0..8, 255)),
            ReportField::Variable(absolute_field(BUTTON_MIN, 8..9, 1)),
            ReportField::Variable(absolute_field(DIGITIZER_SWITCH_MIN, 9..10, 1)),
        ];
        let descriptor = descriptor_with_fields(fields, 10);
        assert_eq!(processor.process_descriptor(descriptor), Ok(()));
        assert!(processor.supported_usages.contains(&Usage::from(BUTTON_MIN)));
        assert!(processor.supported_usages.contains(&Usage::from(DIGITIZER_SWITCH_MIN)));
    }

    // --- process_report tests ---

    #[test]
    fn process_report_empty_report_is_no_op() {
        let mut processor = PointerProcessor::new();
        let fields = alloc::vec![ReportField::Variable(absolute_field(GENERIC_DESKTOP_X, 0..8, 255))];
        let descriptor = descriptor_with_fields(fields, 8);
        processor.process_descriptor(descriptor).unwrap();

        let mut state = PointerState::new();
        processor.process_report(&[], &mut state);
        assert!(!state.state_changed);
    }

    #[test]
    fn process_report_updates_pointer_state() {
        let mut processor = PointerProcessor::new();
        let fields = alloc::vec![
            ReportField::Variable(absolute_field(GENERIC_DESKTOP_X, 0..8, 255)),
            ReportField::Variable(absolute_field(GENERIC_DESKTOP_Y, 8..16, 255)),
        ];
        let descriptor = descriptor_with_fields(fields, 16);
        processor.process_descriptor(descriptor).unwrap();

        let mut state = PointerState::new();
        processor.process_report(&[128, 64], &mut state);
        assert!(state.state_changed);
        assert_ne!(state.current_state.current_x, CENTER);
    }

    #[test]
    fn process_report_unregistered_report_id_is_ignored() {
        let report_id = hidparser::report_data_types::ReportId::from(&[0x01][..]);
        let mut processor = PointerProcessor::new();
        let descriptor = ReportDescriptor {
            input_reports: alloc::vec![hidparser::Report {
                report_id: Some(report_id),
                size_in_bits: 8,
                fields: alloc::vec![ReportField::Variable(absolute_field(GENERIC_DESKTOP_X, 0..8, 255))],
            }],
            output_reports: alloc::vec![],
            bad_input_reports: alloc::vec![],
            bad_output_reports: alloc::vec![],
            features: alloc::vec![],
            bad_features: alloc::vec![],
        };
        processor.process_descriptor(descriptor).unwrap();

        let mut state = PointerState::new();
        // Report with wrong report ID (0x02)
        processor.process_report(&[0x02, 128], &mut state);
        assert!(!state.state_changed);
    }

    #[test]
    fn process_report_report_length_mismatch_still_processes() {
        let mut processor = PointerProcessor::new();
        let fields = alloc::vec![ReportField::Variable(absolute_field(GENERIC_DESKTOP_X, 0..8, 255))];
        let descriptor = descriptor_with_fields(fields, 8);
        processor.process_descriptor(descriptor).unwrap();

        let mut state = PointerState::new();
        // Send 2 bytes when only 1 expected — should still process
        processor.process_report(&[128, 0], &mut state);
        assert!(state.state_changed);
    }

    #[test]
    fn process_report_contact_count_limits_axis_updates() {
        let mut processor = PointerProcessor::new();
        // Two X fields and a contact count field
        let fields = alloc::vec![
            ReportField::Variable(absolute_field(DIGITIZER_CONTACT_COUNT, 0..8, 255)),
            ReportField::Variable(absolute_field(GENERIC_DESKTOP_X, 8..16, 255)),
            ReportField::Variable(absolute_field(GENERIC_DESKTOP_X, 16..24, 255)),
        ];
        let descriptor = descriptor_with_fields(fields, 24);
        processor.process_descriptor(descriptor).unwrap();

        let mut state = PointerState::new();
        // Contact count = 1 → only first X field should be processed
        processor.process_report(&[1, 200, 50], &mut state);
        assert!(state.state_changed);
    }

    #[test]
    fn process_report_with_report_id_strips_id_byte() {
        let report_id = hidparser::report_data_types::ReportId::from(&[0x01][..]);
        let mut processor = PointerProcessor::new();
        let descriptor = ReportDescriptor {
            input_reports: alloc::vec![hidparser::Report {
                report_id: Some(report_id),
                size_in_bits: 8,
                fields: alloc::vec![ReportField::Variable(absolute_field(GENERIC_DESKTOP_X, 0..8, 255))],
            }],
            output_reports: alloc::vec![],
            bad_input_reports: alloc::vec![],
            bad_output_reports: alloc::vec![],
            features: alloc::vec![],
            bad_features: alloc::vec![],
        };
        processor.process_descriptor(descriptor).unwrap();

        let mut state = PointerState::new();
        // Report with correct report ID (0x01) + data
        processor.process_report(&[0x01, 200], &mut state);
        assert!(state.state_changed);
    }

    #[test]
    fn process_report_with_report_id_and_empty_data_is_no_op() {
        let report_id = hidparser::report_data_types::ReportId::from(&[0x01][..]);
        let mut processor = PointerProcessor::new();
        let descriptor = ReportDescriptor {
            input_reports: alloc::vec![hidparser::Report {
                report_id: Some(report_id),
                size_in_bits: 8,
                fields: alloc::vec![ReportField::Variable(absolute_field(GENERIC_DESKTOP_X, 0..8, 255))],
            }],
            output_reports: alloc::vec![],
            bad_input_reports: alloc::vec![],
            bad_output_reports: alloc::vec![],
            features: alloc::vec![],
            bad_features: alloc::vec![],
        };
        processor.process_descriptor(descriptor).unwrap();

        let mut state = PointerState::new();
        // Only report ID byte, no data
        processor.process_report(&[0x01], &mut state);
        assert!(!state.state_changed);
    }

    #[test]
    fn axis_handler_ignores_unknown_usage() {
        let mut state = PointerState::new();
        // Usage that's not X, Y, Z, or Wheel
        let field = absolute_field(0x00010099, 0..8, 255);
        state.axis_handler(&field, &[128]);
        assert!(!state.state_changed);
    }

    #[test]
    fn axis_handler_wheel_updates_z() {
        let mut state = PointerState::new();
        let field = relative_field(GENERIC_DESKTOP_WHEEL, 0..8, -127, 127);
        state.axis_handler(&field, &[5]);
        assert!(state.state_changed);
        assert_eq!(state.current_state.current_z, 5);
    }

    #[test]
    fn button_handler_digitizer_switch_sets_bit() {
        let mut state = PointerState::new();
        let field = absolute_field(DIGITIZER_SWITCH_MIN, 0..1, 1);
        state.button_handler(&field, &[0x01]);
        assert!(state.state_changed);
        assert_eq!(state.current_state.active_buttons, 1);
    }

    #[test]
    fn button_handler_ignores_unknown_usage() {
        let mut state = PointerState::new();
        let field = absolute_field(0x00990001, 0..1, 1);
        state.button_handler(&field, &[0x01]);
        assert!(!state.state_changed);
    }

    #[test]
    fn button_handler_shift_exceeding_u32_bits_is_ignored() {
        let mut state = PointerState::new();
        // BUTTON_MAX has a shift of BUTTON_MAX - BUTTON_MIN which could be large
        let field = absolute_field(BUTTON_MIN + 33, 0..1, 1);
        state.button_handler(&field, &[0x01]);
        assert!(!state.state_changed);
    }
}
