use crate::protocol::EfiRscHandlerCallback;
use crate::status_code::RustRscHandlerCallback;

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum RscHandlerCallback {
    Rust(RustRscHandlerCallback),
    Efi(EfiRscHandlerCallback),
}
