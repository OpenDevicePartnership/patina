use core::cell::OnceCell;
use core::sync::atomic::AtomicBool;

use alloc::vec;
use alloc::{boxed::Box, vec::Vec};
use patina_sdk::boot_services::BootServices;
use patina_sdk::boot_services::tpl::Tpl;
use patina_sdk::runtime_services::RuntimeServices;
use r_efi::efi::{self, Status};
use spin::RwLock;

use crate::service::StatusCodeData;
use crate::{
    callback::{self, RscHandlerCallback},
    error::RscHandlerError,
    protocol::EfiStatusCodeHeader,
    service::{RscHandler, StatusCodeType, StatusCodeValue},
};

use patina_sdk::{
    boot_services::{
        StandardBootServices,
        event::EventType,
        tpl::{self},
    },
    component::service::IntoService,
    uefi_protocol::status_code::StatusCodeRuntimeProtocol,
};

pub(crate) type RustRscHandlerCallback =
    fn(StatusCodeType, StatusCodeValue, u32, Option<efi::Guid>, Option<StatusCodeData>) -> efi::Status;

#[derive(Clone, PartialEq, Eq)]
struct RscHandlerCallbackEntry {
    pub(crate) callback: RscHandlerCallback,
    tpl: tpl::Tpl,
    status_code_buffer: Vec<RscDataEntry>,
    event_handle: Option<efi::Event>, // optional if TPL is high
}

impl RscHandlerCallbackEntry {
    fn new(callback: RscHandlerCallback, tpl: tpl::Tpl) -> Self {
        Self { callback, tpl, status_code_buffer: Vec::new(), event_handle: None }
    }

    pub fn process_all(&mut self) {
        for entry in &self.status_code_buffer {
            match &self.callback {
                RscHandlerCallback::Rust(cb) => {
                    cb(entry.code_type, entry.value, entry.instance, entry.caller_id, entry.status_code_data.clone());
                }
                RscHandlerCallback::Efi(cb) => {
                    // Optional parameters become NULL pointers when passed to EFI callbacks.
                    let caller_id = entry.caller_id.as_ref().map_or(core::ptr::null(), |id| id as *const efi::Guid);
                    match &entry.status_code_data {
                        Some(data) => {
                            let mut data_header = Vec::new();
                            data_header.extend_from_slice(&data.data_header.to_bytes());
                            data_header.extend_from_slice(&data.data_bytes);
                            let data_header_ptr = data_header.as_ptr() as *const EfiStatusCodeHeader;
                            cb(entry.code_type, entry.value, entry.instance, caller_id, data_header_ptr);
                        }
                        None => {
                            cb(entry.code_type, entry.value, entry.instance, caller_id, core::ptr::null());
                        }
                    }
                }
            }
        }
        self.status_code_buffer.clear();
    }
}

#[derive(Clone, PartialEq, Eq)]
struct RscDataEntry {
    code_type: StatusCodeType,
    value: StatusCodeValue,
    instance: u32,
    reserved: u32,
    caller_id: Option<efi::Guid>,
    status_code_data: Option<StatusCodeData>,
}

#[derive(IntoService)]
#[service(dyn RscHandler)]
pub(crate) struct StandardRscHandler<B: BootServices + 'static, R: RuntimeServices + 'static> {
    pub(crate) callback_list: RwLock<Vec<RscHandlerCallbackEntry>>,
    boot_services: OnceCell<B>,
    runtime_services: OnceCell<R>,
    /// Report Status Code can only be entered once at a time, this flag is used to prevent re-entrancy.
    report_status_code_entry: AtomicBool,
}

unsafe impl<B> Sync for StandardRscHandler<B> where B: BootServices + Sync {}

impl<B> StandardRscHandler<B>
where
    B: BootServices,
{
    pub const fn new_uninit() -> Self {
        Self {
            callback_list: RwLock::new(vec![]),
            boot_services: OnceCell::new(),
            report_status_code_entry: AtomicBool::new(false),
        }
    }

    pub fn initialize(&self, bs: B) -> Result<(), RscHandlerError>
    where
        B: BootServices,
    {
        self.boot_services.set(bs).map_err(|_| RscHandlerError::AlreadyInitialized)
    }
}

