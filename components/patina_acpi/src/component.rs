//! Component that provides initialization of ACPI functionality in the core.

use crate::acpi_table::{AcpiTable, AcpiTableHeader, AcpiXsdtMetadata};
use crate::alloc::boxed::Box;
use crate::hob::AcpiMemoryHob;
use crate::service::{AcpiProvider, AcpiTableManager};

use core::mem;

use alloc::vec::Vec;
use patina_sdk::boot_services::{BootServices, StandardBootServices};

use patina_sdk::component::{
    IntoComponent,
    hob::Hob,
    params::Commands,
    service::{Service, memory::MemoryManager},
};
use patina_sdk::efi_types::EfiMemoryType;
use patina_sdk::error::EfiError;

use crate::{
    acpi::ACPI_TABLE_INFO,
    acpi_protocol::{AcpiSdtProtocol, AcpiTableProtocol},
    acpi_table::{AcpiRsdp, AcpiXsdt},
    signature::{
        self, ACPI_HEADER_LEN, ACPI_RESERVED_BYTE, ACPI_RSDP_REVISION, ACPI_XSDT_REVISION, MAX_INITIAL_ENTRIES,
    },
};

/// Initializes the ACPI provider service.
#[derive(IntoComponent, Default)]
pub struct AcpiProviderManager {
    /// Platform vendor.
    pub oem_id: [u8; 6],
    /// Product variant for platform vendor.
    pub oem_table_id: [u8; 8],
    /// Platform edition (OEM-defined). Not to be confused with ACPI revision.
    pub oem_revision: u32,
    /// ID of compiler used to generate the ACPI table.
    pub creator_id: u32,
    /// Version of the tool used to generate the ACPI table.
    pub creator_revision: u32,
}

impl AcpiProviderManager {
    /// Initializes a new `AcpiProviderManager`.
    pub fn new(
        oem_id: [u8; 6],
        oem_table_id: [u8; 8],
        oem_revision: u32,
        creator_id: u32,
        creator_revision: u32,
    ) -> Self {
        Self { oem_id, oem_table_id, oem_revision, creator_id, creator_revision }
    }

    fn entry_point(
        self,
        boot_services: StandardBootServices,
        mut commands: Commands,
        acpi_hob: Option<Hob<AcpiMemoryHob>>,
        memory_manager: Service<dyn MemoryManager>,
    ) -> patina_sdk::error::Result<()> {
        ACPI_TABLE_INFO.initialize(boot_services, memory_manager).map_err(|_e| EfiError::AlreadyStarted)?;

        // Both XSDT and RSDP are always in reclaim memory.
        let allocator = ACPI_TABLE_INFO
            .memory_manager
            .get()
            .ok_or(EfiError::NotStarted)?
            .get_allocator(EfiMemoryType::ACPIReclaimMemory)
            .map_err(|_e| EfiError::OutOfResources)?;

        // Create and set the XSDT with an initial number of entries.
        let xsdt_capacity = ACPI_HEADER_LEN + MAX_INITIAL_ENTRIES * mem::size_of::<u64>();
        let mut xsdt_allocated_bytes = Vec::with_capacity_in(xsdt_capacity, allocator);
        let xsdt_info = AcpiXsdt {
            header: AcpiTableHeader {
                signature: signature::XSDT,
                length: ACPI_HEADER_LEN as u32, // XSDT starts off with no entries
                revision: ACPI_XSDT_REVISION,
                checksum: 0,
                oem_id: self.oem_id,
                oem_table_id: self.oem_table_id,
                oem_revision: self.oem_revision,
                creator_id: self.creator_id,
                creator_revision: self.creator_revision,
            },
        };
        // Fill in XSDT data.
        let header_bytes = xsdt_info.header.hdr_to_bytes();
        xsdt_allocated_bytes.extend(header_bytes);
        // Fill in trailing space with zeros so it is accessible (Vec length != Vec capacity).
        xsdt_allocated_bytes.extend(core::iter::repeat(0u8).take(xsdt_capacity - ACPI_HEADER_LEN));

        // // Get pointer to the XSDT in memory for RSDP and metadata.
        let xsdt_ptr = xsdt_allocated_bytes.as_mut_ptr();
        let xsdt_addr = xsdt_ptr as u64;
        let xsdt_metadata = AcpiXsdtMetadata {
            n_entries: 0,
            max_capacity: MAX_INITIAL_ENTRIES,
            slice: xsdt_allocated_bytes.into_boxed_slice(),
        };
        ACPI_TABLE_INFO.set_xsdt(xsdt_metadata);

        // Set up initial values for the RSDP, including XSDT address.
        // Fields preceded with an underscore are for unsupported ACPI version 1.0.
        let rsdp_data = AcpiRsdp {
            signature: signature::ACPI_RSDP_TABLE,
            _checksum: 0,
            oem_id: self.oem_id,
            revision: ACPI_RSDP_REVISION,
            _rsdt_address: 0,
            length: mem::size_of::<AcpiRsdp>() as u32, // RSDP size is fixed for ACPI 2.0+.
            xsdt_address: xsdt_addr,
            extended_checksum: 0,
            reserved: [ACPI_RESERVED_BYTE; 3],
        };

        // Allocate memory for the RSDP.
        let rsdp_allocated = unsafe {
            AcpiTable::new(rsdp_data, ACPI_TABLE_INFO.memory_manager.get().ok_or(EfiError::NotStarted)?)
                .map_err(|_e| EfiError::InvalidParameter)?
        };
        ACPI_TABLE_INFO.set_rsdp(rsdp_allocated);

        // Checksum the root tables after setting up.
        ACPI_TABLE_INFO.checksum_common_tables().map_err(|_e| EfiError::NotStarted)?;

        if let Some(acpi_guid_hob) = acpi_hob {
            let _ = ACPI_TABLE_INFO.install_tables_from_hob(acpi_guid_hob);
        }

        commands.add_service(&ACPI_TABLE_INFO);

        Ok(())
    }
}

/// Produces EDKII ACPI protocols.
#[derive(IntoComponent)]
pub struct AcpiSystemProtocolManager {}

impl AcpiSystemProtocolManager {
    /// Initializes a new `AcpiSystemProtocolManager`.
    pub fn new() -> Self {
        Self {}
    }

    fn entry_point(self, boot_services: StandardBootServices) -> patina_sdk::error::Result<()> {
        boot_services.install_protocol_interface(None, Box::new(AcpiTableProtocol::new()))?;
        boot_services.install_protocol_interface(None, Box::new(AcpiSdtProtocol::new()))?;
        Ok(())
    }
}

/// Initializes the ACPI table manager service.
/// This services wraps `AcpiProvider` and allows for generic retrieval of tables.
#[derive(IntoComponent, Default)]
pub struct GenericAcpiManager {}

impl GenericAcpiManager {
    /// Initializes a new `GenericAcpiManager`.
    pub fn new() -> Self {
        Self {}
    }

    fn entry_point(
        self,
        mut commands: Commands,
        acpi_provider: Service<dyn AcpiProvider>,
        memory_manager: Service<dyn MemoryManager>,
    ) -> patina_sdk::error::Result<()> {
        let acpi_service = AcpiTableManager { provider_service: acpi_provider, memory_manager };

        commands.add_service(acpi_service);

        Ok(())
    }
}
