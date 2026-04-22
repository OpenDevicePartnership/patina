//! Provides Keyboard HID support.
//!
//! This module handles the core logic for processing keystrokes from HID
//! devices.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
//!
pub mod key_queue;
pub mod layout;
pub(crate) mod simple_text_in;
pub(crate) mod simple_text_in_ex;

use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet},
    vec,
    vec::Vec,
};
use core::ptr;

use r_efi::{efi, protocols};

use hidparser::{
    ArrayField, ReportDescriptor, ReportField, VariableField,
    report_data_types::{ReportId, Usage},
};

use patina::{
    boot_services::{BootServices, c_ptr::PtrMetadata, event::EventType, tpl::Tpl},
    tpl_mutex::TplMutex,
};

use crate::hid_io::{HidIo, HidReportReceiver};

#[cfg(feature = "ctrl-alt-del")]
use r_efi::protocols::simple_text_input_ex::{
    LEFT_ALT_PRESSED, LEFT_CONTROL_PRESSED, RIGHT_ALT_PRESSED, RIGHT_CONTROL_PRESSED, SHIFT_STATE_VALID,
};

use self::key_queue::OrdKeyData;

// usages supported by this module
const KEYBOARD_MODIFIER_USAGE_MIN: u32 = 0x000700E0;
const KEYBOARD_MODIFIER_USAGE_MAX: u32 = 0x000700E7;
const KEYBOARD_USAGE_MIN: u32 = 0x00070001;
const KEYBOARD_USAGE_MAX: u32 = 0x00070065;
const LED_USAGE_MIN: u32 = 0x00080001;
const LED_USAGE_MAX: u32 = 0x00080005;

// maps a given field to a routine that handles input from it.
struct KeyInputFieldHandler<F> {
    field: F,
    report_handler: fn(current_keys: &mut BTreeSet<Usage>, field: &F, report: &[u8]),
}

// maps a given field to a routine that builds output reports for it.
struct KeyOutputFieldBuilder {
    field: VariableField,
    field_builder: fn(led_state: &BTreeSet<Usage>, field: &VariableField, report: &mut [u8]),
}

// Defines an input report and the fields of interest in it.
#[derive(Default)]
struct KeyboardInputReportSpec {
    report_id: Option<ReportId>,
    report_size: usize,
    relevant_variable_fields: Vec<KeyInputFieldHandler<VariableField>>,
    relevant_array_fields: Vec<KeyInputFieldHandler<ArrayField>>,
}

// Defines an output report and the fields of interest in it.
#[derive(Default)]
struct KeyboardOutputReportSpec {
    report_id: Option<ReportId>,
    report_size: usize,
    relevant_variable_fields: Vec<KeyOutputFieldBuilder>,
}

// Result of processing a single HID report.
struct ProcessReportResult {
    should_signal_notify: bool,
    output_reports: Vec<(Option<ReportId>, Vec<u8>)>,
}

// Core keyboard data processing logic, independent of UEFI boot services.
struct KeyboardProcessor {
    input_reports: BTreeMap<Option<ReportId>, KeyboardInputReportSpec>,
    output_builders: Vec<KeyboardOutputReportSpec>,
    report_id_present: bool,
    last_keys: BTreeSet<Usage>,
    current_keys: BTreeSet<Usage>,
    led_state: BTreeSet<Usage>,
    notification_callbacks: BTreeMap<usize, (OrdKeyData, protocols::simple_text_input_ex::KeyNotifyFunction)>,
    next_notify_handle: usize,
}

impl KeyboardProcessor {
    // Creates a new processor with default state.
    fn new() -> Self {
        Self {
            input_reports: BTreeMap::new(),
            output_builders: Vec::new(),
            report_id_present: false,
            last_keys: BTreeSet::new(),
            current_keys: BTreeSet::new(),
            led_state: BTreeSet::new(),
            notification_callbacks: BTreeMap::new(),
            next_notify_handle: 0,
        }
    }

    // Parses a report descriptor and registers input/output field handlers.
    fn process_descriptor(&mut self, descriptor: ReportDescriptor) -> Result<(), efi::Status> {
        let multiple_reports =
            descriptor.input_reports.len() > 1 || descriptor.output_reports.len() > 1 || descriptor.features.len() > 1;

        for report in &descriptor.input_reports {
            let mut report_data_spec = KeyboardInputReportSpec { report_id: report.report_id, ..Default::default() };

            self.report_id_present = report.report_id.is_some();

            if multiple_reports && !self.report_id_present {
                return Err(efi::Status::DEVICE_ERROR);
            }

            report_data_spec.report_size = report.size_in_bits.div_ceil(8);

            for field in &report.fields {
                match field {
                    ReportField::Variable(field) => {
                        if let KEYBOARD_MODIFIER_USAGE_MIN..=KEYBOARD_MODIFIER_USAGE_MAX = field.usage.into() {
                            report_data_spec.relevant_variable_fields.push(KeyInputFieldHandler {
                                field: field.clone(),
                                report_handler: handle_variable_key,
                            });
                        }
                    }
                    ReportField::Array(field) => {
                        for usage_list in &field.usage_list {
                            if usage_list.contains(Usage::from(KEYBOARD_USAGE_MIN))
                                || usage_list.contains(Usage::from(KEYBOARD_USAGE_MAX))
                            {
                                report_data_spec.relevant_array_fields.push(KeyInputFieldHandler {
                                    field: field.clone(),
                                    report_handler: handle_array_key,
                                });
                                break;
                            }
                        }
                    }
                    ReportField::Padding(_) => (),
                }
            }
            if !(report_data_spec.relevant_variable_fields.is_empty()
                && report_data_spec.relevant_array_fields.is_empty())
            {
                self.input_reports.insert(report_data_spec.report_id, report_data_spec);
            }
        }

        for report in &descriptor.output_reports {
            let mut report_builder = KeyboardOutputReportSpec { report_id: report.report_id, ..Default::default() };

            self.report_id_present = report.report_id.is_some();

            if multiple_reports && !self.report_id_present {
                return Err(efi::Status::DEVICE_ERROR);
            }

            report_builder.report_size = usize::div_ceil(report.size_in_bits, 8);

            for field in &report.fields {
                match field {
                    ReportField::Variable(field) => {
                        if let LED_USAGE_MIN..=LED_USAGE_MAX = field.usage.into() {
                            report_builder
                                .relevant_variable_fields
                                .push(KeyOutputFieldBuilder { field: field.clone(), field_builder: build_led_report })
                        }
                    }
                    ReportField::Array(_) | ReportField::Padding(_) => (),
                }
            }
            if !report_builder.relevant_variable_fields.is_empty() {
                self.output_builders.push(report_builder);
            }
        }

        if self.input_reports.is_empty() && self.output_builders.is_empty() {
            log::trace!("process_descriptor: no relevant keyboard fields found");
            Err(efi::Status::UNSUPPORTED)
        } else {
            log::trace!(
                "process_descriptor: {:?} input report(s) with {:?} variable/{:?} array fields, {:?} output builder(s)",
                self.input_reports.len(),
                self.input_reports.values().map(|r| r.relevant_variable_fields.len()).sum::<usize>(),
                self.input_reports.values().map(|r| r.relevant_array_fields.len()).sum::<usize>(),
                self.output_builders.len(),
            );
            Ok(())
        }
    }

    // Resets key tracking state and the key queue.
    fn reset(&mut self, kq: &mut key_queue::KeyQueue, extended_verification: bool) {
        self.last_keys.clear();
        self.current_keys.clear();
        kq.reset(extended_verification);
        if extended_verification {
            self.led_state.clear();
        }
    }

    // Builds output reports reflecting the current LED state.
    fn build_led_output_reports(&self) -> Vec<(Option<ReportId>, Vec<u8>)> {
        let mut output_vec = Vec::new();
        for output_builder in &self.output_builders {
            let mut report_buffer = vec![0u8; output_builder.report_size];
            for field_builder in &output_builder.relevant_variable_fields {
                (field_builder.field_builder)(&self.led_state, &field_builder.field, report_buffer.as_mut_slice());
            }
            output_vec.push((output_builder.report_id, report_buffer));
        }
        output_vec
    }