impl<B> RscHandler for StandardRscHandler<B>
where
    B: BootServices,
{
    fn register_callback(&self, callback: RscHandlerCallback, tpl: tpl::Tpl) -> Result<(), RscHandlerError> {
        for entry in self.callback_list.read().iter() {
            // sherry: a thorny problem
            if entry.callback == callback {
                return Err(RscHandlerError::CallbackAlreadyRegistered);
            }
        }

        let mut new_entry = RscHandlerCallbackEntry::new(callback, tpl);

        if tpl <= tpl::Tpl(efi::TPL_HIGH_LEVEL) {
            let event = self
                .boot_services
                .get()
                .ok_or(RscHandlerError::NotInitialized)?
                .create_event::<Box<RscHandlerCallbackEntry>>(
                    EventType::NOTIFY_SIGNAL,
                    tpl,
                    Some(rsc_hander_notification),
                    Box::new(new_entry.clone()),
                )
                .map_err(|e| RscHandlerError::EventCreationFailed(e))?;
            new_entry.event_handle = Some(event);
        }

        self.callback_list.write().push(new_entry);

        Ok(())
    }

    fn unregister_callback(&self, callback: RscHandlerCallback) -> Result<(), RscHandlerError> {
        let mut callback_list = self.callback_list.write();
        if let Some(index) = callback_list.iter().position(|entry| entry.callback == callback) {
            let entry = callback_list.remove(index);

            if entry.tpl <= tpl::Tpl(efi::TPL_HIGH_LEVEL) {
                self.boot_services
                    .get()
                    .ok_or(RscHandlerError::NotInitialized)?
                    .close_event(entry.event_handle.ok_or(RscHandlerError::MissingEvent)?)
                    .map_err(|e| RscHandlerError::EventCreationFailed(e))?;
            }
            return Ok(());
        }
        Err(RscHandlerError::UnregisterNotFound)
    }

    fn report_status_code(
        &self,
        code_type: StatusCodeType,
        value: StatusCodeValue,
        instance: u32,
        caller_id: Option<efi::Guid>,
        status_code_data: Option<StatusCodeData>,
    ) -> Result<(), RscHandlerError> {
        if self.report_status_code_entry.swap(true, core::sync::atomic::Ordering::SeqCst) {
            return Err(RscHandlerError::ReentrantReportStatusCode);
        }

        for callback_entry in self.callback_list.write().iter_mut() {
            if callback_entry.tpl <= tpl::Tpl(efi::TPL_HIGH_LEVEL) {
                callback_entry.status_code_buffer.push(RscDataEntry {
                    code_type,
                    value,
                    instance,
                    reserved: 0,
                    caller_id,
                    status_code_data: status_code_data.clone(),
                });
                if let Some(event) = callback_entry.event_handle {
                    self.boot_services
                        .get()
                        .ok_or(RscHandlerError::NotInitialized)?
                        .signal_event(event)
                        .map_err(|e| RscHandlerError::EventCreationFailed(e))?;
                }
            } else {
                match &callback_entry.callback {
                    RscHandlerCallback::Rust(cb) => {
                        // SHERRY: lots of clones is a concern
                        cb(code_type, value, instance, caller_id, status_code_data.clone());
                    }
                    RscHandlerCallback::Efi(cb) => {
                        let caller_id_param = caller_id.as_ref().map_or(core::ptr::null(), |id| id as *const efi::Guid);

                        match &status_code_data {
                            Some(data) => {
                                let mut data_header = Vec::new();
                                data_header.extend_from_slice(&data.data_header.to_bytes());
                                data_header.extend_from_slice(&data.data_bytes);
                                let data_header_ptr = data_header.as_ptr() as *const EfiStatusCodeHeader;
                                cb(code_type, value, instance, caller_id_param, data_header_ptr);
                            }
                            None => {
                                cb(code_type, value, instance, caller_id_param, core::ptr::null());
                            }
                        }
                    }
                }
            }
        }

        self.report_status_code_entry.store(false, core::sync::atomic::Ordering::SeqCst);

        Ok(())
    }
}

impl<B> StandardRscHandler<B>
where
    B: BootServices,
{
    /// Converts C-based function pointers from physical addresses to virtual addresses after the system has switched to virtual mode.
    fn convert_addresses(&self) {
        for entry in self.callback_list.write().iter_mut() {
            if let RscHandlerCallback::Efi(cb) = &mut entry.callback {
                *cb = callback::convert_efi_callback(*cb);
            }
        }
    }
}

