//! Patina Performance Property Publisher
//!
//! Publishes the platform's performance counter properties (frequency and counter range) as a UEFI configuration
//! table, so an OS or later component can convert recorded performance ticks into wall-clock time.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::boxed::Box;

use patina::{
    component::{
        component,
        service::{
            Service,
            perf_timer::ArchTimerFunctionality,
            performance::PerformanceManager,
            uefi_services::config_table::{ConfigurationTableServices, ConfigurationTableServicesExt},
        },
    },
    error::EfiError,
    performance::measurement::PerformanceProperty,
};

/// Publishes the [`PerformanceProperty`] configuration table describing the platform's performance counter.
///
/// ## Example Usage
///
/// ```rust
/// use patina_performance::component::property::*;
///
/// let component = PropertyPublisher::new();
/// ```
#[derive(Default)]
pub struct PropertyPublisher;

#[component]
impl PropertyPublisher {
    /// Creates a new instance of the component.
    pub const fn new() -> Self {
        Self
    }

    /// Depends on [`PerformanceManager`] only to gate dispatch, this component's own work never calls it. The
    /// DXE Core only publishes [`PerformanceManager`] when performance measurement is enabled, and this table
    /// should only be published in that case too.
    fn entry_point(
        self,
        _performance: Service<dyn PerformanceManager>,
        timer: Service<dyn ArchTimerFunctionality>,
        config_table: Service<dyn ConfigurationTableServices>,
    ) -> Result<(), EfiError> {
        let property = PerformanceProperty::new(timer.perf_frequency(), timer.cpu_count_start(), timer.cpu_count_end());
        let property: &'static PerformanceProperty = Box::leak(Box::new(property));

        config_table.install(property)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patina::component::service::{
        performance::MockPerformanceManager,
        uefi_services::config_table::{ConfigTable, MockConfigurationTableServices},
    };

    struct MockTimer;

    impl ArchTimerFunctionality for MockTimer {
        fn perf_frequency(&self) -> u64 {
            100
        }
        fn cpu_count(&self) -> u64 {
            200
        }
        fn cpu_count_start(&self) -> u64 {
            1
        }
        fn cpu_count_end(&self) -> u64 {
            9999
        }
    }

    fn mock_performance() -> Service<dyn PerformanceManager> {
        Service::mock(Box::new(MockPerformanceManager::new()))
    }

    #[test]
    fn test_property_publisher_entry_point_installs_table() {
        let mut config_table = MockConfigurationTableServices::new();
        config_table
            .expect_install_typed_table()
            .once()
            .withf(|guid, type_id, _table| {
                assert_eq!(guid, &PerformanceProperty::TABLE_GUID);
                assert_eq!(type_id, &core::any::TypeId::of::<PerformanceProperty>());
                true
            })
            .returning(|_, _, _| Ok(()));

        let result = PropertyPublisher::new().entry_point(
            mock_performance(),
            Service::mock(Box::new(MockTimer)),
            Service::mock(Box::new(config_table)),
        );

        assert!(result.is_ok());
    }
}
