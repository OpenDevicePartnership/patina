use crate::acpi_table::{AcpiTableHeader, AcpiXsdtMetadata};
use crate::alloc::boxed::Box;

use core::mem;
use core::ptr::NonNull;

use alloc::vec::Vec;
use patina_sdk::boot_services::{BootServices, StandardBootServices};

use patina_sdk::efi_types::EfiMemoryType;
use patina_sdk::error::EfiError;
use patina_sdk::{
    component::{
        hob::{FromHob, Hob},
        params::{Commands, Config},
        service::{
            memory::{AllocationOptions, MemoryManager},
            Service,
        },
        IntoComponent,
    },
    uefi_size_to_pages,
};

use crate::error::AcpiError;
use crate::{
    acpi::ACPI_TABLE_INFO,
    acpi_protocol::{AcpiSdtProtocol, AcpiTableProtocol},
    acpi_table::{AcpiRsdp, AcpiXsdt},
    config::AcpiProviderInit,
    signature::{
        self, ACPI_HEADER_LEN, ACPI_RESERVED_BYTE, ACPI_RSDP_REVISION, ACPI_RSDP_TABLE, ACPI_XSDT_REVISION,
        MAX_INITIAL_ENTRIES,
    },
};

// Initializes the ACPI provider service
#[derive(IntoComponent, Default)]
pub struct AcpiProviderManager {}

#[derive(Copy, Clone, FromHob)]
#[hob = "9f9a9506-5597-4515-bab6-8bcde784ba87"]
pub struct AcpiMemoryHob {
    pub rsdp_address: u64,
}

impl AcpiProviderManager {
    pub fn new() -> Self {
        Self {}
    }

    fn entry_point(
        self,
        boot_services: StandardBootServices,
        mut commands: Commands,
        config: Config<AcpiProviderInit>,
        acpi_hob: Option<Hob<AcpiMemoryHob>>,
        memory_manager: Service<dyn MemoryManager>,
    ) -> patina_sdk::error::Result<()> {
        ACPI_TABLE_INFO
            .initialize(config.should_reclaim_memory, boot_services, memory_manager)
            .map_err(|_e| EfiError::AlreadyStarted)?;

        // Create the RSDP.
        let allocator = ACPI_TABLE_INFO
            .memory_manager
            .get()
            .ok_or(AcpiError::ProviderNotInitialized)?
            .get_allocator(EfiMemoryType::ACPIReclaimMemory)
            .map_err(|_e| EfiError::OutOfResources)?;
        let rsdp_len = mem::size_of::<AcpiRsdp>(); // Size of RSDP is fixed for version 2.0+.
        let mut rsdp_allocated_bytes = Vec::with_capacity_in(rsdp_len, allocator);

        // Create and set the XSDT with an initial number of entries.
        let xsdt_len = ACPI_HEADER_LEN + MAX_INITIAL_ENTRIES * mem::size_of::<u64>();
        let mut xsdt_allocated_bytes = Vec::with_capacity_in(xsdt_len, allocator);
        // Fill in XSDT data.
        xsdt_allocated_bytes.extend_from_slice(&signature::XSDT.to_le_bytes());
        xsdt_allocated_bytes.extend_from_slice(&(ACPI_HEADER_LEN as u32).to_le_bytes()); // The XSDT starts off with zero entries.
        xsdt_allocated_bytes.extend_from_slice(&ACPI_XSDT_REVISION.to_le_bytes());
        xsdt_allocated_bytes.extend_from_slice(&config.oem_id);
        xsdt_allocated_bytes.extend_from_slice(&config.oem_table_id);
        xsdt_allocated_bytes.extend_from_slice(&config.creator_id.to_le_bytes());
        xsdt_allocated_bytes.extend_from_slice(&config.creator_revision.to_le_bytes());

        // Get pointer to the XSDT in memory for RSDP and metadata.
        let xsdt_ptr = xsdt_allocated_bytes.as_mut_ptr();
        let xsdt_addr = xsdt_ptr as u64;
        let xsdt_header = NonNull::new(xsdt_ptr as *mut AcpiTableHeader).ok_or(EfiError::OutOfResources)?; // Return error if XSDT allocation fails.
        let xsdt_metadata = AcpiXsdtMetadata {
            header: xsdt_header,
            nentries: 0,
            max_capacity: MAX_INITIAL_ENTRIES,
            slice: xsdt_allocated_bytes.into_boxed_slice(),
        };

        // Initialize RSDP data, including XSDT address.
        rsdp_allocated_bytes.extend_from_slice(&signature::ACPI_RSDP_TABLE.to_le_bytes());
        rsdp_allocated_bytes.extend_from_slice(&config.oem_id);
        rsdp_allocated_bytes.extend_from_slice(&ACPI_RSDP_REVISION.to_le_bytes());
        rsdp_allocated_bytes.extend_from_slice(&(rsdp_len as u32).to_le_bytes());
        rsdp_allocated_bytes.extend_from_slice(&xsdt_addr.to_le_bytes());
        rsdp_allocated_bytes.extend_from_slice(&[ACPI_RESERVED_BYTE; 3]);

        ACPI_TABLE_INFO.checksum_common_tables().map_err(|_e| EfiError::NotStarted);

        if let Some(acpi_guid_hob) = acpi_hob {
            let _ = ACPI_TABLE_INFO.install_tables_from_hob(acpi_guid_hob);
        }

        commands.add_service(&ACPI_TABLE_INFO);

        Ok(())
    }
}

// Produces EDKII ACPI protocols
#[derive(IntoComponent)]
pub struct AcpiSystemTableProtocolManager {}

impl AcpiSystemTableProtocolManager {
    fn entry_point(self, boot_services: StandardBootServices) -> patina_sdk::error::Result<()> {
        boot_services.install_protocol_interface(None, Box::new(AcpiTableProtocol::new()))?;
        boot_services.install_protocol_interface(None, Box::new(AcpiSdtProtocol::new()))?;
        Ok(())
    }
}
