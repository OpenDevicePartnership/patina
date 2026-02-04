//! Component that provides initialization of ACPI functionality in the core.
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{
    acpi_table::{AcpiTableHeader, AcpiXsdtMetadata},
    alloc::boxed::Box,
    hob::AcpiMemoryHob,
    service::{AcpiProvider, AcpiTableManager},
};
use alloc::vec::Vec;

use core::mem;

use patina::{
    boot_services::{BootServices, StandardBootServices},
    component::{Storage, component},
    uefi_size_to_pages,
};

use patina::{
    component::{
        hob::Hob,
        service::{Service, memory::MemoryManager},
    },
    efi_types::EfiMemoryType,
    error::EfiError,
};

use crate::{
    acpi::STANDARD_ACPI_PROVIDER,
    acpi_protocol::{AcpiGetProtocol, AcpiTableProtocol},
    acpi_table::{AcpiRsdp, AcpiXsdt},
    signature::{
        self, ACPI_HEADER_LEN, ACPI_RESERVED_BYTE, ACPI_RSDP_REVISION, ACPI_XSDT_REVISION, MAX_INITIAL_ENTRIES,
    },
};

/// Initializes the ACPI provider service.
#[derive(Default)]
pub struct AcpiComponent {
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
impl AcpiComponent {
    /// Initializes a new `AcpiComponent`.
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
        storage: &mut Storage,
        boot_services: StandardBootServices,
        acpi_hob: Option<Hob<AcpiMemoryHob>>,
        memory_manager: Service<dyn MemoryManager>,
    ) -> patina::error::Result<()> {
        // Produce the EDKII ACPI protocol interfaces.
        boot_services.install_protocol_interface(None, Box::new(AcpiTableProtocol::new()))?;
        boot_services.install_protocol_interface(None, Box::new(AcpiGetProtocol::new()))?;

        // Initialize the ACPI table info singleton (used for the protocol).
        STANDARD_ACPI_PROVIDER
            .initialize(boot_services, memory_manager.clone())
            .map_err(|_e| EfiError::AlreadyStarted)?;

        // Create and set the XSDT with an initial number of entries.
        let xsdt_size = ACPI_HEADER_LEN + MAX_INITIAL_ENTRIES * mem::size_of::<u64>();

        // The XSDT is always allocated in reclaim memory.
        let allocator = STANDARD_ACPI_PROVIDER
            .memory_manager
            .get()
            .ok_or(EfiError::NotStarted)?
            .get_allocator(EfiMemoryType::ACPIReclaimMemory)
            .map_err(|_e| EfiError::OutOfResources)?;

        // Allocate memory for the initial XSDT buffer.
        let mut xsdt_allocated_bytes = Vec::with_capacity_in(xsdt_size, allocator);

        // Get the raw address for the RSDP.
        let xsdt_addr = xsdt_allocated_bytes.as_ptr() as u64;

        // Create XSDT header.
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
        xsdt_allocated_bytes.extend(xsdt_info.header.hdr_to_bytes());
        // Fill in trailing space with zeros so it is accessible (Vec length != Vec capacity).
        xsdt_allocated_bytes.extend(core::iter::repeat_n(0u8, xsdt_size - ACPI_HEADER_LEN));

        // Set up XSDT data tracking.
        let xsdt_metadata = AcpiXsdtMetadata {
            n_entries: 0,
            max_capacity: MAX_INITIAL_ENTRIES,
            slice: xsdt_allocated_bytes.into_boxed_slice(),
        };
        STANDARD_ACPI_PROVIDER.set_xsdt(xsdt_metadata);

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
        let rsdp_allocation = STANDARD_ACPI_PROVIDER
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

        // SAFETY: `rsdp_ptr` points to valid memory that was just allocated and initialized,
        // and it will live for the 'static lifetime as ACPI memory.
        let rsdp_allocated = unsafe { &mut *rsdp_ptr };

        STANDARD_ACPI_PROVIDER.set_rsdp(rsdp_allocated);

        // Checksum the root tables after setting up.
        STANDARD_ACPI_PROVIDER.checksum_common_tables();

        if let Some(acpi_guid_hob) = acpi_hob {
            let _ = STANDARD_ACPI_PROVIDER.install_tables_from_hob(acpi_guid_hob);
        }

        storage.add_service(&STANDARD_ACPI_PROVIDER);

        // Set up the generic wrapper service for ACPI table management.
        // This allows installation of generic ACPI tables; i.e. install_acpi_table<T>.
        let acpi_provider = storage.get_service::<dyn AcpiProvider>().ok_or(EfiError::NotStarted)?;
        let acpi_service = AcpiTableManager { provider_service: acpi_provider, memory_manager };
        // Register the ACPI table manager service.
        // Consumers of ACPI table management should use this service rather than the provider directly.
        storage.add_service(acpi_service);

        log::trace!("ACPI Provider initialized.");

        Ok(())
    }
}
