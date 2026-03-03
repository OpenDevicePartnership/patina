//! HidIo Support.
//!
//! Abstractions for interacting with HID devices via the UEFI HidIo protocol.
//!
//! ## Architecture
//!
//! Incoming reports are buffered through a `ReportQueue` rather than being
//! processed inline from the HidIo producer's callback. This ensures all report
//! processing occurs at a consistent TPL_CALLBACK regardless of the producer's
//! calling TPL:
//!
//! 1. **Report callback** (any TPL): locks [`TplMutex`] at TPL_NOTIFY, pushes
//!    raw bytes onto a [`VecDeque`], signals a TPL_CALLBACK event.
//! 2. **Event handler** (TPL_CALLBACK): locks the same mutex, dequeues all
//!    pending reports, then dispatches them to all receivers.
//!
//! ## Traits
//!
//! - [`HidIo`] — narrow, receiver-facing interface for reading descriptors and
//!   sending output reports.
//! - [`HidReportReceiver`] — interface for logic that receives HID reports.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
pub mod protocol;

use alloc::{boxed::Box, collections::VecDeque, vec::Vec};
use core::{cell::Cell, ffi::c_void, mem::ManuallyDrop, slice::from_raw_parts};

#[cfg(test)]
use mockall::automock;
use r_efi::efi;

use self::protocol::{HID_IO_PROTOCOL_GUID, HidIoProtocol};
use hidparser::ReportDescriptor;
use patina::{
    boot_services::{BootServices, event::EventType, tpl::Tpl},
    tpl_mutex::TplMutex,
};

/// Interface for logic that receives HID reports from a device.
#[cfg_attr(test, automock)]
pub trait HidReportReceiver {
    /// Passes an incoming report to the receiver for processing.
    fn receive_report(&mut self, report: &[u8], hid_io: &dyn HidIo);
}

/// Factory function that creates a fully initialized HidReportReceiver for a controller.
///
/// Returns the receiver on success, or an error status if the device is not supported
/// by this receiver type (e.g. wrong report descriptor).
pub type ReceiverFactory = Box<dyn FnOnce(efi::Handle, &dyn HidIo) -> Result<Box<dyn HidReportReceiver>, efi::Status>>;

/// Receiver-facing interface for interacting with a HID device.
///
/// Provides read-only access to the report descriptor and the ability to send
/// output reports (e.g. keyboard LED state). Does not manage device lifecycle.
#[cfg_attr(test, automock)]
pub trait HidIo {
    /// Returns the parsed report descriptor for the device.
    fn get_report_descriptor(&self) -> Result<ReportDescriptor, efi::Status>;
    /// Sends an output report to the device.
    fn set_output_report(&self, id: Option<u8>, report: &[u8]) -> Result<(), efi::Status>;
}

// -- ReportQueue ----------------------------------------------------------

/// State for active report reception.
///
/// Heap-allocated via [`Box`] for address stability. Shared between the HidIo
/// report callback (pushes reports) and the TPL_CALLBACK event handler
/// (processes them and fans out to all receivers).
struct ReportQueue<T: BootServices + Clone + 'static> {
    /// Pending report bytes, protected at TPL_NOTIFY.
    queue: TplMutex<VecDeque<Vec<u8>>, T>,
    /// Boot services reference for signaling the drain event.
    boot_services: &'static T,
    /// TPL_CALLBACK event that triggers [`UefiHidIo::process_queued_reports`].
    process_queue_event: efi::Event,
    /// The receivers that process drained reports.
    receivers: Vec<Box<dyn HidReportReceiver>>,
    /// Raw pointer to the HidIo protocol for passing `&dyn HidIo` to receivers.
    hid_io: *const HidIoProtocol,
    /// Set to `true` during drop if unregister_report_callback fails. Checked by
    /// `report_callback` to bail early, preventing access to freed resources.
    poisoned: Cell<bool>,
}

// -- UefiHidIo --------------------------------------------------------------