    // Processes an incoming HID report, queuing keystrokes and building LED output.
    fn process_report(&mut self, report: &[u8], kq: &mut key_queue::KeyQueue) -> ProcessReportResult {
        let mut result = ProcessReportResult { should_signal_notify: false, output_reports: Vec::new() };
        if report.is_empty() {
            return result;
        }
        let (report_id, report) = match self.report_id_present {
            true => (Some(ReportId::from(&report[0..1])), &report[1..]),
            false => (None, &report[0..]),
        };

        if report.is_empty() {
            return result;
        }

        if let Some(report_data) = self.input_reports.get(&report_id) {
            if report.len() != report_data.report_size {
                log::trace!(
                    "receive_report: unexpected report length for report_id: {:?}. expected {:?}, actual {:?}",
                    report_id,
                    report_data.report_size,
                    report.len()
                );
            }

            self.current_keys.clear();

            for field in &report_data.relevant_variable_fields {
                (field.report_handler)(&mut self.current_keys, &field.field, report);
            }

            for field in &report_data.relevant_array_fields {
                (field.report_handler)(&mut self.current_keys, &field.field, report);
            }

            if self.last_keys != self.current_keys {
                let mut released_keys = Vec::new();
                let mut pressed_keys = Vec::new();
                // XOR the key sets to find keys that changed state between reports.
                for changed_key in (&self.last_keys ^ &self.current_keys).into_iter().rev() {
                    if self.last_keys.contains(&changed_key) {
                        released_keys.push(changed_key);
                    } else {
                        pressed_keys.push(changed_key);
                    }
                }

                log::trace!(
                    "process_report: {:?} key(s) released, {:?} key(s) pressed",
                    released_keys.len(),
                    pressed_keys.len(),
                );

                for key in released_keys {
                    kq.keystroke(key, key_queue::KeyAction::KeyUp);
                }
                for key in pressed_keys {
                    kq.keystroke(key, key_queue::KeyAction::KeyDown);
                }

                if kq.peek_notify_key().is_some() {
                    result.should_signal_notify = true;
                }

                // Only send LED output reports when LED state actually changes.
                let current_leds: BTreeSet<Usage> = kq.active_leds().iter().cloned().collect();
                if current_leds != self.led_state {
                    log::trace!("process_report: LED state changed, generating output reports");
                    self.led_state = current_leds;
                    result.output_reports = self.build_led_output_reports();
                }
            }

            core::mem::swap(&mut self.last_keys, &mut self.current_keys);
        }
        result
    }

    // Registers a key notification callback, returning its handle.
    fn insert_key_notify_callback(
        &mut self,
        key_data: protocols::simple_text_input_ex::KeyData,
        key_notification_function: protocols::simple_text_input_ex::KeyNotifyFunction,
        kq: &mut key_queue::KeyQueue,
    ) -> usize {
        let key_data = OrdKeyData(key_data);
        for (handle, entry) in &self.notification_callbacks {
            if entry.0 == key_data && ptr::fn_addr_eq(entry.1, key_notification_function) {
                return *handle;
            }
        }
        self.next_notify_handle += 1;
        self.notification_callbacks.insert(self.next_notify_handle, (key_data.clone(), key_notification_function));
        kq.add_notify_key(key_data);
        self.next_notify_handle
    }

    // Unregisters a key notification callback by handle.
    fn remove_key_notify_callback(
        &mut self,
        notification_handle: usize,
        kq: &mut key_queue::KeyQueue,
    ) -> Result<(), efi::Status> {
        if let Some(entry) = self.notification_callbacks.remove(&notification_handle) {
            let removed_key = entry.0;
            if !self.notification_callbacks.values().any(|(key, _)| *key == removed_key) {
                kq.remove_notify_key(&removed_key);
            }
            Ok(())
        } else {
            Err(efi::Status::INVALID_PARAMETER)
        }
    }

    // Returns the next pending notify key and its matching callbacks.
    fn pending_callbacks(
        &self,
        kq: &mut key_queue::KeyQueue,
    ) -> (Option<protocols::simple_text_input_ex::KeyData>, Vec<protocols::simple_text_input_ex::KeyNotifyFunction>)
    {
        if let Some(pending_notify_key) = kq.pop_notify_key() {
            let mut pending_callbacks = Vec::new();
            for (key, callback) in self.notification_callbacks.values() {
                if OrdKeyData(pending_notify_key).matches_registered_key(key) {
                    pending_callbacks.push(*callback);
                }
            }
            (Some(pending_notify_key), pending_callbacks)
        } else {
            (None, Vec::new())
        }
    }
}

// Context passed to the keyboard layout change event callback.
pub(crate) struct LayoutChangeContext<T: BootServices + Clone + 'static> {
    boot_services: &'static T,
    keyboard_handler: *mut KeyboardHidHandler<T>,
}

/// Keyboard HID handler that processes reports and produces UEFI SimpleTextIn keystrokes.
pub struct KeyboardHidHandler<T: BootServices + Clone + 'static> {
    boot_services: &'static T,
    controller: efi::Handle,
    hid_io: Option<*const dyn HidIo>,
    simple_text_in_key: Option<PtrMetadata<'static, Box<simple_text_in::SimpleTextInFfi<T>>>>,
    simple_text_in_ex_key: Option<PtrMetadata<'static, Box<simple_text_in_ex::SimpleTextInExFfi<T>>>>,
    processor: KeyboardProcessor,
    pub(crate) state: TplMutex<key_queue::KeyQueue, T>,
    pub(crate) key_notify_event: efi::Event,
    layout_change_event: efi::Event,
    layout_context: *mut LayoutChangeContext<T>,
}

