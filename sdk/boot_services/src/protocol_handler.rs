use core::{ffi::c_void, ptr::NonNull};

use r_efi::efi;

pub type Registration = NonNull<c_void>;

#[derive(Debug, Clone, Copy)]
pub enum HandleSearchType {
    AllHandle,
    ByRegisterNotify(Registration),
    ByProtocol(&'static efi::Guid),
}

impl From<HandleSearchType> for efi::LocateSearchType {
    fn from(val: HandleSearchType) -> Self {
        match val {
            HandleSearchType::AllHandle => efi::ALL_HANDLES,
            HandleSearchType::ByRegisterNotify(_) => efi::BY_REGISTER_NOTIFY,
            HandleSearchType::ByProtocol(_) => efi::BY_PROTOCOL,
        }
    }
}