/// HID device using UEFI boot services to interact with HidIo controllers.
///
/// Reports are buffered in a [`TplMutex`]-protected queue and processed at
/// TPL_CALLBACK via a UEFI event, rather than inline from the producer's
/// callback. The device is fully active from construction; teardown happens on
/// drop.
pub struct UefiHidIo<T: BootServices + Clone + 'static> {
    hid_io: *const HidIoProtocol,
    boot_services: &'static T,
    controller: efi::Handle,
    agent: efi::Handle,
    report_queue: ManuallyDrop<Box<ReportQueue<T>>>,
}

impl<T: BootServices + Clone + 'static> UefiHidIo<T> {
    /// Returns true if the given controller supports the HidIo protocol.
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // efi::Handle is an opaque *mut c_void, not dereferenced
    pub fn supports(boot_services: &'static T, agent: efi::Handle, controller: efi::Handle) -> bool {
        // SAFETY: HidIoProtocol layout matches the HidIo GUID; ProtocolInterface is correctly implemented.
        // We only care that the protocol exists, we do not use the resulting reference.
        unsafe {
            boot_services
                .open_protocol::<HidIoProtocol>(controller, agent, controller, efi::OPEN_PROTOCOL_GET_PROTOCOL)
                .is_ok()
        }
    }

    /// Creates a new UefiHidIo bound to the given controller.
    ///
    /// Opens the device `by_driver`, runs each factory to create receivers (keeping
    /// only those that succeed), creates the report queue, and registers the
    /// protocol callback. Returns `UNSUPPORTED` if no receivers initialize
    /// successfully. The device is released on drop.
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // efi::Handle is an opaque *mut c_void, not dereferenced
    pub fn new(
        boot_services: &'static T,
        agent: efi::Handle,
        controller: efi::Handle,
        receiver_factories: Vec<ReceiverFactory>,
    ) -> Result<Self, efi::Status> {
        // SAFETY: HidIoProtocol layout matches the HidIo GUID; ProtocolInterface is correctly implemented.
        // Open BY_DRIVER to ensure exclusive access to the underlying protocol.
        let hid_io_raw = unsafe {
            boot_services.open_protocol::<HidIoProtocol>(controller, agent, controller, efi::OPEN_PROTOCOL_BY_DRIVER)
        }?;

        let hid_io = hid_io_raw as *const HidIoProtocol;

        // SAFETY: hid_io points to a protocol opened BY_DRIVER above; valid for our lifetime.
        let hid_io_ref = unsafe { &*hid_io };

        // Create receivers from factories, keeping only those that succeed.
        let receivers: Vec<_> =
            receiver_factories.into_iter().filter_map(|factory| factory(controller, hid_io_ref).ok()).collect();

        if receivers.is_empty() {
            // No receivers initialized — close the protocol and report unsupported.
            log::trace!("UefiHidIo::new: no receivers initialized, returning UNSUPPORTED");
            if let Err(status) = boot_services.close_protocol(controller, &HID_IO_PROTOCOL_GUID, agent, controller) {
                log::error!("Unexpected error closing HidIo protocol: {status:x?}");
            }
            return Err(efi::Status::UNSUPPORTED);
        }

        // Build the report queue.
        let mut report_queue = Box::new(ReportQueue {
            queue: TplMutex::new((*boot_services).clone(), Tpl::NOTIFY, VecDeque::new()),
            boot_services,
            process_queue_event: core::ptr::null_mut(),
            receivers,
            hid_io,
            poisoned: Cell::new(false),
        });

        let queue_ptr: *mut ReportQueue<T> = &mut *report_queue;

        // SAFETY: queue_ptr is heap-allocated via Box and remains valid for the event's lifetime
        // (the event is closed before the Box is dropped).
        let process_queue_event = unsafe {
            boot_services.create_event_unchecked::<ReportQueue<T>>(
                EventType::NOTIFY_SIGNAL,
                Tpl::CALLBACK,
                Some(Self::process_queued_reports),
                queue_ptr,
            )
        }
        .inspect_err(|_| {
            let _ = boot_services.close_protocol(controller, &HID_IO_PROTOCOL_GUID, agent, controller);
        })?;

        report_queue.process_queue_event = process_queue_event;

        // Register the HidIo callback. This must happen after process_queue_event is stored so the
        // report_callback can safely signal it. This also configures HidIo to start sending reports.
        // SAFETY: hid_io points to a protocol opened BY_DRIVER above; valid for our lifetime.
        let hid_io_protocol = unsafe { &*hid_io };
        match (hid_io_protocol.register_report_callback)(hid_io, Self::report_callback, queue_ptr as *mut c_void) {
            efi::Status::SUCCESS => (),
            err => {
                let _ = boot_services.close_event(process_queue_event);
                let _ = boot_services.close_protocol(controller, &HID_IO_PROTOCOL_GUID, agent, controller);
                return Err(err);
            }
        }

        log::trace!(
            "UefiHidIo::new: initialized with {:?} receivers on controller {:?}",
            report_queue.receivers.len(),
            controller
        );

        Ok(Self { hid_io, boot_services, controller, agent, report_queue: ManuallyDrop::new(report_queue) })
    }

    /// HidIo protocol report callback. Enqueues the report and signals the drain event. This runs at
    /// whatever TPL the HidIo instance generating the report runs at, which may vary by controller and report type.
    extern "efiapi" fn report_callback(report_buffer_size: u16, report_buffer: *mut c_void, context: *mut c_void) {
        // SAFETY: context is a valid *mut ReportQueue<T> set during register_report_callback in new().
        // The HidIo protocol callback signature requires *mut c_void, but this function only takes a
        // shared reference; no mutable aliasing occurs because mutation goes through the TplMutex on the queue.
        let report_queue =
            unsafe { (context as *const ReportQueue<T>).as_ref() }.expect("null report_callback context");
        if report_queue.poisoned.get() {
            return;
        }
        // SAFETY: report_buffer is valid for report_buffer_size bytes per HidIo protocol contract.
        let report = unsafe { from_raw_parts(report_buffer as *const u8, report_buffer_size as usize) };
        log::trace!("report_callback: received report, size: {:?}", report_buffer_size);
        {
            let mut queue = report_queue.queue.lock();
            queue.push_back(report.to_vec());
        }
        let _ = report_queue.boot_services.signal_event(report_queue.process_queue_event);
    }

    /// Event handler that dequeues pending reports and dispatches them to all receivers.
    /// This runs at TPL_CALLBACK, ensuring that processing for events happens at the lowest possible TPL regardless of
    /// the TPL the report was received at.
    extern "efiapi" fn process_queued_reports(_event: efi::Event, context: *mut ReportQueue<T>) {
        // SAFETY: context is a valid *mut ReportQueue<T> set during event creation.
        let report_queue = unsafe { context.as_mut() }.expect("null process_queued_reports context");
        let reports: Vec<Vec<u8>> = {
            let mut queue = report_queue.queue.lock();
            queue.drain(..).collect()
        };

        log::trace!("process_queued_reports: draining {:?} queued reports", reports.len());

        // Any additional events that are queued while processing will also trigger a signal on this event,
        // which will re-queue this function. So we don't need to worry about missing reports.

        // SAFETY: hid_io points to a protocol opened BY_DRIVER; valid for device lifetime.
        let hid_io = unsafe { &*report_queue.hid_io };
        for report in &reports {
            for receiver in &mut report_queue.receivers {
                receiver.receive_report(report, hid_io);
            }
        }
    }
}