impl<T: BootServices + Clone + 'static> KeyboardHidHandler<T> {
    /// Creates a fully initialized Keyboard HID handler for the given controller.
    ///
    /// Returns a boxed handler because protocol installation stores raw pointers to `self`.
    /// Boxing first ensures those pointers remain valid (the handler is never moved after boxing).
    pub fn new(
        boot_services: &'static T,
        controller: efi::Handle,
        hid_io: &dyn HidIo,
    ) -> Result<Box<Self>, efi::Status> {
        let mut processor = KeyboardProcessor::new();
        let descriptor = hid_io.get_report_descriptor()?;
        processor.process_descriptor(descriptor)?;

        let mut handler = Box::new(Self {
            boot_services,
            controller,
            // SAFETY: hid_io is valid for the device lifetime (backed by a BY_DRIVER protocol
            // reference in UefiHidIo). Transmute erases the borrow lifetime for raw pointer storage.
            hid_io: Some(unsafe {
                core::mem::transmute::<*const dyn HidIo, *const dyn HidIo>(hid_io as *const dyn HidIo)
            }),
            simple_text_in_key: None,
            simple_text_in_ex_key: None,
            processor,
            state: TplMutex::new((*boot_services).clone(), Tpl::NOTIFY, key_queue::KeyQueue::default()),
            key_notify_event: core::ptr::null_mut(),
            layout_change_event: core::ptr::null_mut(),
            layout_context: core::ptr::null_mut(),
        });

        handler.reset(true);
        handler.install_protocol_interfaces()?;
        handler.initialize_keyboard_layout()?;

        // Register Ctrl-Alt-Delete handler to reset the system. Register only for DEL scan code;
        // CTRL-ALT shift state is validated in the callback to handle any left/right combination.
        #[cfg(feature = "ctrl-alt-del")]
        {
            let reset_key_data = protocols::simple_text_input_ex::KeyData {
                key: protocols::simple_text_input::InputKey { scan_code: key_queue::SCAN_DELETE, unicode_char: 0 },
                key_state: protocols::simple_text_input_ex::KeyState { key_toggle_state: 0, key_shift_state: 0 },
            };
            handler.insert_key_notify_callback(reset_key_data, reset_notification_function);
        }

        Ok(handler)
    }

    // Installs SimpleTextIn and SimpleTextInEx protocol interfaces.
    fn install_protocol_interfaces(&mut self) -> Result<(), efi::Status> {
        let sti_key = simple_text_in::SimpleTextInFfi::install(self.boot_services, self.controller, self)?;
        let sti_ex_key = match simple_text_in_ex::SimpleTextInExFfi::install(self.boot_services, self.controller, self)
        {
            Ok(key) => key,
            Err(status) => {
                let _ = simple_text_in::SimpleTextInFfi::<T>::uninstall(self.boot_services, self.controller, sti_key);
                return Err(status);
            }
        };
        self.simple_text_in_key = Some(sti_key);
        self.simple_text_in_ex_key = Some(sti_ex_key);
        Ok(())
    }

    // Creates and registers the keyboard layout change event.
    fn install_layout_change_event(&mut self) -> Result<(), efi::Status> {
        let context = LayoutChangeContext { boot_services: self.boot_services, keyboard_handler: self as *mut Self };
        let context_ptr = Box::into_raw(Box::new(context));

        // SAFETY: context_ptr is valid from Box::into_raw and will remain valid for the event lifetime.
        let layout_change_event = unsafe {
            self.boot_services.create_event_ex_unchecked(
                EventType::NOTIFY_SIGNAL,
                Tpl::NOTIFY,
                Some(Self::on_layout_update),
                context_ptr,
                &protocols::hii_database::SET_KEYBOARD_LAYOUT_EVENT_GUID,
            )
        };

        match layout_change_event {
            Ok(event) => {
                self.layout_change_event = event;
                self.layout_context = context_ptr;
                Ok(())
            }
            Err(status) => {
                // SAFETY: context_ptr was created via Box::into_raw above and is being reclaimed on the error path.
                drop(unsafe { Box::from_raw(context_ptr) });
                Err(status)
            }
        }
    }

    // Closes the layout change event and frees its context.
    fn uninstall_layout_change_event(&mut self) -> Result<(), efi::Status> {
        if !self.layout_change_event.is_null() {
            let layout_change_event = self.layout_change_event;
            if let Err(status) = self.boot_services.close_event(layout_change_event) {
                log::error!("Failed to close layout_change_event event, status: {:x?}", status);
                // SAFETY: layout_context is valid while self exists; nulling the handler prevents stale callbacks.
                unsafe {
                    (*self.layout_context).keyboard_handler = ptr::null_mut();
                }
                return Err(status);
            }
            // SAFETY: layout_context was created via Box::into_raw during install_layout_change_event.
            drop(unsafe { Box::from_raw(self.layout_context) });
            self.layout_context = ptr::null_mut();
            self.layout_change_event = ptr::null_mut();
        }
        Ok(())
    }

    // Installs the default US keyboard layout via the HII database.
    fn install_default_layout(&mut self) -> Result<(), efi::Status> {
        // SAFETY: We locate the HII database protocol and call its methods per UEFI spec.
        let hii_database_protocol_ptr = unsafe {
            self.boot_services.locate_protocol_unchecked(&protocols::hii_database::PROTOCOL_GUID, ptr::null_mut())
        };

        let hii_database_protocol_ptr = match hii_database_protocol_ptr {
            Ok(ptr) => ptr as *mut protocols::hii_database::Protocol,
            Err(status) => {
                log::error!("keyboard::install_default_layout: Could not locate hii_database protocol: {:x?}", status);
                return Err(status);
            }
        };

        // SAFETY: Dereferencing the protocol pointer returned from locate_protocol; null handled by the else branch.
        let Some(hii_database_protocol) = (unsafe { hii_database_protocol_ptr.as_mut() }) else {
            log::error!("keyboard::install_default_layout: locate_protocol returned null pointer.");
            return Err(efi::Status::NOT_FOUND);
        };

        let mut hii_handle: r_efi::hii::Handle = ptr::null_mut();
        let status = (hii_database_protocol.new_package_list)(
            hii_database_protocol_ptr,
            layout::get_default_keyboard_pkg_list_buffer().as_ptr() as *const r_efi::hii::PackageListHeader,
            ptr::null_mut(),
            &mut hii_handle as *mut r_efi::hii::Handle,
        );

        if status.is_error() {
            log::error!("keyboard::install_default_layout: Failed to install keyboard layout package: {:x?}", status);
            return Err(status);
        }

        let status = (hii_database_protocol.set_keyboard_layout)(
            hii_database_protocol_ptr,
            &layout::DEFAULT_KEYBOARD_LAYOUT_GUID as *const efi::Guid as *mut efi::Guid,
        );
        if status.is_error() {
            log::error!("keyboard::install_default_layout: Failed to set keyboard layout: {:x?}", status);
            return Err(status);
        }

        Ok(())
    }

    // Initializes the keyboard layout from the HII database or installs a default.
    fn initialize_keyboard_layout(&mut self) -> Result<(), efi::Status> {
        log::trace!("initialize_keyboard_layout: setting up keyboard layout");
        self.install_layout_change_event()?;

        // fake signal event to pick up any existing layout
        Self::on_layout_update(self.layout_change_event, self.layout_context);

        // install a default layout if no layout is installed.
        // Note: the guard must be dropped before install_default_layout to avoid
        // re-entrant TplMutex acquisition if a notification fires during installation.
        let needs_default = self.state.lock().layout().is_none();
        if needs_default {
            log::trace!("initialize_keyboard_layout: no existing layout found, installing default");
            self.install_default_layout()?;
        }
        Ok(())
    }

    // Sends HID output reports to the device.
    pub(crate) fn send_output_reports(
        &mut self,
        hid_io: &dyn HidIo,
        output_reports: Vec<(Option<ReportId>, Vec<u8>)>,
    ) -> Result<(), efi::Status> {
        if !output_reports.is_empty() {
            log::trace!("send_output_reports: sending {:?} report(s)", output_reports.len());
        }
        for (id, output_report) in output_reports {
            let result = hid_io.set_output_report(id.map(|x| u32::from(x) as u8), &output_report);
            if let Err(result) = result {
                log::error!("send_output_reports: unexpected error sending output report: {:?}", result);
                return Err(result);
            }
        }
        Ok(())
    }

    /// Resets the keyboard driver state.
    pub fn reset(&mut self, extended_verification: bool) {
        let mut kq = self.state.lock();
        self.processor.reset(&mut kq, extended_verification);
    }

    /// Returns a clone of the keystroke at the front of the keystroke queue.
    pub fn peek_key(&self) -> Option<protocols::simple_text_input_ex::KeyData> {
        self.state.lock().peek_key()
    }

    /// Removes and returns the keystroke at the front of the keystroke queue.
    pub fn pop_key(&self) -> Option<protocols::simple_text_input_ex::KeyData> {
        self.state.lock().pop_key()
    }

    /// Returns the current key state (i.e. the SHIFT and TOGGLE state).
    pub fn get_key_state(&self) -> protocols::simple_text_input_ex::KeyState {
        self.state.lock().init_key_state()
    }

    /// Sets the keyboard toggle state and sends updated LED reports to the device.
    pub fn set_key_toggle_state(&mut self, toggle_state: u8) {
        let current_leds: BTreeSet<Usage> = {
            let mut kq = self.state.lock();
            kq.set_key_toggle_state(toggle_state);
            kq.active_leds().iter().cloned().collect()
        };
        if current_leds != self.processor.led_state {
            self.processor.led_state = current_leds;
            let output_reports = self.processor.build_led_output_reports();
            if let Some(hid_io_ptr) = self.hid_io {
                // SAFETY: hid_io is valid for the device lifetime, set during construction.
                let hid_io = unsafe { &*hid_io_ptr };
                if let Err(e) = self.send_output_reports(hid_io, output_reports) {
                    log::error!("set_key_toggle_state: failed to send LED reports: {:?}", e);
                }
            }
        }
    }

    /// Registers a new key notify callback function.
    pub fn insert_key_notify_callback(
        &mut self,
        key_data: protocols::simple_text_input_ex::KeyData,
        key_notification_function: protocols::simple_text_input_ex::KeyNotifyFunction,
    ) -> usize {
        let mut kq = self.state.lock();
        self.processor.insert_key_notify_callback(key_data, key_notification_function, &mut kq)
    }

    /// Unregisters a previously registered key notify callback function.
    pub fn remove_key_notify_callback(&mut self, notification_handle: usize) -> Result<(), efi::Status> {
        let mut kq = self.state.lock();
        self.processor.remove_key_notify_callback(notification_handle, &mut kq)
    }

    /// Returns the set of keys that have pending callbacks.
    pub fn pending_callbacks(
        &mut self,
    ) -> (Option<protocols::simple_text_input_ex::KeyData>, Vec<protocols::simple_text_input_ex::KeyNotifyFunction>)
    {
        let mut kq = self.state.lock();
        self.processor.pending_callbacks(&mut kq)
    }

    /// Returns the controller associated with this KeyboardHidHandler.
    pub fn controller(&self) -> efi::Handle {
        self.controller
    }

    #[cfg(test)]
    pub fn new_for_test(boot_services: &'static T) -> Self {
        Self {
            boot_services,
            controller: core::ptr::null_mut(),
            hid_io: None,
            simple_text_in_key: None,
            simple_text_in_ex_key: None,
            processor: KeyboardProcessor::new(),
            state: TplMutex::new((*boot_services).clone(), Tpl::NOTIFY, key_queue::KeyQueue::default()),
            key_notify_event: core::ptr::null_mut(),
            layout_change_event: core::ptr::null_mut(),
            layout_context: core::ptr::null_mut(),
        }
    }

    #[cfg(test)]
    pub fn set_layout(&mut self, layout: Option<layout::HiiKeyboardLayout>) {
        self.state.lock().set_layout(layout)
    }

    #[cfg(test)]
    pub fn set_notify_event(&mut self, event: efi::Event) {
        self.key_notify_event = event;
    }

    // Handles keyboard layout change events from the HII database.
    extern "efiapi" fn on_layout_update(_event: efi::Event, context: *mut LayoutChangeContext<T>) {
        log::trace!("on_layout_update: keyboard layout change event received");
        // SAFETY: context was set during event registration via Box::into_raw and remains valid for the event lifetime.
        let context = unsafe { context.as_mut() }.expect("bad context pointer");

        if context.keyboard_handler.is_null() {
            log::error!("on_layout_update invoked with invalid handler");
            return;
        }

        // SAFETY: keyboard_handler is null-checked above.
        let keyboard_handler = unsafe { context.keyboard_handler.as_mut() }.expect("bad keyboard handler");

        // SAFETY: We locate the HII database protocol per UEFI spec.
        let hii_database_protocol_ptr = unsafe {
            context.boot_services.locate_protocol_unchecked(&protocols::hii_database::PROTOCOL_GUID, ptr::null_mut())
        };

        let Ok(hii_database_protocol_ptr) =
            hii_database_protocol_ptr.map(|p| p as *mut protocols::hii_database::Protocol)
        else {
            return;
        };

        // SAFETY: Dereferencing the protocol pointer returned from locate_protocol; null handled by the else branch.
        let Some(hii_database_protocol) = (unsafe { hii_database_protocol_ptr.as_mut() }) else {
            log::error!("on_layout_update: locate_protocol returned null pointer.");
            return;
        };

        // retrieve keyboard layout size
        let mut layout_buffer_len: u16 = 0;
        match (hii_database_protocol.get_keyboard_layout)(
            hii_database_protocol_ptr,
            ptr::null_mut(),
            &mut layout_buffer_len as *mut u16,
            ptr::null_mut(),
        ) {
            efi::Status::NOT_FOUND => return,
            status if status != efi::Status::BUFFER_TOO_SMALL => {
                log::error!(
                    "on_layout_update: unexpected return from get_keyboard_layout when determining length: {:x?}",
                    status
                );
                return;
            }
            _ => (),
        }

        let mut keyboard_layout_buffer = vec![0u8; layout_buffer_len as usize];
        let status = (hii_database_protocol.get_keyboard_layout)(
            hii_database_protocol_ptr,
            ptr::null_mut(),
            &mut layout_buffer_len as *mut u16,
            keyboard_layout_buffer.as_mut_ptr() as *mut protocols::hii_database::KeyboardLayout<0>,
        );

        if status.is_error() {
            log::error!("Unexpected return from get_keyboard_layout: {:x?}", status);
            return;
        }

        match layout::keyboard_layout_from_buffer(&keyboard_layout_buffer) {
            Ok(keyboard_layout) => {
                log::trace!("on_layout_update: successfully parsed layout with {:?} keys", keyboard_layout.keys.len());
                keyboard_handler.state.lock().set_layout(Some(keyboard_layout));
            }
            Err(_) => {
                log::error!("keyboard::on_layout_update: Could not parse keyboard layout buffer.");
            }
        }
    }
}