extern "efiapi" fn rsc_hander_notification(event: efi::Event, ctx: Box<RscHandlerCallbackEntry>) {
    let mut entry = *ctx;
    entry.process_all();
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicBool, Ordering};
    use patina_sdk::boot_services::MockBootServices;

    use super::*;

    fn dummy_callback(
        _code_type: StatusCodeType,
        _value: StatusCodeValue,
        _instance: u32,
        _caller_id: Option<efi::Guid>,
        _header: Option<StatusCodeData>,
    ) -> efi::Status {
        efi::Status::SUCCESS
    }

    #[test]
    fn test_register_and_unregister_callback() {
        let handler = StandardRscHandler::<MockBootServices>::new_uninit();
        let mut mock_boot_services = MockBootServices::new();
        mock_boot_services
            .expect_create_event::<Box<RscHandlerCallbackEntry>>()
            .return_const_st(Ok(1_usize as efi::Event));
        mock_boot_services.expect_close_event().return_const_st(Ok(()));

        handler.initialize(mock_boot_services).unwrap();

        let tpl = Tpl(efi::TPL_APPLICATION);
        assert!(handler.register_callback(RscHandlerCallback::Rust(dummy_callback), tpl).is_ok());
        assert_eq!(
            handler.register_callback(RscHandlerCallback::Rust(dummy_callback), tpl).unwrap_err(),
            RscHandlerError::CallbackAlreadyRegistered
        );
        assert!(handler.unregister_callback(RscHandlerCallback::Rust(dummy_callback)).is_ok());
        assert_eq!(
            handler.unregister_callback(RscHandlerCallback::Rust(dummy_callback)).unwrap_err(),
            RscHandlerError::UnregisterNotFound
        );
    }

    #[test]
    fn test_register_with_high_tpl_creates_event() {
        let handler = StandardRscHandler::<MockBootServices>::new_uninit();
        let mut mock_boot_services = MockBootServices::new();
        mock_boot_services
            .expect_create_event::<Box<RscHandlerCallbackEntry>>()
            .times(1)
            .return_const_st(Ok(1_usize as efi::Event));
        handler.initialize(mock_boot_services).unwrap();

        let tpl = Tpl(efi::TPL_HIGH_LEVEL);
        assert!(handler.register_callback(RscHandlerCallback::Rust(dummy_callback), tpl).is_ok());
    }

    #[test]
    fn test_initialize_twice_fails() {
        let handler = StandardRscHandler::<MockBootServices>::new_uninit();
        handler.initialize(MockBootServices::new()).unwrap();
        assert_eq!(handler.initialize(MockBootServices::new()), Err(RscHandlerError::AlreadyInitialized));
    }

    static DIRECT_CALLED: AtomicBool = AtomicBool::new(false);

    fn direct_callback(
        _code_type: StatusCodeType,
        _value: StatusCodeValue,
        _instance: u32,
        _caller_id: Option<efi::Guid>,
        _header: Option<StatusCodeData>,
    ) -> efi::Status {
        DIRECT_CALLED.store(true, Ordering::SeqCst);
        efi::Status::SUCCESS
    }

    #[test]
    fn test_report_status_code_calls_rust_callback_immediately() {
        let handler = StandardRscHandler::<MockBootServices>::new_uninit();
        let mock_boot_services = MockBootServices::new();
        handler.initialize(mock_boot_services).unwrap();

        // Use a TPL greater than TPL_HIGH_LEVEL so the callback is invoked directly.
        let tpl = Tpl(efi::TPL_HIGH_LEVEL + 1);
        assert!(handler.register_callback(RscHandlerCallback::Rust(direct_callback), tpl).is_ok());

        let code_type: StatusCodeType = unsafe { core::mem::zeroed() };
        let value: StatusCodeValue = unsafe { core::mem::zeroed() };

        assert!(handler.report_status_code(code_type, value, 1, None, None).is_ok());
        assert!(DIRECT_CALLED.load(Ordering::SeqCst));
    }

    #[test]
    fn test_report_status_code_buffers_and_signals_event() {
        let handler = StandardRscHandler::<MockBootServices>::new_uninit();
        let mut mock_boot_services = MockBootServices::new();
        // Expect event creation for low TPL registrations.
        mock_boot_services
            .expect_create_event::<Box<RscHandlerCallbackEntry>>()
            .times(1)
            .return_const_st(Ok(1_usize as efi::Event));
        // Expect signaling the event when report_status_code is called.
        mock_boot_services.expect_signal_event().times(1).return_const_st(Ok(()));
        mock_boot_services.expect_close_event().return_const_st(Ok(()));

        handler.initialize(mock_boot_services).unwrap();

        let tpl = Tpl(efi::TPL_APPLICATION);
        assert!(handler.register_callback(RscHandlerCallback::Rust(dummy_callback), tpl).is_ok());

        let code_type: StatusCodeType = unsafe { core::mem::zeroed() };
        let value: StatusCodeValue = unsafe { core::mem::zeroed() };

        // This should buffer the entry and trigger a call to signal_event on the boot services.
        assert!(handler.report_status_code(code_type, value, 42, None, None).is_ok());
    }
}