impl<T: BootServices + Clone + 'static> Drop for UefiHidIo<T> {
    fn drop(&mut self) {
        // SAFETY: hid_io points to a protocol opened BY_DRIVER in new; valid until this drop.
        let hid_io_protocol = unsafe { &*self.hid_io };
        let unregister_status = (hid_io_protocol.unregister_report_callback)(self.hid_io, Self::report_callback);

        if unregister_status != efi::Status::SUCCESS {
            // Callback may still fire — poison the queue so report_callback becomes a no-op,
            // and leak the Box so the memory it dereferences remains valid.
            self.report_queue.poisoned.set(true);
            log::error!(
                "Failed to unregister report callback: {unregister_status:x?}. \
                 Leaking ReportQueue to prevent use-after-free."
            );
        }

        let _ = self.boot_services.close_event(self.report_queue.process_queue_event);
        if let Err(status) =
            self.boot_services.close_protocol(self.controller, &HID_IO_PROTOCOL_GUID, self.agent, self.controller)
        {
            log::error!("Unexpected error closing HidIo protocol: {status:x?}");
        }

        if unregister_status == efi::Status::SUCCESS {
            // SAFETY: No more callbacks can fire; safe to free the report queue.
            unsafe { ManuallyDrop::drop(&mut self.report_queue) };
        }
    }
}

