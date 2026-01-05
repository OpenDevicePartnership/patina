//! Component that provides initialization of ACPI functionality in the core.

use crate::{
    acpi_table::{AcpiTableHeader, AcpiXsdtMetadata},
    alloc::boxed::Box,
    hob::AcpiMemoryHob,
    service::{AcpiProvider, AcpiTableManager},
};

use core::mem;

use patina::{
    boot_services::{BootServices, StandardBootServices},
    component::component,
    uefi_size_to_pages,
};

use patina::{
    component::{
        hob::Hob,
        params::Commands,
        service::{Service, memory::MemoryManager},
    },
    efi_types::EfiMemoryType,
    error::EfiError,
};

use crate::{
    acpi::ACPI_TABLE_INFO,
    acpi_protocol::{AcpiGetProtocol, AcpiTableProtocol},
    acpi_table::{AcpiRsdp, AcpiXsdt},
    signature::{
        self, ACPI_HEADER_LEN, ACPI_RESERVED_BYTE, ACPI_RSDP_REVISION, ACPI_XSDT_REVISION, MAX_INITIAL_ENTRIES,
    },
};

/// Initializes the ACPI provider service.
#[derive(Default)]
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

#[component]
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

    /// Initializes the ACPI system.
    /// Ignore coverage due to the use of `StandardBootServices`.
    #[coverage(off)]
    fn entry_point(
        self,
        boot_services: StandardBootServices,
        mut commands: Commands,
        acpi_hob: Option<Hob<AcpiMemoryHob>>,
        memory_manager: Service<dyn MemoryManager>,
    ) -> patina::error::Result<()> {
        ACPI_TABLE_INFO.initialize(boot_services, memory_manager).map_err(|_e| EfiError::AlreadyStarted)?;

        // Create and set the XSDT with an initial number of entries.
        let xsdt_size = ACPI_HEADER_LEN + MAX_INITIAL_ENTRIES * mem::size_of::<u64>();

        // Allocate pages directly instead of using Vec
        let xsdt_allocation = ACPI_TABLE_INFO
            .memory_manager
            .get()
            .ok_or(EfiError::NotStarted)?
            .allocate_pages(
                uefi_size_to_pages!(xsdt_size),
                patina::component::service::memory::AllocationOptions::new()
                    .with_memory_type(EfiMemoryType::ACPIReclaimMemory),
            )
            .map_err(|_e| EfiError::OutOfResources)?;

        // Get the raw pointer from the allocation
        let xsdt_ptr = xsdt_allocation.into_raw_ptr().ok_or(EfiError::OutOfResources)?;
        let xsdt_addr = xsdt_ptr as u64;

        // Create XSDT header
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

        // Write the XSDT header to the allocated memory.
        let header_bytes = xsdt_info.header.hdr_to_bytes();
        // SAFETY: `xsdt_ptr` is valid for writes of size `xsdt_size`.
        unsafe {
            core::ptr::copy_nonoverlapping(header_bytes.as_ptr(), xsdt_ptr, ACPI_HEADER_LEN);
            // Zero out the rest of the allocated space.
            core::ptr::write_bytes(xsdt_ptr.add(ACPI_HEADER_LEN), 0, xsdt_size - ACPI_HEADER_LEN);
        }

        // Convert the raw pointer into a Box<[u8], &'static dyn Allocator>.
        // We need to create a slice from the raw pointer and then box it with the allocator.
        // SAFETY: The XSDT was allocated above and should be valid for `xsdt_size` bytes.
        let xsdt_slice: &'static mut [u8] = unsafe { core::slice::from_raw_parts_mut(xsdt_ptr, xsdt_size) };

        let xsdt_metadata = AcpiXsdtMetadata { n_entries: 0, max_capacity: MAX_INITIAL_ENTRIES, slice: xsdt_slice };
        ACPI_TABLE_INFO.set_xsdt(xsdt_metadata);

        // Set up initial values for the RSDP, including XSDT address.
        // Fields preceded with an underscore are for unsupported ACPI version 1.0.
        let rsdp_data = AcpiRsdp {
            signature: signature::ACPI_RSDP_TABLE,
            checksum: 0,
            oem_id: self.oem_id,
            revision: ACPI_RSDP_REVISION,
            _rsdt_address: 0,
            length: mem::size_of::<AcpiRsdp>() as u32, // RSDP size is fixed for ACPI 2.0+.
            xsdt_address: xsdt_addr,
            extended_checksum: 0,
            reserved: [ACPI_RESERVED_BYTE; 3],
        };

        // Allocate memory for the RSDP using allocate_pages
        let rsdp_size = mem::size_of::<AcpiRsdp>();
        let rsdp_allocation = ACPI_TABLE_INFO
            .memory_manager
            .get()
            .ok_or(EfiError::NotStarted)?
            .allocate_pages(
                uefi_size_to_pages!(rsdp_size),
                patina::component::service::memory::AllocationOptions::new()
                    .with_memory_type(EfiMemoryType::ACPIReclaimMemory),
            )
            .map_err(|_e| EfiError::OutOfResources)?;

        // Get the raw pointer from the allocation
        let rsdp_ptr = rsdp_allocation.into_raw_ptr().ok_or(EfiError::OutOfResources)?;

        // Write the RSDP data to the allocated memory
        // SAFETY: `rsdp_ptr` is valid for writes of size `rsdp_size`; the RSDP has a well-defined layout.
        unsafe {
            core::ptr::write(rsdp_ptr, rsdp_data);
        }

        // Convert the raw pointer into a Box<AcpiRsdp, &'static dyn Allocator>.
        // SAFETY: The allocated memory is valid for the lifetime of the ACPI_TABLE_INFO.
        let rsdp_allocated = unsafe { Box::leak(Box::from_raw(rsdp_ptr)) };

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
#[derive(Default)]
pub struct AcpiSystemProtocolManager {}

#[component]
impl AcpiSystemProtocolManager {
    /// Initializes a new `AcpiSystemProtocolManager`.
    pub fn new() -> Self {
        Self {}
    }

    /// Initializes the ACPI protocols.
    /// Ignore coverage due to the use of `StandardBootServices`.
    #[coverage(off)]
    fn entry_point(self, boot_services: StandardBootServices) -> patina::error::Result<()> {
        boot_services.install_protocol_interface(None, Box::new(AcpiTableProtocol::new()))?;
        boot_services.install_protocol_interface(None, Box::new(AcpiGetProtocol::new()))?;
        Ok(())
    }
}

/// Initializes the ACPI table manager service.
/// This services wraps `AcpiProvider` and allows for generic retrieval of tables.
#[derive(Default)]
pub struct GenericAcpiManager {}

#[component]
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
    ) -> patina::error::Result<()> {
        let acpi_service = AcpiTableManager { provider_service: acpi_provider, memory_manager };

        commands.add_service(acpi_service);

        Ok(())
    }
}