// Inserts the field's usage into the active key set if the field value is nonzero.
fn handle_variable_key(current_keys: &mut BTreeSet<Usage>, field: &VariableField, report: &[u8]) {
    match field.field_value(report) {
        Some(x) if x != 0 => _ = current_keys.insert(field.usage),
        _ => (),
    }
}

// Resolves an array field value to a usage and inserts it into the active key set.
fn handle_array_key(current_keys: &mut BTreeSet<Usage>, field: &ArrayField, report: &[u8]) {
    match field.field_value(report) {
        Some(index) if index != 0 => {
            let mut index = (index as u32 - u32::from(field.logical_minimum)) as usize;
            let usage = field.usage_list.iter().find_map(|x| {
                let range_size = (x.end() - x.start()) as usize;
                if index <= range_size {
                    x.range().nth(index)
                } else {
                    index -= range_size;
                    None
                }
            });
            if let Some(usage) = usage {
                current_keys.insert(Usage::from(usage));
            }
        }
        _ => (),
    }
}

// Sets a single LED field in the output report buffer.
fn build_led_report(led_state: &BTreeSet<Usage>, field: &VariableField, report: &mut [u8]) {
    let status = field.set_field_value(led_state.contains(&field.usage).into(), report);
    if status.is_err() {
        log::warn!("build_led_report: failed to set field value: {:?}", status);
    }
}

// Notification function called when Ctrl-Alt-Delete is pressed.
// Any DEL key press triggers this callback; CTRL-ALT qualification is checked here to handle
// arbitrary left/right modifier combinations without needing separate registrations.
#[cfg(feature = "ctrl-alt-del")]
extern "efiapi" fn reset_notification_function(key_data: *mut protocols::simple_text_input_ex::KeyData) -> efi::Status {
    if key_data.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: null-checked above, using read_unaligned to avoid any alignment issues.
    let key_data = unsafe { key_data.read_unaligned() };
    if key_data.key.scan_code != key_queue::SCAN_DELETE {
        return efi::Status::SUCCESS;
    }

    // Check that DEL is qualified with valid CTRL-ALT state.
    if key_data.key_state.key_shift_state & SHIFT_STATE_VALID == 0
        || key_data.key_state.key_shift_state & (LEFT_CONTROL_PRESSED | RIGHT_CONTROL_PRESSED) == 0
        || key_data.key_state.key_shift_state & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) == 0
    {
        return efi::Status::SUCCESS;
    }

    log::warn!("Ctrl-Alt-Del pressed, resetting system.");
    let rt_ptr = crate::RUNTIME_SERVICES.load(core::sync::atomic::Ordering::SeqCst);
    // SAFETY: rt_ptr is loaded from a global atomic; null is handled by the if-let.
    if let Some(runtime_services) = unsafe { rt_ptr.as_ref() } {
        (runtime_services.reset_system)(efi::RESET_COLD, efi::Status::SUCCESS, 0, core::ptr::null_mut());
    }
    // reset_system should not return; if it does, there is nothing useful to do.
    efi::Status::SUCCESS
}

impl<T: BootServices + Clone + 'static> HidReportReceiver for KeyboardHidHandler<T> {
    fn receive_report(&mut self, report: &[u8], hid_io: &dyn HidIo) {
        log::trace!("keyboard::receive_report: {:?} bytes", report.len());
        let result = {
            let mut kq = self.state.lock();
            self.processor.process_report(report, &mut kq)
        };
        if result.should_signal_notify {
            let _ = self.boot_services.signal_event(self.key_notify_event);
        }
        if let Err(e) = self.send_output_reports(hid_io, result.output_reports) {
            log::error!("unexpected error sending output report: {:?}", e);
        }
    }
}

impl<T: BootServices + Clone + 'static> Drop for KeyboardHidHandler<T> {
    fn drop(&mut self) {
        if let Some(key) = self.simple_text_in_key.take()
            && let Err(status) =
                simple_text_in::SimpleTextInFfi::<T>::uninstall(self.boot_services, self.controller, key)
        {
            log::error!("KeyboardHidHandler::drop: Failed to uninstall simple_text_in: {:?}", status);
        }
        if let Some(key) = self.simple_text_in_ex_key.take()
            && let Err(status) =
                simple_text_in_ex::SimpleTextInExFfi::<T>::uninstall(self.boot_services, self.controller, key)
        {
            log::error!("KeyboardHidHandler::drop: Failed to uninstall simple_text_in_ex: {:?}", status);
        }
        if let Err(status) = self.uninstall_layout_change_event() {
            log::error!("KeyboardHidHandler::drop: Failed to close layout_change_event: {:?}", status);
        }
    }
}

#[cfg(test)]
mod test {
    use alloc::vec;