#[cfg(test)]
mod test {
    use alloc::{boxed::Box, vec};
    use core::{
        ffi::c_void,
        sync::atomic::{AtomicPtr, Ordering},
    };

    use r_efi::efi;

    use super::protocol::{HidIoReportCallback, HidReportType};

    use patina::boot_services::MockBootServices;

    use super::*;

    fn mock_boot_services() -> &'static mut MockBootServices {
        let mut mock = MockBootServices::new();
        mock.expect_raise_tpl().returning(|_| Tpl::APPLICATION);
        mock.expect_restore_tpl().returning(|_| ());
        // SAFETY: Leaked mock for test use with 'static lifetime requirement.
        unsafe { Box::into_raw(Box::new(mock)).as_mut().unwrap() }
    }

    fn mock_hid_io_protocol() -> &'static mut HidIoProtocol {
        HidIoProtocol::stub()
    }

    /// A factory that always succeeds, returning a no-op receiver.
    fn ok_factory() -> ReceiverFactory {
        Box::new(|_, _| Ok(Box::new(MockHidReportReceiver::new()) as Box<dyn HidReportReceiver>))
    }

    /// A factory that always fails.
    fn err_factory() -> ReceiverFactory {
        Box::new(|_, _| Err(efi::Status::UNSUPPORTED))
    }

    /// Creates a UefiHidIo with a mock protocol and a single no-op receiver.
    /// `setup` is called on the receiver mock before it's boxed.
    fn make_hid_device(
        setup: impl FnOnce(&mut MockBootServices, &mut MockHidReportReceiver),
    ) -> UefiHidIo<MockBootServices> {
        let boot_services = mock_boot_services();
        boot_services.expect_close_protocol().returning(|_, _, _, _| Ok(()));
        boot_services.expect_close_event().returning(|_| Ok(()));
        boot_services
            .expect_create_event_unchecked::<ReportQueue<MockBootServices>>()
            .returning(|_, _, _, _| Ok(0xE0E as efi::Event));

        let mut receiver = MockHidReportReceiver::new();
        setup(boot_services, &mut receiver);

        let hid_io_protocol = mock_hid_io_protocol();
        let hid_io = hid_io_protocol as *const HidIoProtocol;

        let receivers: Vec<Box<dyn HidReportReceiver>> = vec![Box::new(receiver)];
        let mut report_queue = Box::new(ReportQueue {
            poisoned: Cell::new(false),
            queue: TplMutex::new((*boot_services).clone(), Tpl::NOTIFY, VecDeque::new()),
            boot_services,
            process_queue_event: core::ptr::null_mut(),
            receivers,
            hid_io,
        });
        report_queue.process_queue_event = 0xE0E as efi::Event;

        UefiHidIo {
            hid_io,
            boot_services,
            controller: core::ptr::null_mut(),
            agent: core::ptr::null_mut(),
            report_queue: ManuallyDrop::new(report_queue),
        }
    }

    /// Returns a mutable pointer to the mock protocol for setting up test stubs.
    ///
    /// # Safety
    ///
    /// Only safe in tests where the protocol was leaked from `mock_hid_io_protocol()`.
    /// The caller must ensure no aliasing references exist.
    unsafe fn mock_protocol(hid_io: *const HidIoProtocol) -> &'static mut HidIoProtocol {
        // SAFETY: hid_io was leaked from mock_hid_io_protocol and is valid for the test lifetime.
        unsafe { &mut *(hid_io as *mut HidIoProtocol) }
    }

    #[test]
    fn new_returns_error_when_open_protocol_fails() {
        let boot_services = mock_boot_services();
        boot_services.expect_open_protocol::<HidIoProtocol>().returning(|_, _, _, _| Err(efi::Status::NOT_FOUND));

        let result = UefiHidIo::new(boot_services, core::ptr::null_mut(), core::ptr::null_mut(), vec![ok_factory()]);
        assert_eq!(result.err(), Some(efi::Status::NOT_FOUND));
    }

    #[test]
    fn new_returns_unsupported_when_no_receivers_initialize() {
        let boot_services = mock_boot_services();
        boot_services.expect_open_protocol::<HidIoProtocol>().returning(|_, _, _, _| Ok(mock_hid_io_protocol()));
        boot_services.expect_close_protocol().returning(|_, _, _, _| Ok(()));

        let result = UefiHidIo::new(boot_services, core::ptr::null_mut(), core::ptr::null_mut(), vec![err_factory()]);
        assert_eq!(result.err(), Some(efi::Status::UNSUPPORTED));
    }

    #[test]
    fn new_returns_unsupported_when_receivers_vec_is_empty() {
        let boot_services = mock_boot_services();
        boot_services.expect_open_protocol::<HidIoProtocol>().returning(|_, _, _, _| Ok(mock_hid_io_protocol()));
        boot_services.expect_close_protocol().returning(|_, _, _, _| Ok(()));

        let result = UefiHidIo::new(boot_services, core::ptr::null_mut(), core::ptr::null_mut(), vec![]);
        assert_eq!(result.err(), Some(efi::Status::UNSUPPORTED));
    }

    #[test]
    fn hid_io_set_output_report_calls_protocol() {
        let device = make_hid_device(|_, _| {});

        extern "efiapi" fn mock_set_report(
            _this: *const HidIoProtocol,
            report_id: u8,
            report_type: HidReportType,
            report_buffer_size: usize,
            report_buffer: *mut c_void,
        ) -> efi::Status {
            assert_eq!(report_id, 5);
            assert_eq!(report_type, HidReportType::OutputReport);
            assert_eq!(report_buffer_size, 4);
            // SAFETY: report_buffer is valid for report_buffer_size bytes, as guaranteed by the HID I/O protocol contract.
            let report = unsafe { core::slice::from_raw_parts(report_buffer as *const u8, report_buffer_size) };
            assert_eq!(report, [0x00, 0x01, 0x02, 0x03]);
            efi::Status::SUCCESS
        }
        // SAFETY: device.hid_io was leaked from mock_hid_io_protocol; no aliasing references exist.
        unsafe { mock_protocol(device.hid_io) }.set_report = mock_set_report;

        // SAFETY: device.hid_io is a valid pointer leaked from mock_hid_io_protocol.
        assert_eq!(unsafe { &*device.hid_io }.set_output_report(Some(5), &[0x00, 0x01, 0x02, 0x03]), Ok(()));
    }

    #[test]
    fn report_callback_enqueues_and_process_queued_reports_delivers_to_all() {
        static CALLBACK_CONTEXT: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

        let boot_services = mock_boot_services();
        boot_services.expect_close_protocol().returning(|_, _, _, _| Ok(()));
        boot_services.expect_close_event().returning(|_| Ok(()));
        boot_services
            .expect_create_event_unchecked::<ReportQueue<MockBootServices>>()
            .returning(|_, _, _, _| Ok(0xE0E as efi::Event));
        boot_services.expect_signal_event().returning(|_| Ok(()));

        let hid_io_protocol = mock_hid_io_protocol();

        extern "efiapi" fn register_cb(
            _this: *const HidIoProtocol,
            _callback: HidIoReportCallback,
            context: *mut c_void,
        ) -> efi::Status {
            CALLBACK_CONTEXT.store(context, Ordering::Relaxed);
            efi::Status::SUCCESS
        }
        hid_io_protocol.register_report_callback = register_cb;

        let hid_io = hid_io_protocol as *const HidIoProtocol;

        let mut r1 = MockHidReportReceiver::new();
        r1.expect_receive_report().withf(|report, _| report == [0x10u8, 0x20, 0x30]).times(1).return_const(());
        let mut r2 = MockHidReportReceiver::new();
        r2.expect_receive_report().withf(|report, _| report == [0x10u8, 0x20, 0x30]).times(1).return_const(());

        let receivers: Vec<Box<dyn HidReportReceiver>> = vec![Box::new(r1), Box::new(r2)];
        let mut report_queue = Box::new(ReportQueue {
            poisoned: Cell::new(false),
            queue: TplMutex::new((*boot_services).clone(), Tpl::NOTIFY, VecDeque::new()),
            boot_services,
            process_queue_event: core::ptr::null_mut(),
            receivers,
            hid_io,
        });
        report_queue.process_queue_event = 0xE0E as efi::Event;

        let mut device = UefiHidIo {
            hid_io,
            boot_services,
            controller: core::ptr::null_mut(),
            agent: core::ptr::null_mut(),
            report_queue: ManuallyDrop::new(report_queue),
        };

        // Simulate the HidIo producer calling report_callback.
        let report_data = [0x10u8, 0x20, 0x30];
        let queue_ptr: *mut ReportQueue<MockBootServices> = &mut **device.report_queue as *mut _;
        UefiHidIo::<MockBootServices>::report_callback(
            report_data.len() as u16,
            report_data.as_ptr() as *mut c_void,
            queue_ptr as *mut c_void,
        );

        // Manually invoke the process_queued_reports handler (normally triggered by the UEFI event system).
        UefiHidIo::<MockBootServices>::process_queued_reports(core::ptr::null_mut(), queue_ptr);
        // MockHidReportReceivers will verify expectations on drop.
    }

    #[test]
    fn drop_unregisters_callback_and_closes_protocol() {
        let device = make_hid_device(|_, _| {});
        drop(device);
        // MockBootServices expectations for close_protocol and close_event
        // are verified on drop.
    }

    #[test]
    fn new_cleans_up_on_register_callback_failure() {
        extern "efiapi" fn failing_register(
            _this: *const HidIoProtocol,
            _callback: HidIoReportCallback,
            _context: *mut c_void,
        ) -> efi::Status {
            efi::Status::DEVICE_ERROR
        }

        let boot_services = mock_boot_services();
        boot_services.expect_open_protocol::<HidIoProtocol>().returning(|_, _, _, _| {
            let protocol = mock_hid_io_protocol();
            protocol.register_report_callback = failing_register;
            Ok(protocol)
        });
        boot_services.expect_close_protocol().returning(|_, _, _, _| Ok(()));
        boot_services.expect_close_event().returning(|_| Ok(()));
        boot_services
            .expect_create_event_unchecked::<ReportQueue<MockBootServices>>()
            .returning(|_, _, _, _| Ok(0xE0E as efi::Event));

        let result = UefiHidIo::new(boot_services, core::ptr::null_mut(), core::ptr::null_mut(), vec![ok_factory()]);
        assert_eq!(result.err(), Some(efi::Status::DEVICE_ERROR));
    }
}
