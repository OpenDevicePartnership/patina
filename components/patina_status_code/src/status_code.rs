use alloc::{boxed::Box, vec::Vec};
use r_efi::efi;

use crate::{
    error::RscHandlerError,
    protocol::EfiStatusCodeHeader,
    service::{RscHandler, RscHandlerCallback, StatusCodeType, StatusCodeValue},
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

#[derive(Clone, PartialEq, Eq)]
struct RscHandlerCallbackEntry {
    callback: RscHandlerCallback,
    tpl: tpl::Tpl,
    status_code_buffer: Vec<RscDataEntry>,
    // Need this bc rust fn pointer comparisons aren't guaranteed
    cb_id: usize,
}

#[derive(Clone, PartialEq, Eq)]
struct RscDataEntry {
    code_type: StatusCodeType,
    value: StatusCodeValue,
    instance: u32,
    reserved: u32,
    caller_id: efi::Guid,
    data: EfiStatusCodeHeader,
}

#[derive(IntoService)]
#[service(dyn RscHandler)]
struct StandardRscHandler<B: BootServices + 'static> {
    callback_list: Vec<RscHandlerCallbackEntry>,
    boot_services: B,
    next_id: usize, // hack for rust fn comp
}

impl<B> RscHandler for StandardRscHandler<B>
where
    B: BootServices,
{
    fn register(&mut self, callback: RscHandlerCallback, tpl: tpl::Tpl) -> Result<(), RscHandlerError> {
        for entry in &self.callback_list {
            if entry.callback == callback {
                return Err(RscHandlerError::CallbackAlreadyRegistered);
            }
        }

        self.next_id += 1;

        let new_entry = RscHandlerCallbackEntry { callback, tpl, status_code_buffer: Vec::new(), cb_id: self.next_id };

        if tpl <= tpl::Tpl(efi::TPL_HIGH_LEVEL) {
            self.boot_services
                .create_event(EventType::NOTIFY_SIGNAL, tpl, Some(rsc_hander_notification), Box::new(new_entry.clone()))
                .map_err(|e| RscHandlerError::EventCreationFailed(e))?;
        }

        self.callback_list.push(new_entry);

        Ok(())
    }

    fn unregister(&self, callback: RscHandlerCallback) -> Result<(), RscHandlerError> {
        // Implementation for unregistering the callback
        Ok(())
    }
}

extern "efiapi" fn rsc_hander_notification(event: efi::Event, ctx: Box<RscHandlerCallbackEntry>) {}