    use hidparser::{
        ReportDescriptor, ReportField, VariableField,
        report_data_types::{ReportAttributes, Usage},
    };
    use r_efi::{efi, protocols};

    use super::*;

    fn modifier_field(usage: u32, bit: u32) -> VariableField {
        VariableField {
            bits: bit..bit + 1,
            usage: Usage::from(usage),
            logical_minimum: 0.into(),
            logical_maximum: 1.into(),
            attributes: ReportAttributes::default(),
            ..Default::default()
        }
    }

    fn led_field(usage: u32, bit: u32) -> VariableField {
        VariableField {
            bits: bit..bit + 1,
            usage: Usage::from(usage),
            logical_minimum: 0.into(),
            logical_maximum: 1.into(),
            attributes: ReportAttributes::default(),
            ..Default::default()
        }
    }

    fn modifier_only_descriptor() -> ReportDescriptor {
        ReportDescriptor {
            input_reports: vec![hidparser::Report {
                report_id: None,
                size_in_bits: 8,
                fields: vec![
                    ReportField::Variable(modifier_field(KEYBOARD_MODIFIER_USAGE_MIN, 0)),
                    ReportField::Variable(modifier_field(KEYBOARD_MODIFIER_USAGE_MIN + 1, 1)),
                ],
            }],
            output_reports: vec![],
            bad_input_reports: vec![],
            bad_output_reports: vec![],
            features: vec![],
            bad_features: vec![],
        }
    }

    fn keyboard_with_leds_descriptor() -> ReportDescriptor {
        ReportDescriptor {
            input_reports: vec![hidparser::Report {
                report_id: None,
                size_in_bits: 8,
                fields: vec![ReportField::Variable(modifier_field(KEYBOARD_MODIFIER_USAGE_MIN, 0))],
            }],
            output_reports: vec![hidparser::Report {
                report_id: None,
                size_in_bits: 8,
                fields: vec![
                    ReportField::Variable(led_field(LED_USAGE_MIN, 0)),
                    ReportField::Variable(led_field(LED_USAGE_MIN + 1, 1)),
                    ReportField::Variable(led_field(LED_USAGE_MIN + 2, 2)),
                ],
            }],
            bad_input_reports: vec![],
            bad_output_reports: vec![],
            features: vec![],
            bad_features: vec![],
        }
    }

    fn empty_descriptor() -> ReportDescriptor {
        ReportDescriptor {
            input_reports: vec![hidparser::Report { report_id: None, size_in_bits: 0, fields: vec![] }],
            output_reports: vec![],
            bad_input_reports: vec![],
            bad_output_reports: vec![],
            features: vec![],
            bad_features: vec![],
        }
    }

    fn led_only_descriptor() -> ReportDescriptor {
        ReportDescriptor {
            input_reports: vec![],
            output_reports: vec![hidparser::Report {
                report_id: None,
                size_in_bits: 8,
                fields: vec![ReportField::Variable(led_field(LED_USAGE_MIN, 0))],
            }],
            bad_input_reports: vec![],
            bad_output_reports: vec![],
            features: vec![],
            bad_features: vec![],
        }
    }

    // --- processor defaults ---

    #[test]
    fn new_processor_has_defaults() {
        let processor = KeyboardProcessor::new();
        assert!(processor.input_reports.is_empty());
        assert!(processor.output_builders.is_empty());
        assert!(processor.last_keys.is_empty());
        assert!(processor.current_keys.is_empty());
        assert!(processor.led_state.is_empty());
        assert!(processor.notification_callbacks.is_empty());
        assert_eq!(processor.next_notify_handle, 0);
    }

    // --- process_descriptor ---

    #[test]
    fn process_descriptor_with_modifier_fields_succeeds() {
        let mut processor = KeyboardProcessor::new();
        assert_eq!(processor.process_descriptor(modifier_only_descriptor()), Ok(()));
        assert_eq!(processor.input_reports.len(), 1);
        let report_data = processor.input_reports.values().next().unwrap();
        assert_eq!(report_data.relevant_variable_fields.len(), 2);
        assert!(report_data.relevant_array_fields.is_empty());
    }

    #[test]
    fn process_descriptor_with_leds_creates_output_builders() {
        let mut processor = KeyboardProcessor::new();
        assert_eq!(processor.process_descriptor(keyboard_with_leds_descriptor()), Ok(()));
        assert_eq!(processor.input_reports.len(), 1);
        assert_eq!(processor.output_builders.len(), 1);
        assert_eq!(processor.output_builders[0].relevant_variable_fields.len(), 3);
    }

    #[test]
    fn process_descriptor_with_no_relevant_fields_returns_unsupported() {
        let mut processor = KeyboardProcessor::new();
        assert_eq!(processor.process_descriptor(empty_descriptor()), Err(efi::Status::UNSUPPORTED));
    }

    #[test]
    fn process_descriptor_with_only_led_output_fields_succeeds() {
        let mut processor = KeyboardProcessor::new();
        assert_eq!(processor.process_descriptor(led_only_descriptor()), Ok(()));
        assert!(processor.input_reports.is_empty());
        assert_eq!(processor.output_builders.len(), 1);
    }

    // --- reset ---

    #[test]
    fn reset_clears_keys_and_state() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        processor.last_keys.insert(Usage::from(0x00070004));
        processor.current_keys.insert(Usage::from(0x00070005));
        processor.led_state.insert(Usage::from(LED_USAGE_MIN));

        processor.reset(&mut kq, true);

