use patina_sdk::{
    boot_services::{self, StandardBootServices},
    component::IntoComponent,
    error::EfiError,
};

use crate::protocol::GLOBAL_RSC_HANDLER;

#[derive(IntoComponent, Default)]
pub struct StatusCodeInit {}

impl StatusCodeInit {
    pub fn new() -> Self {
        Self {}
    }

    fn entry_point(self, boot_services: StandardBootServices) -> patina_sdk::error::Result<()> {
        GLOBAL_RSC_HANDLER.initialize(boot_services).map_err(|_| EfiError::AlreadyStarted)?;

        Ok(())
    }
}
