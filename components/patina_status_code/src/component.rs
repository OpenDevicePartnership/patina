use patina_sdk::{
    boot_services::{self, StandardBootServices},
    component::{IntoComponent, params::Commands},
    error::EfiError,
};

use crate::protocol::GLOBAL_RSC_HANDLER;

#[derive(IntoComponent, Default)]
pub struct StatusCodeInit {}

impl StatusCodeInit {
    pub fn new() -> Self {
        Self {}
    }

    fn entry_point(self, boot_services: StandardBootServices, mut commands: Commands) -> patina_sdk::error::Result<()> {
        GLOBAL_RSC_HANDLER.initialize(boot_services).map_err(|_| EfiError::AlreadyStarted)?;

        commands.add_service(&GLOBAL_RSC_HANDLER);

        Ok(())
    }
}