        assert!(processor.last_keys.is_empty());
        assert!(processor.current_keys.is_empty());
        assert!(processor.led_state.is_empty());
    }

    #[test]
    fn reset_non_extended_preserves_led_state() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        processor.led_state.insert(Usage::from(LED_USAGE_MIN));

        processor.reset(&mut kq, false);

        assert!(!processor.led_state.is_empty());
    }

    // --- key queue ---

    #[test]
    fn peek_key_returns_none_on_empty_queue() {
        let kq = key_queue::KeyQueue::default();
        assert!(kq.peek_key().is_none());
    }

    #[test]
    fn pop_key_returns_none_on_empty_queue() {
        let mut kq = key_queue::KeyQueue::default();
        assert!(kq.pop_key().is_none());
    }

    #[test]
    fn get_key_state_returns_initial_state() {
        let kq = key_queue::KeyQueue::default();
        let state = kq.init_key_state();
        assert_eq!(state.key_shift_state, protocols::simple_text_input_ex::SHIFT_STATE_VALID);
        assert_eq!(state.key_toggle_state, protocols::simple_text_input_ex::TOGGLE_STATE_VALID);
    }

    #[test]
    fn set_key_toggle_state_updates_state() {
        let mut kq = key_queue::KeyQueue::default();
        let toggle =
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::CAPS_LOCK_ACTIVE;
        kq.set_key_toggle_state(toggle);
        let state = kq.init_key_state();
        assert_ne!(state.key_toggle_state & protocols::simple_text_input_ex::CAPS_LOCK_ACTIVE, 0);
    }

    // --- key notify callbacks ---

    extern "efiapi" fn dummy_callback(_key: *mut protocols::simple_text_input_ex::KeyData) -> efi::Status {
        efi::Status::SUCCESS
    }

    #[test]
    fn insert_key_notify_returns_handle() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        let key_data: protocols::simple_text_input_ex::KeyData = Default::default();
        let handle = processor.insert_key_notify_callback(key_data, dummy_callback, &mut kq);
        assert!(handle > 0);
        assert_eq!(processor.notification_callbacks.len(), 1);
    }

    #[test]
    fn insert_duplicate_notify_returns_same_handle() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        let key_data: protocols::simple_text_input_ex::KeyData = Default::default();
        let handle1 = processor.insert_key_notify_callback(key_data, dummy_callback, &mut kq);
        let handle2 = processor.insert_key_notify_callback(key_data, dummy_callback, &mut kq);
        assert_eq!(handle1, handle2);
        assert_eq!(processor.notification_callbacks.len(), 1);
    }

    #[test]
    fn remove_key_notify_succeeds() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        let key_data: protocols::simple_text_input_ex::KeyData = Default::default();
        let handle = processor.insert_key_notify_callback(key_data, dummy_callback, &mut kq);
        assert_eq!(processor.remove_key_notify_callback(handle, &mut kq), Ok(()));
        assert!(processor.notification_callbacks.is_empty());
    }

    #[test]
    fn remove_key_notify_invalid_returns_error() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        assert_eq!(processor.remove_key_notify_callback(42, &mut kq), Err(efi::Status::INVALID_PARAMETER));
    }

    #[test]
    fn pending_callbacks_returns_none_when_empty() {
        let processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        let (key, callbacks) = processor.pending_callbacks(&mut kq);
        assert!(key.is_none());
        assert!(callbacks.is_empty());
    }

    // --- LED reports ---

    #[test]
    fn build_led_output_reports_reflects_led_state() {
        let mut processor = KeyboardProcessor::new();
        processor.process_descriptor(keyboard_with_leds_descriptor()).unwrap();

        // Set num_lock LED active
        processor.led_state.insert(Usage::from(LED_USAGE_MIN));

        let reports = processor.build_led_output_reports();
        assert_eq!(reports.len(), 1);
        let (id, report) = &reports[0];
        assert!(id.is_none());
        assert_eq!(report.len(), 1);
        assert_eq!(report[0] & 0x01, 0x01); // bit 0 = num lock
    }

    #[test]
    fn build_led_output_reports_empty_without_descriptor() {
        let processor = KeyboardProcessor::new();
        let reports = processor.build_led_output_reports();
        assert!(reports.is_empty());
    }

    #[test]
    fn process_report_skips_led_output_when_led_state_unchanged() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        processor.process_descriptor(keyboard_with_leds_descriptor()).unwrap();

        // Press modifier (key state changes, but LED state stays empty).
        let result = processor.process_report(&[0x01], &mut kq);
        assert!(result.output_reports.is_empty());

        // Release modifier (key state changes again, LED state still empty).
        let result = processor.process_report(&[0x00], &mut kq);
        assert!(result.output_reports.is_empty());
    }

    #[test]
    fn process_report_sends_led_output_when_led_state_changes() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        processor.process_descriptor(keyboard_with_leds_descriptor()).unwrap();

        // Simulate a prior LED state (e.g. num lock was on).
        processor.led_state.insert(Usage::from(LED_USAGE_MIN));

        // Press modifier — key state changes and kq reports no active LEDs,
        // which differs from the pre-set led_state, so output reports are sent.
        let result = processor.process_report(&[0x01], &mut kq);
        assert!(!result.output_reports.is_empty());

        // LED state is now synchronized; same key change should not resend.
        let result = processor.process_report(&[0x00], &mut kq);
        assert!(result.output_reports.is_empty());
    }

    // --- process_descriptor edge cases ---

    #[test]
    fn process_descriptor_multiple_input_reports_without_ids_returns_error() {
        let mut processor = KeyboardProcessor::new();
        let descriptor = ReportDescriptor {
            input_reports: vec![
                hidparser::Report {
                    report_id: None,
                    size_in_bits: 8,
                    fields: vec![ReportField::Variable(modifier_field(KEYBOARD_MODIFIER_USAGE_MIN, 0))],
                },
                hidparser::Report {
                    report_id: None,
                    size_in_bits: 8,
                    fields: vec![ReportField::Variable(modifier_field(KEYBOARD_MODIFIER_USAGE_MIN + 1, 0))],
                },
            ],
            output_reports: vec![],
            bad_input_reports: vec![],
            bad_output_reports: vec![],
            features: vec![],
            bad_features: vec![],
        };
        assert_eq!(processor.process_descriptor(descriptor), Err(efi::Status::DEVICE_ERROR));
    }

    #[test]
    fn process_descriptor_multiple_output_reports_without_ids_returns_error() {
        let mut processor = KeyboardProcessor::new();
        let descriptor = ReportDescriptor {
            input_reports: vec![hidparser::Report {
                report_id: None,
                size_in_bits: 8,
                fields: vec![ReportField::Variable(modifier_field(KEYBOARD_MODIFIER_USAGE_MIN, 0))],
            }],
            output_reports: vec![
                hidparser::Report {
                    report_id: None,
                    size_in_bits: 8,
                    fields: vec![ReportField::Variable(led_field(LED_USAGE_MIN, 0))],
                },
                hidparser::Report {
                    report_id: None,
                    size_in_bits: 8,
                    fields: vec![ReportField::Variable(led_field(LED_USAGE_MIN + 1, 0))],
                },
            ],
            bad_input_reports: vec![],
            bad_output_reports: vec![],
            features: vec![],
            bad_features: vec![],
        };
        assert_eq!(processor.process_descriptor(descriptor), Err(efi::Status::DEVICE_ERROR));
    }

    #[test]
    fn process_descriptor_with_array_fields_succeeds() {
        let mut processor = KeyboardProcessor::new();
        let descriptor = ReportDescriptor {
            input_reports: vec![hidparser::Report {
                report_id: None,
                size_in_bits: 8,
                fields: vec![ReportField::Array(hidparser::ArrayField {
                    bits: 0..8,
                    usage_list: vec![hidparser::report_data_types::UsageRange::from(
                        KEYBOARD_USAGE_MIN..=KEYBOARD_USAGE_MAX,
                    )],
                    logical_minimum: 0.into(),
                    logical_maximum: 0x65.into(),
                    ..Default::default()
                })],
            }],
            output_reports: vec![],
            bad_input_reports: vec![],
            bad_output_reports: vec![],
            features: vec![],
            bad_features: vec![],
        };
        assert_eq!(processor.process_descriptor(descriptor), Ok(()));
        assert_eq!(processor.input_reports.len(), 1);
        let report_data = processor.input_reports.values().next().unwrap();
        assert_eq!(report_data.relevant_array_fields.len(), 1);
    }

    // --- process_report edge cases ---

    #[test]
    fn process_report_empty_returns_no_output() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        processor.process_descriptor(modifier_only_descriptor()).unwrap();
        let result = processor.process_report(&[], &mut kq);
        assert!(result.output_reports.is_empty());
        assert!(!result.should_signal_notify);
    }

    #[test]
    fn process_report_unregistered_report_id_is_ignored() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        let report_id = hidparser::report_data_types::ReportId::from(&[0x01][..]);
        let descriptor = ReportDescriptor {
            input_reports: vec![hidparser::Report {
                report_id: Some(report_id),
                size_in_bits: 8,
                fields: vec![ReportField::Variable(modifier_field(KEYBOARD_MODIFIER_USAGE_MIN, 0))],
            }],
            output_reports: vec![],
            bad_input_reports: vec![],
            bad_output_reports: vec![],
            features: vec![],
            bad_features: vec![],
        };
        processor.process_descriptor(descriptor).unwrap();
        // Send a report with a different report ID (0x02)
        let result = processor.process_report(&[0x02, 0x00], &mut kq);
        assert!(result.output_reports.is_empty());
    }

    #[test]
    fn process_report_key_press_and_release_queues_keystrokes() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        kq.set_layout(Some(crate::keyboard::layout::get_default_keyboard_layout()));
        processor.process_descriptor(modifier_only_descriptor()).unwrap();

        // Press left ctrl (bit 0)
        processor.process_report(&[0x01], &mut kq);
        // Release left ctrl
        processor.process_report(&[0x00], &mut kq);

        // Key state should reflect ctrl was pressed then released.
        let state = kq.init_key_state();
        assert_eq!(state.key_shift_state, protocols::simple_text_input_ex::SHIFT_STATE_VALID);
    }

    #[test]
    fn process_report_same_report_twice_does_not_re_queue() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        kq.set_layout(Some(crate::keyboard::layout::get_default_keyboard_layout()));
        processor.process_descriptor(modifier_only_descriptor()).unwrap();

        // Press key
        processor.process_report(&[0x01], &mut kq);
        // Same report again — no change in state
        let result = processor.process_report(&[0x01], &mut kq);
        assert!(result.output_reports.is_empty());
        assert!(!result.should_signal_notify);
    }

    #[test]
    fn process_report_with_notify_key_sets_signal() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        kq.set_layout(Some(crate::keyboard::layout::get_default_keyboard_layout()));
        processor.process_descriptor(modifier_only_descriptor()).unwrap();

        // Enable partial key support so modifier-only keys get enqueued
        kq.set_key_toggle_state(
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::KEY_STATE_EXPOSED,
        );

        // Register a notification for left ctrl
        let mut reg_key: protocols::simple_text_input_ex::KeyData = Default::default();
        reg_key.key_state.key_shift_state =
            protocols::simple_text_input_ex::SHIFT_STATE_VALID | protocols::simple_text_input_ex::LEFT_CONTROL_PRESSED;
        processor.insert_key_notify_callback(reg_key, dummy_callback, &mut kq);

        // Press left ctrl
        let result = processor.process_report(&[0x01], &mut kq);
        assert!(result.should_signal_notify);
    }

    // --- pending_callbacks ---

    #[test]
    fn pending_callbacks_returns_matching_callbacks() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        kq.set_layout(Some(crate::keyboard::layout::get_default_keyboard_layout()));
        processor.process_descriptor(modifier_only_descriptor()).unwrap();

        kq.set_key_toggle_state(
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::KEY_STATE_EXPOSED,
        );

        let mut reg_key: protocols::simple_text_input_ex::KeyData = Default::default();
        reg_key.key_state.key_shift_state =
            protocols::simple_text_input_ex::SHIFT_STATE_VALID | protocols::simple_text_input_ex::LEFT_CONTROL_PRESSED;
        processor.insert_key_notify_callback(reg_key, dummy_callback, &mut kq);

        // Press left ctrl
        processor.process_report(&[0x01], &mut kq);

        let (key, callbacks) = processor.pending_callbacks(&mut kq);
        assert!(key.is_some());
        assert_eq!(callbacks.len(), 1);
    }

    // --- remove_key_notify with shared key ---

    #[test]
    fn remove_key_notify_keeps_key_if_other_callback_remains() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        let key_data: protocols::simple_text_input_ex::KeyData = Default::default();
        let handle1 = processor.insert_key_notify_callback(key_data, dummy_callback, &mut kq);
        let handle2 = processor.insert_key_notify_callback(key_data, dummy_callback2, &mut kq);
        assert_ne!(handle1, handle2);
        // Remove first callback — key should remain since second still exists
        assert_eq!(processor.remove_key_notify_callback(handle1, &mut kq), Ok(()));
        assert_eq!(processor.notification_callbacks.len(), 1);
    }

    extern "efiapi" fn dummy_callback2(_key: *mut protocols::simple_text_input_ex::KeyData) -> efi::Status {
        efi::Status::SUCCESS
    }

    // --- handle_variable_key tests ---

    #[test]
    fn handle_variable_key_inserts_usage_when_nonzero() {
        let mut keys = BTreeSet::new();
        let field = modifier_field(KEYBOARD_MODIFIER_USAGE_MIN, 0);
        // Byte 0 bit 0 set → nonzero → usage inserted
        handle_variable_key(&mut keys, &field, &[0x01]);
        assert!(keys.contains(&Usage::from(KEYBOARD_MODIFIER_USAGE_MIN)));
    }

    #[test]
    fn handle_variable_key_does_nothing_when_zero() {
        let mut keys = BTreeSet::new();
        let field = modifier_field(KEYBOARD_MODIFIER_USAGE_MIN, 0);
        handle_variable_key(&mut keys, &field, &[0x00]);
        assert!(keys.is_empty());
    }

    // --- handle_array_key tests ---

    #[test]
    fn handle_array_key_inserts_usage_for_valid_index() {
        let mut keys = BTreeSet::new();
        let field = hidparser::ArrayField {
            bits: 0..8,
            usage_list: vec![hidparser::report_data_types::UsageRange::from(KEYBOARD_USAGE_MIN..=KEYBOARD_USAGE_MAX)],
            logical_minimum: 0.into(),
            logical_maximum: 0x65.into(),
            ..Default::default()
        };
        // Report value 4 → index 4 from logical_minimum 0 → usage KEYBOARD_USAGE_MIN + 4
        handle_array_key(&mut keys, &field, &[0x04]);
        assert_eq!(keys.len(), 1);
        assert!(keys.contains(&Usage::from(KEYBOARD_USAGE_MIN + 4)));
    }

    #[test]
    fn handle_array_key_does_nothing_when_zero() {
        let mut keys = BTreeSet::new();
        let field = hidparser::ArrayField {
            bits: 0..8,
            usage_list: vec![hidparser::report_data_types::UsageRange::from(KEYBOARD_USAGE_MIN..=KEYBOARD_USAGE_MAX)],
            logical_minimum: 0.into(),
            logical_maximum: 0x65.into(),
            ..Default::default()
        };
        handle_array_key(&mut keys, &field, &[0x00]);
        assert!(keys.is_empty());
    }

    // --- build_led_report tests ---

    #[test]
    fn build_led_report_sets_field_when_usage_present() {
        let mut led_state = BTreeSet::new();
        led_state.insert(Usage::from(LED_USAGE_MIN));
        let field = led_field(LED_USAGE_MIN, 0);
        let mut report = [0u8; 1];
        build_led_report(&led_state, &field, &mut report);
        assert_ne!(report[0] & 0x01, 0);
    }

    #[test]
    fn build_led_report_clears_field_when_usage_absent() {
        let led_state = BTreeSet::new();
        let field = led_field(LED_USAGE_MIN, 0);
        let mut report = [0xFFu8; 1];
        build_led_report(&led_state, &field, &mut report);
        assert_eq!(report[0] & 0x01, 0);
    }

    // --- process_report with array fields ---

    #[test]
    fn process_report_with_array_field_handles_key_press() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        kq.set_layout(Some(crate::keyboard::layout::get_default_keyboard_layout()));

        let descriptor = ReportDescriptor {
            input_reports: vec![hidparser::Report {
                report_id: None,
                size_in_bits: 8,
                fields: vec![ReportField::Array(hidparser::ArrayField {
                    bits: 0..8,
                    usage_list: vec![hidparser::report_data_types::UsageRange::from(
                        KEYBOARD_USAGE_MIN..=KEYBOARD_USAGE_MAX,
                    )],
                    logical_minimum: 0.into(),
                    logical_maximum: 0x65.into(),
                    ..Default::default()
                })],
            }],
            output_reports: vec![],
            bad_input_reports: vec![],
            bad_output_reports: vec![],
            features: vec![],
            bad_features: vec![],
        };
        processor.process_descriptor(descriptor).unwrap();

        // Report value 0x04 → index 4 in usage range → produces a keystroke
        processor.process_report(&[0x04], &mut kq);
        let key_data = kq.pop_key().unwrap();
        assert_ne!(key_data.key.unicode_char, 0);
    }

    #[test]
    fn process_report_key_release_produces_key_up() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        kq.set_layout(Some(crate::keyboard::layout::get_default_keyboard_layout()));
        processor.process_descriptor(modifier_only_descriptor()).unwrap();

        // Press left shift
        processor.process_report(&[0x02], &mut kq);
        // Release it
        processor.process_report(&[0x00], &mut kq);

        // After release, the init_key_state should show shift no longer pressed
        let state = kq.init_key_state();
        assert_eq!(state.key_shift_state & protocols::simple_text_input_ex::LEFT_SHIFT_PRESSED, 0);
    }

    #[test]
    fn process_report_report_length_mismatch_still_processes() {
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        kq.set_layout(Some(crate::keyboard::layout::get_default_keyboard_layout()));
        processor.process_descriptor(modifier_only_descriptor()).unwrap();

        // Send 2 bytes when descriptor expects 1 — should still process without error
        processor.process_report(&[0x01, 0x00], &mut kq);
    }

    #[test]
    fn process_report_with_report_id_strips_id_byte() {
        let report_id = hidparser::report_data_types::ReportId::from(&[0x01][..]);
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        kq.set_layout(Some(crate::keyboard::layout::get_default_keyboard_layout()));
        let descriptor = ReportDescriptor {
            input_reports: vec![hidparser::Report {
                report_id: Some(report_id),
                size_in_bits: 8,
                fields: vec![ReportField::Variable(modifier_field(KEYBOARD_MODIFIER_USAGE_MIN, 0))],
            }],
            output_reports: vec![],
            bad_input_reports: vec![],
            bad_output_reports: vec![],
            features: vec![],
            bad_features: vec![],
        };
        processor.process_descriptor(descriptor).unwrap();

        // Report with ID byte + modifier data
        processor.process_report(&[0x01, 0x01], &mut kq);
        let state = kq.init_key_state();
        assert_ne!(state.key_shift_state & protocols::simple_text_input_ex::LEFT_CONTROL_PRESSED, 0);
    }

    #[test]
    fn process_report_with_report_id_and_empty_data_is_no_op() {
        let report_id = hidparser::report_data_types::ReportId::from(&[0x01][..]);
        let mut processor = KeyboardProcessor::new();
        let mut kq = key_queue::KeyQueue::default();
        let descriptor = ReportDescriptor {
            input_reports: vec![hidparser::Report {
                report_id: Some(report_id),
                size_in_bits: 8,
                fields: vec![ReportField::Variable(modifier_field(KEYBOARD_MODIFIER_USAGE_MIN, 0))],
            }],
            output_reports: vec![],
            bad_input_reports: vec![],
            bad_output_reports: vec![],
            features: vec![],
            bad_features: vec![],
        };
        processor.process_descriptor(descriptor).unwrap();

        // Only report ID byte, no data
        let result = processor.process_report(&[0x01], &mut kq);
        assert!(!result.should_signal_notify);
    }

    #[test]
    fn process_descriptor_with_padding_field_ignores_it() {
        let mut processor = KeyboardProcessor::new();
        let descriptor = ReportDescriptor {
            input_reports: vec![hidparser::Report {
                report_id: None,
                size_in_bits: 16,
                fields: vec![
                    ReportField::Variable(modifier_field(KEYBOARD_MODIFIER_USAGE_MIN, 0)),
                    ReportField::Padding(hidparser::PaddingField { bits: 8..16 }),
                ],
            }],
            output_reports: vec![],
            bad_input_reports: vec![],
            bad_output_reports: vec![],
            features: vec![],
            bad_features: vec![],
        };
        assert_eq!(processor.process_descriptor(descriptor), Ok(()));
        let report_data = processor.input_reports.values().next().unwrap();
        assert_eq!(report_data.relevant_variable_fields.len(), 1);
    }

    #[test]
    fn process_descriptor_output_array_field_ignored() {
        let mut processor = KeyboardProcessor::new();
        let descriptor = ReportDescriptor {
            input_reports: vec![hidparser::Report {
                report_id: None,
                size_in_bits: 8,
                fields: vec![ReportField::Variable(modifier_field(KEYBOARD_MODIFIER_USAGE_MIN, 0))],
            }],
            output_reports: vec![hidparser::Report {
                report_id: None,
                size_in_bits: 8,
                fields: vec![ReportField::Array(hidparser::ArrayField { bits: 0..8, ..Default::default() })],
            }],
            bad_input_reports: vec![],
            bad_output_reports: vec![],
            features: vec![],
            bad_features: vec![],
        };
        assert_eq!(processor.process_descriptor(descriptor), Ok(()));
        assert!(processor.output_builders.is_empty());
    }

    #[test]
    fn build_led_output_reports_produces_correct_output() {
        let mut processor = KeyboardProcessor::new();
        processor.process_descriptor(keyboard_with_leds_descriptor()).unwrap();
        // Set LED state to include num lock LED
        processor.led_state.insert(Usage::from(LED_USAGE_MIN));

        let reports = processor.build_led_output_reports();
        assert!(!reports.is_empty());
        let (_, report_data) = &reports[0];
        // First bit should be set for the LED
        assert_ne!(report_data[0] & 0x01, 0);
    }

    #[cfg(feature = "ctrl-alt-del")]
    mod ctrl_alt_del_tests {
        use super::*;
        use core::sync::atomic::{AtomicBool, Ordering};
        use r_efi::protocols::simple_text_input_ex::{
            KeyData, KeyState, LEFT_ALT_PRESSED, LEFT_CONTROL_PRESSED, RIGHT_ALT_PRESSED, RIGHT_CONTROL_PRESSED,
            SHIFT_STATE_VALID,
        };

        static RESET_CALLED: AtomicBool = AtomicBool::new(false);
        static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

        extern "efiapi" fn mock_reset_system(
            _reset_type: efi::ResetType,
            _status: efi::Status,
            _data_size: usize,
            _data: *mut core::ffi::c_void,
        ) {
            RESET_CALLED.store(true, Ordering::SeqCst);
        }

        fn make_key_data(scan_code: u16, shift_state: u32) -> KeyData {
            KeyData {
                key: protocols::simple_text_input::InputKey { scan_code, unicode_char: 0 },
                key_state: KeyState { key_shift_state: shift_state, key_toggle_state: 0 },
            }
        }

        // Acquires the test lock, installs a mock RuntimeServices, runs the closure,
        // and cleans up. Serializes access to the shared RUNTIME_SERVICES and RESET_CALLED globals.
        fn with_mock_runtime_services<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
            let _guard = TEST_LOCK.lock().unwrap();
            let rt = Box::leak(Box::new(core::mem::MaybeUninit::<efi::RuntimeServices>::zeroed()));
            // SAFETY: Only reset_system is accessed by the code under test.
            unsafe {
                (*rt.as_mut_ptr()).reset_system = mock_reset_system;
            }
            crate::RUNTIME_SERVICES.store(rt.as_mut_ptr(), Ordering::SeqCst);
            RESET_CALLED.store(false, Ordering::SeqCst);
            f();
        }

        #[test]
        fn reset_notification_returns_invalid_parameter_on_null() {
            let status = reset_notification_function(core::ptr::null_mut());
            assert_eq!(status, efi::Status::INVALID_PARAMETER);
        }

        #[test]
        fn reset_notification_ignores_non_delete_scan_code() {
            with_mock_runtime_services(|| {
                let mut key_data =
                    make_key_data(key_queue::SCAN_F1, SHIFT_STATE_VALID | LEFT_CONTROL_PRESSED | LEFT_ALT_PRESSED);
                let status = reset_notification_function(&mut key_data);
                assert_eq!(status, efi::Status::SUCCESS);
                assert!(!RESET_CALLED.load(Ordering::SeqCst));
            });
        }

        #[test]
        fn reset_notification_ignores_delete_without_shift_state_valid() {
            with_mock_runtime_services(|| {
                let mut key_data = make_key_data(key_queue::SCAN_DELETE, LEFT_CONTROL_PRESSED | LEFT_ALT_PRESSED);
                let status = reset_notification_function(&mut key_data);
                assert_eq!(status, efi::Status::SUCCESS);
                assert!(!RESET_CALLED.load(Ordering::SeqCst));
            });
        }

        #[test]
        fn reset_notification_ignores_delete_without_ctrl() {
            with_mock_runtime_services(|| {
                let mut key_data = make_key_data(key_queue::SCAN_DELETE, SHIFT_STATE_VALID | LEFT_ALT_PRESSED);
                let status = reset_notification_function(&mut key_data);
                assert_eq!(status, efi::Status::SUCCESS);
                assert!(!RESET_CALLED.load(Ordering::SeqCst));
            });
        }

        #[test]
        fn reset_notification_ignores_delete_without_alt() {
            with_mock_runtime_services(|| {
                let mut key_data = make_key_data(key_queue::SCAN_DELETE, SHIFT_STATE_VALID | LEFT_CONTROL_PRESSED);
                let status = reset_notification_function(&mut key_data);
                assert_eq!(status, efi::Status::SUCCESS);
                assert!(!RESET_CALLED.load(Ordering::SeqCst));
            });
        }

        #[test]
        fn reset_notification_triggers_on_left_ctrl_left_alt_delete() {
            with_mock_runtime_services(|| {
                let mut key_data =
                    make_key_data(key_queue::SCAN_DELETE, SHIFT_STATE_VALID | LEFT_CONTROL_PRESSED | LEFT_ALT_PRESSED);
                let status = reset_notification_function(&mut key_data);
                assert_eq!(status, efi::Status::SUCCESS);
                assert!(RESET_CALLED.load(Ordering::SeqCst));
            });
        }

        #[test]
        fn reset_notification_triggers_on_right_ctrl_right_alt_delete() {
            with_mock_runtime_services(|| {
                let mut key_data = make_key_data(
                    key_queue::SCAN_DELETE,
                    SHIFT_STATE_VALID | RIGHT_CONTROL_PRESSED | RIGHT_ALT_PRESSED,
                );
                let status = reset_notification_function(&mut key_data);
                assert_eq!(status, efi::Status::SUCCESS);
                assert!(RESET_CALLED.load(Ordering::SeqCst));
            });
        }

        #[test]
        fn reset_notification_triggers_on_mixed_ctrl_alt_delete() {
            with_mock_runtime_services(|| {
                let mut key_data =
                    make_key_data(key_queue::SCAN_DELETE, SHIFT_STATE_VALID | LEFT_CONTROL_PRESSED | RIGHT_ALT_PRESSED);
                let status = reset_notification_function(&mut key_data);
                assert_eq!(status, efi::Status::SUCCESS);
                assert!(RESET_CALLED.load(Ordering::SeqCst));
            });
        }
    }
}
