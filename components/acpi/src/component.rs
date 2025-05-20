use core::mem;
use uefi_sdk::component::hob::{FromHob, Hob};
use uefi_sdk::component::params::Commands;
use uefi_sdk::component::service::memory::{AllocationOptions, MemoryManager, PageAllocationStrategy};
use uefi_sdk::component::service::Service;
use uefi_sdk::component::IntoComponent;
use uefi_sdk::{
    boot_services::{allocation::AllocType, BootServices, StandardBootServices},
    component::params::Config,
    uefi_size_to_pages,
};

use crate::acpi_protocol::{AcpiSdtProtocol, AcpiTableProtocol};
use crate::acpi_table::AcpiXsdt;
use crate::config::AcpiProviderInit;
use crate::signature::{self, XSDT};
use crate::signature::{
    ACPI_HEADER_LEN, ACPI_RESERVED_BYTE, ACPI_RSDP_REVISION, ACPI_RSDP_TABLE, ACPI_XSDT_REVISION, MAX_INITIAL_ENTRIES,
};
use crate::{acpi::ACPI_TABLE_INFO, acpi_table::AcpiRsdp};

#[derive(IntoComponent, Default)]
pub struct AcpiProviderManager {}

#[derive(Copy, Clone, FromHob)]
#[hob = "9f9a9506-5597-4515-bab6-8bcde784ba87"]
pub struct AcpiMemoryHob {
    pub rsdp_address: u64,
}

impl AcpiProviderManager {
    fn new() -> Self {
        Self {}
    }

    fn entry_point(
        self,
        boot_services: StandardBootServices,
        mut commands: Commands,
        config: Config<AcpiProviderInit>,
        acpi_hob: Option<Hob<AcpiMemoryHob>>,
        memory_manager: Service<dyn MemoryManager>,
    ) -> uefi_sdk::error::Result<()> {
        ACPI_TABLE_INFO.initialize(config.version, config.should_reclaim_memory, boot_services, memory_manager);

        // Create and set the RSDP
        let rsdp_alloc = memory_manager.allocate_zero_pages(
            uefi_size_to_pages!(mem::size_of::<AcpiRsdp>()),
            AllocationOptions::new()
                .with_memory_type(ACPI_TABLE_INFO.memory_type())
                .with_strategy(PageAllocationStrategy::Any),
        )?;
        let rsdp = unsafe { &mut *(rsdp_alloc.into_raw_ptr::<u8>() as *mut AcpiRsdp) };
        ACPI_TABLE_INFO.set_rsdp(rsdp);

        // Create and set the XSDT with an initial number of entries
        let xsdt_alloc = memory_manager.allocate_zero_pages(
            uefi_size_to_pages!(ACPI_HEADER_LEN + mem::size_of::<u64>() * MAX_INITIAL_ENTRIES),
            AllocationOptions::new()
                .with_memory_type(ACPI_TABLE_INFO.memory_type())
                .with_strategy(PageAllocationStrategy::Any),
        )?;
        let xsdt_addr = xsdt_alloc.into_raw_ptr::<u8>();

        let xsdt = unsafe { &mut *(xsdt_addr as *mut AcpiXsdt) };
        ACPI_TABLE_INFO.set_xsdt(xsdt);

        // Initialize RSDP data
        rsdp.signature = ACPI_RSDP_TABLE;
        rsdp.oem_id = config.oem_id;
        rsdp.revision = ACPI_RSDP_REVISION;
        rsdp.length = mem::size_of::<AcpiRsdp>() as u32;
        rsdp.xsdt_address = xsdt_addr as u64;
        rsdp.reserved = [ACPI_RESERVED_BYTE; 3];

        // Initialize XSDT data
        xsdt.signature = signature::XSDT;
        xsdt.length = ACPI_HEADER_LEN as u32;
        xsdt.revision = ACPI_XSDT_REVISION;
        xsdt.oem_id = config.oem_id;
        xsdt.oem_table_id = config.oem_table_id;
        xsdt.creator_id = config.creator_id;
        xsdt.creator_revision = config.creator_revision;
        // First entry of XSDT is always the FADT
        xsdt.length += mem::size_of::<u64>() as u32;

        ACPI_TABLE_INFO.checksum_common_tables().expect("Unable to checksum during ACPI initialization");

        if let Some(acpi_guid_hob) = acpi_hob {
            let _ = ACPI_TABLE_INFO.install_tables_from_hob(acpi_guid_hob);
        }

        commands.add_service(&ACPI_TABLE_INFO);

        Ok(())
    }
}

#[derive(IntoComponent)]
pub struct AcpiSystemTableManager {}

impl AcpiSystemTableManager {
    fn entry_point(self, boot_services: StandardBootServices) -> uefi_sdk::error::Result<()> {
        boot_services.install_protocol_interface(None, Box::new(AcpiTableProtocol::new()))?;
        boot_services.install_protocol_interface(None, Box::new(AcpiSdtProtocol::new()))?;
        Ok(())
    }
}
