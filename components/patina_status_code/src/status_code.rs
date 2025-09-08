use core::cell::OnceCell;

use alloc::vec;
use alloc::{boxed::Box, vec::Vec};
use r_efi::efi;
use spin::RwLock;

use crate::{
    callback::{self, RscHandlerCallback},
    error::RscHandlerError,
    protocol::EfiStatusCodeHeader,
    service::{RscHandler, StatusCodeType, StatusCodeValue},
};

use patina_sdk::{
    boot_services::{
        BootServices, StandardBootServices,
        event::EventType,
        tpl::{self, Tpl},
    },
    component::service::IntoService,
    uefi_protocol::status_code::StatusCodeRuntimeProtocol,
};

// SHERRY: this may be problematic for FFI
pub(crate) type RustRscHandlerCallback =
    fn(StatusCodeType, StatusCodeValue, u32, efi::Guid, &EfiStatusCodeHeader) -> efi::Status;

#[derive(Clone, PartialEq, Eq)]
struct RscHandlerCallbackEntry {
    callback: RscHandlerCallback,
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
                    cb(entry.code_type, entry.value, entry.instance, entry.caller_id, &entry.data_header);
                }
                RscHandlerCallback::Efi(cb) => unsafe {
                    cb(entry.code_type, entry.value, entry.instance, entry.caller_id, &entry.data_header);
                },
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
    caller_id: efi::Guid,
    data_header: EfiStatusCodeHeader,
}

#[derive(IntoService)]
#[service(dyn RscHandler)]
pub(crate) struct StandardRscHandler<B: BootServices + 'static> {
    callback_list: RwLock<Vec<RscHandlerCallbackEntry>>,
    boot_services: OnceCell<B>,
}

unsafe impl<B> Sync for StandardRscHandler<B> where B: BootServices + Sync {}

impl<B> StandardRscHandler<B>
where
    B: BootServices,
{
    pub const fn new_uninit() -> Self {
        Self { callback_list: RwLock::new(vec![]), boot_services: OnceCell::new() }
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
    fn register(&self, callback: RscHandlerCallback, tpl: tpl::Tpl) -> Result<(), RscHandlerError> {
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
                .create_event(EventType::NOTIFY_SIGNAL, tpl, Some(rsc_hander_notification), Box::new(new_entry.clone()))
                .map_err(|e| RscHandlerError::EventCreationFailed(e))?;
            new_entry.event_handle = Some(event);
        }

        self.callback_list.write().push(new_entry);

        Ok(())
    }

    fn unregister(&self, callback: RscHandlerCallback) -> Result<(), RscHandlerError> {
        // sherry: more fun issues yay :)
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
}

extern "efiapi" fn rsc_hander_notification(event: efi::Event, ctx: Box<RscHandlerCallbackEntry>) {
    let mut entry = *ctx;
    entry.process_all();
}
