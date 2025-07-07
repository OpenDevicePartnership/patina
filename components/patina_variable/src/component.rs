use std::process::Command;

use patina_sdk::{
    component::{params::Commands, service::Service, IntoComponent},
    runtime_services::{self, StandardRuntimeServices},
};

use crate::{
    service::{VariableServices, VariableStorage},
    variable::SimpleVariableStorage,
};

/// Sets up variable storage provider.
#[derive(IntoComponent)]
struct VariableStorageInitManager {}

impl VariableStorageInitManager {
    pub fn new() -> Self {
        Self {}
    }

    pub fn entry_point(
        self,
        runtime_services: StandardRuntimeServices,
        mut commands: Commands,
    ) -> patina_sdk::error::Result<()> {
        let simple_vars = SimpleVariableStorage { runtime_services };
        commands.add_service(simple_vars);
        Ok(())
    }
}

/// Sets up variable storage services.
#[derive(IntoComponent)]
pub struct VariableServiceInitManager {}

impl VariableServiceInitManager {
    pub fn new() -> Self {
        Self {}
    }

    fn entry_point(
        self,
        mut commands: Commands,
        storage_service: Service<dyn VariableStorage>,
    ) -> patina_sdk::error::Result<()> {
        let variable_service = VariableServices { storage_provider: storage_service };
        commands.add_service(variable_service);
        Ok(())
    }
}
