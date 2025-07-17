//! ACPI Service Implementations.
//!
//! Implements the ACPI service interface defined in `service.rs`.
//! Supports only ACPI version >= 2.0.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use alloc::collections::btree_map::BTreeMap;
use core::{
    cell::OnceCell,
    ffi::c_void,
    mem::{self},
    slice,
    sync::atomic::{AtomicUsize, Ordering},
};
use spin::rwlock::RwLock;

use patina_sdk::{
    boot_services::{BootServices, StandardBootServices},
    component::{
        hob::Hob,
        service::{IntoService, Service, memory::MemoryManager},
    },
    efi_types::EfiMemoryType,
};

use crate::{
    acpi_table::{AcpiDsdt, AcpiFacs, AcpiFadt, AcpiRsdp, AcpiTable, AcpiTableHeader, AcpiXsdtMetadata},
    alloc::vec,
    alloc::vec::Vec,
    error::AcpiError,
    hob::AcpiMemoryHob,
    service::{AcpiNotifyFn, AcpiProvider, TableKey},
    signature::{self, ACPI_CHECKSUM_OFFSET, ACPI_HEADER_LEN, ACPI_VERSIONS_GTE_2, ACPI_XSDT_ENTRY_SIZE},
};

pub static ACPI_TABLE_INFO: StandardAcpiProvider<StandardBootServices> = StandardAcpiProvider::new_uninit();

/// Standard implementation of ACPI services. The service interface can be found in `service.rs`
#[derive(IntoService)]
#[service(dyn AcpiProvider)]
pub(crate) struct StandardAcpiProvider<B: BootServices + 'static> {
    /// Platform-installed ACPI tables.
    /// If installing a non-standard ACPI table, the platform is responsible for writing its own handler and parser.
    pub(crate) acpi_tables: RwLock<BTreeMap<TableKey, AcpiTable>>,
    /// Stores a monotonically increasing unique table key for installation.
    next_table_key: AtomicUsize,
    /// Stores notify callbacks, which are called upon table installation.
    notify_list: RwLock<Vec<AcpiNotifyFn>>,
    /// Provides boot services.
    pub(crate) boot_services: OnceCell<B>,
    /// Provides memory services.
    pub(crate) memory_manager: OnceCell<Service<dyn MemoryManager>>,
    /// Stores data about the XSDT and its entries.
    xsdt_metadata: RwLock<Option<AcpiXsdtMetadata>>,
}

// SAFETY: `StandardAcpiProvider` does not share any internal references or non-Send types across threads.
// All fields are `Send` or properly synchronized.
unsafe impl<B> Sync for StandardAcpiProvider<B> where B: BootServices + Sync {}

// SAFETY: Access to shared state within `StandardAcpiProvider` is synchronized (via mutexes and atomics)
unsafe impl<B> Send for StandardAcpiProvider<B> where B: BootServices + Send {}

impl<B> StandardAcpiProvider<B>
where
    B: BootServices,
{
    /// Known table keys for system tables.
    const FACS_KEY: TableKey = TableKey(1);
    const DSDT_KEY: TableKey = TableKey(2);
    const RSDP_KEY: TableKey = TableKey(3);
    const FADT_KEY: TableKey = TableKey(4);

    /// The first unused key which can be given to callers of `install_acpi_table`.
    const FIRST_FREE_KEY: usize = 5;

    /// Keys which are not available to be indexed or iterated by an end user.
    /// This includes all system tables except the FADT, which *is* treated like a normally installed table.
    const PRIVATE_SYSTEM_TABLES: [TableKey; 3] = [Self::RSDP_KEY, Self::FACS_KEY, Self::DSDT_KEY];

    /// Creates a new `StandardAcpiProvider` with uninitialized fields.
    /// Attempting to use `StandardAcpiProvider` before initialization will cause a panic.
    pub const fn new_uninit() -> Self {
        Self {
            acpi_tables: RwLock::new(BTreeMap::new()),
            next_table_key: AtomicUsize::new(Self::FIRST_FREE_KEY),
            notify_list: RwLock::new(vec![]),
            boot_services: OnceCell::new(),
            memory_manager: OnceCell::new(),
            xsdt_metadata: RwLock::new(None),
        }
    }

    /// Fills in `StandardAcpiProvider` fields at runtime.
    /// This function must be called before any attempts to use `StandardAcpiProvider`, or any usages will fail.
    /// Attempting to initialize a single `StandardAcpiProvider` instance more than once will also cause a failure.
    pub fn initialize(&self, bs: B, memory_manager: Service<dyn MemoryManager>) -> Result<(), AcpiError>
    where
        B: BootServices,
    {
        if self.boot_services.set(bs).is_err() {
            return Err(AcpiError::BootServicesAlreadyInitialized);
        }
        if self.memory_manager.set(memory_manager).is_err() {
            return Err(AcpiError::MemoryManagerAlreadyInitialized);
        }
        Ok(())
    }

    /// Sets up tracking for the RSDP internally.
    pub fn set_rsdp(&self, rsdp_table: AcpiTable) {
        self.acpi_tables.write().insert(Self::RSDP_KEY, rsdp_table);
    }

    /// Sets up tracking for the XSDT internally.
    pub fn set_xsdt(&self, xsdt_data: AcpiXsdtMetadata) {
        let mut write_guard = self.xsdt_metadata.write();
        *write_guard = Some(xsdt_data);
    }
}

/// Implementations of ACPI services.
/// The following functions are called on the Rust side by the `AcpiTableManager` service.
/// They also provide implementations for the C ACPI protocols.
/// For more information on operation and interfaces, see `service.rs`.
impl<B> AcpiProvider for StandardAcpiProvider<B>
where
    B: BootServices,
{
    fn install_acpi_table(&self, table: AcpiTable) -> Result<TableKey, AcpiError> {
        // Based on the ACPI spec, implementations can chose to disallow duplicates or incorporate them into existing installed tables.
        // For simplicity, this implementation rejects attempts to install a new XSDT when one already exists.
        if table.signature() == signature::XSDT {
            return Err(AcpiError::XsdtAlreadyInstalled);
        }

        let table_key = match table.signature() {
            signature::FACS => self.install_facs(table)?,
            signature::FADT => self.install_fadt(table)?,
            signature::DSDT => self.install_dsdt(table)?,
            _ => self.install_standard_table(table)?,
        };

        self.publish_tables()?;
        self.notify_acpi_list(table_key)?;
        Ok(table_key)
    }

    fn uninstall_acpi_table(&self, table_key: TableKey) -> Result<(), AcpiError> {
        self.remove_table_from_list(table_key)?;
        self.publish_tables()?;
        Ok(())
    }

    fn get_acpi_table(&self, table_key: TableKey) -> Result<AcpiTable, AcpiError> {
        self.acpi_tables.read().get(&table_key).cloned().ok_or(AcpiError::InvalidTableKey)
    }

    fn register_notify(&self, should_register: bool, notify_fn: AcpiNotifyFn) -> Result<(), AcpiError> {
        if should_register {
            self.notify_list.write().push(notify_fn);
        } else {
            let found_pos = self.notify_list.read().iter().position(|x| core::ptr::fn_addr_eq(*x, notify_fn));
            if let Some(pos) = found_pos {
                self.notify_list.write().remove(pos);
            } else {
                return Err(AcpiError::InvalidNotifyUnregister);
            }
        }

        Ok(())
    }

    /// Iterate over installed tables in the ACPI table list.
    /// The RSDP, FACS, and DSDT are not considered part of the list of installed tables and should not be iterated over.
    fn iter_tables(&self) -> Vec<AcpiTable> {
        self.acpi_tables
            .read()
            .iter()
            .filter(|(k, _)| !Self::PRIVATE_SYSTEM_TABLES.contains(k))
            .map(|(_, v)| *v)
            .collect()
    }
}

impl<B> StandardAcpiProvider<B>
where
    B: BootServices,
{
    fn install_facs(&self, facs_info: AcpiTable) -> Result<TableKey, AcpiError> {
        // Update the FADT's address pointer to the FACS.
        if let Some(fadt_table) = self.acpi_tables.write().get_mut(&Self::FADT_KEY) {
            // SAFETY: We verify the table's signature before calling `install_facs`.
            let facs_addr = unsafe { facs_info.as_ref::<AcpiFacs>() } as *const AcpiFacs as u64;
            unsafe { fadt_table.as_mut::<AcpiFadt>().set_x_firmware_ctrl(facs_addr) };
        }

        self.acpi_tables.write().insert(Self::FACS_KEY, facs_info);

        self.checksum_common_tables()?;

        // FACS is not added to the list of installed tables in the XSDT.
        // We use a default key for the FACS for easy retrieval and modification internally.
        // This key is opaque to the user and its value does not matter, as long as it is unique.
        Ok(Self::FACS_KEY)
    }

    /// Retrieves a specific entry from the XSDT.
    /// The XSDT has a standard ACPI header followed by a variable-length list of entries in ACPI memory.
    fn get_xsdt_entry_from_hob(idx: usize, xsdt_start_ptr: *const u8, xsdt_len: usize) -> Result<u64, AcpiError> {
        // Offset from the start of the XSDT in memory
        // Entries directly follow the header
        let offset = ACPI_HEADER_LEN + idx * core::mem::size_of::<u64>();
        // Make sure we only read valid entries in the XSDT
        if offset >= xsdt_len {
            return Err(AcpiError::InvalidXsdtEntry);
        }
        // SAFETY: the caller must pass in a valid pointer to an XSDT
        // Find the entry at `offset` and read the value (which is a u64 address)
        let entry_addr = unsafe {
            let entry_ptr = xsdt_start_ptr.add(offset) as *const u64;
            core::ptr::read_unaligned(entry_ptr)
        };

        Ok(entry_addr)
    }

    /// Extracts the XSDT address after performing validation on the RSDP and XSDT.
    fn get_xsdt_address_from_rsdp(rsdp_address: u64) -> Result<u64, AcpiError> {
        if rsdp_address == 0 {
            return Err(AcpiError::NullRsdpFromHob);
        }

        // SAFETY: The RSDP address has been validated as non-null
        let rsdp: &AcpiRsdp = unsafe { &*(rsdp_address as *const AcpiRsdp) };
        if rsdp.signature != signature::ACPI_RSDP_TABLE {
            return Err(AcpiError::InvalidSignature);
        }

        if rsdp.xsdt_address == 0 {
            return Err(AcpiError::XsdtNotInitializedFromHob);
        }

        // Read the header to validate the XSDT signature is valid.
        // SAFETY: `xsdt_address` has been validated to be non-null.
        let xsdt_header = rsdp.xsdt_address as *const AcpiTableHeader;
        if (unsafe { *xsdt_header }).signature != signature::XSDT {
            return Err(AcpiError::InvalidSignature);
        }

        // SAFETY: We validate that the XSDT is non-null and contains the right signature.
        let xsdt_ptr = rsdp.xsdt_address as *const AcpiTableHeader;
        let xsdt = unsafe { &*(xsdt_ptr) };

        if xsdt.length < ACPI_HEADER_LEN as u32 {
            return Err(AcpiError::XsdtInvalidLengthFromHob);
        }

        Ok(rsdp.xsdt_address)
    }

    /// Installs tables pointed to by the FADT if provided in the HOB list.
    fn install_fadt_tables_from_hob(&self, fadt: &AcpiFadt) -> Result<(), AcpiError> {
        // SAFETY: we assume the FADT set up in the HOB points to a valid FACS if the pointer is non-null.
        if fadt.x_firmware_ctrl() != 0 {
            // SAFETY: The FACS address has been checked to be non-null.
            // The caller must ensure that the FACS in the HOB is valid.
            let facs_from_ptr = unsafe { *(fadt.x_firmware_ctrl() as *const AcpiFacs) };
            if facs_from_ptr.signature != signature::FACS {
                return Err(AcpiError::InvalidSignature);
            }

            let facs_table = unsafe {
                AcpiTable::new(facs_from_ptr, self.memory_manager.get().ok_or(AcpiError::ProviderNotInitialized)?)?
            };
            self.install_facs(facs_table)?;
        }

        if fadt.x_dsdt() != 0 {
            // SAFETY: The DSDT address has been checked to be non-null.
            // The caller must ensure that the DSDT in the HOB is valid.
            let dsdt_from_ptr = unsafe { *(fadt.x_dsdt() as *const AcpiDsdt) };
            if dsdt_from_ptr.header.signature != signature::DSDT {
                return Err(AcpiError::InvalidSignature);
            }

            let dsdt_table = unsafe {
                AcpiTable::new(dsdt_from_ptr, self.memory_manager.get().ok_or(AcpiError::ProviderNotInitialized)?)?
            };
            self.install_dsdt(dsdt_table)?;
        }

        Ok(())
    }

    /// Installs tables pointed to by the ACPI memory HOB.
    pub fn install_tables_from_hob(&self, acpi_hob: Hob<AcpiMemoryHob>) -> Result<(), AcpiError> {
        let xsdt_address = Self::get_xsdt_address_from_rsdp(acpi_hob.rsdp_address)?;
        let xsdt_ptr = xsdt_address as *const AcpiTableHeader;

        // SAFETY: `get_xsdt_address_from_rsdp` should perform necessary validations on XSDT.
        let xsdt_length = (unsafe { *xsdt_ptr }).length;

        let entries = (xsdt_length as usize - ACPI_HEADER_LEN) / mem::size_of::<u64>();
        for i in 0..entries {
            // Find the address value of the next XSDT entry.
            let entry_addr = Self::get_xsdt_entry_from_hob(i, xsdt_ptr as *const u8, xsdt_length as usize)?;

            // Each entry points to a table.
            // The type of the table is unknown at this point, since we're installing from a raw pointer.
            // SAFETY: The caller must ensure that the XSDT in the HOB points to valid table entries.
            let tbl_header = unsafe { *(entry_addr as *const AcpiTableHeader) };
            // Because we are installing from raw pointers, information about the type of the table cannot be extracted.
            let mut table = unsafe {
                AcpiTable::new(tbl_header, self.memory_manager.get().ok_or(AcpiError::ProviderNotInitialized)?)?
            };

            self.install_standard_table(table)?;

            // If this table points to other system tables, install them too
            if tbl_header.signature == signature::FADT {
                // SAFETY: assuming the XSDT entry is written correctly, this points to a valid ACPI table
                // and the signature has been verified to match that of the FADT
                let fadt = unsafe { &*(entry_addr as *const AcpiFadt) };
                self.install_fadt_tables_from_hob(fadt)?;
            }

            table.update_checksum(ACPI_CHECKSUM_OFFSET)?;
        }
        self.publish_tables()?;
        Ok(())
    }

    /// Allocates memory for the FADT and adds it  to the list of installed tables
    fn install_fadt(&self, mut fadt_info: AcpiTable) -> Result<TableKey, AcpiError> {
        if self.acpi_tables.read().get(&Self::FADT_KEY).is_some() {
            // FADT already installed. By spec, only one copy of the FADT should ever be installed, and it cannot be replaced.
            return Err(AcpiError::FadtAlreadyInstalled);
        }

        // If the FACS is already installed, update the FADT's x_firmware_ctrl field.
        // If not, it will be updated when the FACS is installed.
        if let Some(facs) = self.acpi_tables.read().get(&Self::FACS_KEY) {
            unsafe { fadt_info.as_mut::<AcpiFadt>() }.inner.x_firmware_ctrl = facs.as_ptr() as u64;
        }

        // If the DSDT is already installed, update the FACP's x_dsdt field.
        // If not, it will be updated when the DSDT is installed.
        if let Some(dsdt) = self.acpi_tables.read().get(&Self::DSDT_KEY) {
            unsafe { fadt_info.as_mut::<AcpiFadt>() }.inner.x_dsdt = dsdt.as_ptr() as u64;
        }

        // The FADT is stored in the XSDT like a normal table. Add the FADT to the XSDT.
        let physical_addr = fadt_info.as_ptr() as u64;
        self.add_entry_to_xsdt(physical_addr)?;

        // Checksum the FADT after modifying fields.
        fadt_info.update_checksum(ACPI_CHECKSUM_OFFSET)?;

        // Add the FADT to the list of installed tables.
        self.acpi_tables.write().insert(Self::FADT_KEY, fadt_info);

        // RSDP derives OEM ID from FADT.
        if let Some(rsdp) = self.acpi_tables.write().get_mut(&Self::RSDP_KEY) {
            unsafe { rsdp.as_mut::<AcpiRsdp>() }.oem_id = fadt_info.header().oem_id;
        }

        // XSDT derives OEM information from FADT.
        if let Some(ref mut xsdt_data) = *self.xsdt_metadata.write() {
            xsdt_data.set_oem_id(fadt_info.header().oem_id);
            xsdt_data.set_oem_table_id(fadt_info.header().oem_table_id);
            xsdt_data.set_oem_revision(fadt_info.header().oem_revision);
        }

        // Checksum root tables after modifying fields.
        self.checksum_common_tables()?;

        self.acpi_tables.write().insert(Self::FADT_KEY, fadt_info);

        Ok(Self::FADT_KEY)
    }

    /// Installs the DSDT.
    /// The DSDT is not added to the list of XSDT entries.
    fn install_dsdt(&self, mut dsdt_info: AcpiTable) -> Result<TableKey, AcpiError> {
        // If the FADT is already installed, update the FACP's x_dsdt field.Add commentMore actions
        // If not, it will be updated when the FACP is installed.
        if let Some(facp) = self.acpi_tables.write().get_mut(&Self::FADT_KEY) {
            unsafe { facp.as_mut::<AcpiFadt>() }.inner.x_dsdt = dsdt_info.as_ptr() as u64;
        };

        dsdt_info.update_checksum(ACPI_CHECKSUM_OFFSET)?;

        self.acpi_tables.write().insert(Self::DSDT_KEY, dsdt_info);

        // The DSDT is not present in the list of XSDT entries.
        // We use a default key for the FACS for easy retrieval and modification internally.
        // This key is opaque to the user and its value does not matter, as long as it is unique.
        Ok(Self::DSDT_KEY)
    }

    /// Allocates ACPI memory for a new table and adds the table to the list of installed ACPI tables.
    pub(crate) fn install_standard_table(&self, mut table_info: AcpiTable) -> Result<TableKey, AcpiError> {
        // By spec, table keys can be assigned in any manner as long as they are unique for each newly installed table.
        // For simplicity, we use a monotonically increasing key.
        let curr_key = TableKey(self.next_table_key.fetch_add(1, Ordering::AcqRel));

        // Add the table to the internal hashmap of installed tables.
        self.acpi_tables.write().insert(curr_key, table_info);

        // Recalculate checksum for the newly installed table.
        table_info.update_checksum(ACPI_CHECKSUM_OFFSET)?;

        // Get the physical address of the table for the XSDT entry.
        let physical_addr = table_info.as_ptr() as u64;
        self.add_entry_to_xsdt(physical_addr)?;

        // Since XSDT was modified, recalculate checksum for root tables.
        self.checksum_common_tables()?;
        Ok(curr_key)
    }

    /// Adds an address entry to the XSDT.
    fn add_entry_to_xsdt(&self, new_table_addr: u64) -> Result<(), AcpiError> {
        let mut max_capacity = 0;
        let mut curr_capacity = 0;

        if let Some(ref xsdt_data) = *self.xsdt_metadata.read() {
            // If the XSDT is already initialized, we can use its metadata.
            max_capacity = xsdt_data.max_capacity;
            curr_capacity = xsdt_data.n_entries;
        }

        // XSDT is full. Reallocate buffer.
        if curr_capacity >= max_capacity {
            self.reallocate_xsdt()?;
        }

        if let Some(ref mut xsdt_data) = *self.xsdt_metadata.write() {
            // Next entry goes after header + existing address entries.
            let entry_offset = ACPI_HEADER_LEN + xsdt_data.n_entries * ACPI_XSDT_ENTRY_SIZE;
            // Fill in the bytes of the new address entry.
            xsdt_data.slice[entry_offset..entry_offset + ACPI_XSDT_ENTRY_SIZE]
                .copy_from_slice(&new_table_addr.to_le_bytes());

            // Increase XSDT length by one entry.
            xsdt_data.set_length(xsdt_data.get_length()? + (ACPI_XSDT_ENTRY_SIZE as u32));
            // Increase XSDT entry count.
            xsdt_data.n_entries += 1;
        }

        // Checksum the XSDT after modifying it.
        self.checksum_common_tables()?;

        Ok(())
    }

    /// Allocates a new, larger memory space for the XSDT when it is full and relocates all entries to the newly allocated memory.
    fn reallocate_xsdt(&self) -> Result<(), AcpiError> {
        if let Some(ref mut xsdt_data) = *self.xsdt_metadata.write() {
            // Calculate current size of the XSDT.
            let num_bytes_original = xsdt_data.get_length()? as usize;
            let curr_capacity = xsdt_data.max_capacity;
            // Use a geometric resizing strategy.
            let new_capacity = curr_capacity * 2;
            xsdt_data.max_capacity = new_capacity;
            // Calculates bytes needed for new number of entries, including the XSDT's header.
            let num_bytes_new = ACPI_HEADER_LEN + new_capacity * ACPI_XSDT_ENTRY_SIZE;

            // The XSDT is always allocated in reclaim memory.
            let allocator = self
                .memory_manager
                .get()
                .ok_or(AcpiError::ProviderNotInitialized)?
                .get_allocator(EfiMemoryType::ACPIReclaimMemory)
                .map_err(|_e| AcpiError::AllocationFailed)?;
            let mut xsdt_allocated_bytes = Vec::with_capacity_in(num_bytes_new, allocator);
            // Copy over existing data.
            xsdt_allocated_bytes.extend_from_slice(&xsdt_data.slice);
            // Fill in trailing space with zeros so it is accessible (Vec length != Vec capacity).
            xsdt_allocated_bytes.extend(core::iter::repeat(0u8).take(num_bytes_new - num_bytes_original));

            // Update the RSDP with the new XSDT address.
            let xsdt_ptr = xsdt_allocated_bytes.as_mut_ptr();
            let xsdt_addr = xsdt_ptr as u64;
            if let Some(rsdp) = self.acpi_tables.write().get_mut(&Self::RSDP_KEY) {
                unsafe { rsdp.as_mut::<AcpiRsdp>() }.xsdt_address = xsdt_addr;
            }

            // Point to the newly allocated data.
            xsdt_data.slice = xsdt_allocated_bytes.into_boxed_slice();
        }

        Ok(())
    }

    /// Removes a table from the list of installed tables.
    fn remove_table_from_list(&self, table_key: TableKey) -> Result<(), AcpiError> {
        let table_for_key = self.acpi_tables.write().remove(&table_key);

        if let Some(table_to_delete) = table_for_key {
            let table_addr = table_to_delete.as_ptr() as u64;
            self.delete_table(table_addr, table_to_delete.signature())
        } else {
            // No table found with the given key.
            Err(AcpiError::InvalidTableKey)
        }
    }

    /// Deletes a table from the list of installed tables and frees its memory.
    fn delete_table(&self, physical_addr: u64, signature: u32) -> Result<(), AcpiError> {
        match signature {
            signature::FADT => {
                self.acpi_tables.write().remove(&Self::FADT_KEY);
            }
            signature::FACS => {
                // Clear out the FACS pointer in the FADT.
                if let Some(fadt_table) = self.acpi_tables.write().get_mut(&Self::FADT_KEY) {
                    unsafe { fadt_table.as_mut::<AcpiFadt>() }.set_x_firmware_ctrl(0);
                    fadt_table.update_checksum(ACPI_CHECKSUM_OFFSET)?;
                }

                self.acpi_tables.write().remove(&Self::FACS_KEY);
            }
            signature::DSDT => {
                // Clear out the FACS pointer in the FADT.
                if let Some(fadt_table) = self.acpi_tables.write().get_mut(&Self::FADT_KEY) {
                    unsafe { fadt_table.as_mut::<AcpiFadt>() }.set_x_dsdt(0);
                    fadt_table.update_checksum(ACPI_CHECKSUM_OFFSET)?;
                }

                self.acpi_tables.write().remove(&Self::DSDT_KEY);
            }
            _ => {
                self.remove_table_from_xsdt(physical_addr)?;
            }
        }

        Ok(())
    }

    /// Removes an address entry from the XSDT when a table is uninstalled.
    fn remove_table_from_xsdt(&self, table_address: u64) -> Result<(), AcpiError> {
        if let Some(ref mut xsdt_data) = *self.xsdt_metadata.write() {
            // Calculate where entries are in the slice.
            let entries_n_bytes = ACPI_XSDT_ENTRY_SIZE * xsdt_data.n_entries;
            let entries_bytes = xsdt_data
                .slice
                .get(ACPI_HEADER_LEN..ACPI_HEADER_LEN + entries_n_bytes)
                .ok_or(AcpiError::XsdtOverflow)?;
            // Look for the corresponding entry.
            let index_opt: Option<usize> = entries_bytes
                .chunks_exact(ACPI_XSDT_ENTRY_SIZE)
                .position(|chunk| u64::from_le_bytes(chunk.try_into().unwrap()) == table_address);

            if let Some(idx) = index_opt {
                let start_ptr = ACPI_HEADER_LEN + idx * ACPI_XSDT_ENTRY_SIZE; // Find where the target entry starts.
                let end_ptr = ACPI_HEADER_LEN + xsdt_data.n_entries * ACPI_XSDT_ENTRY_SIZE; // Find where the XSDT ends.

                // Shift all entries after the one being removed to the left.
                // [.. before .. | target | <- .. after .. ]
                // becomes [.. before .. | .. after.. ]
                xsdt_data.slice.copy_within(start_ptr + ACPI_XSDT_ENTRY_SIZE..end_ptr, start_ptr);

                // Decrement entries.
                xsdt_data.n_entries -= 1;

                // Zero out the end of the XSDT.
                // (After removing and shifting all entries, there is one extra slot at the end.)
                // This is not technically necessary for correctness but is good practice for consistency.
                xsdt_data.slice[end_ptr - ACPI_XSDT_ENTRY_SIZE..end_ptr].iter_mut().for_each(|b| *b = 0);

                // Decrease XSDT length.
                xsdt_data.set_length(xsdt_data.get_length()? - ACPI_XSDT_ENTRY_SIZE as u32);
            }
        }

        self.checksum_common_tables()?;
        Ok(())
    }

    /// Performs `exteneded_checksum` on the RSDP.
    pub(crate) fn checksum_common_tables(&self) -> Result<(), AcpiError> {
        // The RSDP doesn't have a standard header, so it is easier to calculate the checksum manually.
        if let Some(rsdp_table) = self.acpi_tables.write().get_mut(&Self::RSDP_KEY) {
            // SAFETY: We know the size and layout of the RSDP in memory.
            let rsdp_bytes =
                unsafe { slice::from_raw_parts(rsdp_table.table.as_ptr() as *mut u8, mem::size_of::<AcpiRsdp>()) };
            let sum_of_bytes: u8 = rsdp_bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
            // SAFETY: We only ever store an `AcpiRsdp` in the RSDP key.
            let rsdp = unsafe { rsdp_table.as_mut::<AcpiRsdp>() };
            // All bytes must sum to zero, so we write the negative of the sum.
            rsdp.extended_checksum = sum_of_bytes.wrapping_neg();
        }

        if let Some(ref mut xsdt_data) = *self.xsdt_metadata.write() {
            // Zero the old checksum byte.
            xsdt_data.slice[ACPI_CHECKSUM_OFFSET] = 0;
            // Sum all bytes (wrapping since the checksum is a u8 between 0-255).
            let sum_of_bytes: u8 = xsdt_data.slice.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
            // Write new checksum: equivalent to -1 * `sum_of_bytes` (so the sum is zero modulo 256).
            xsdt_data.slice[ACPI_CHECKSUM_OFFSET] = sum_of_bytes.wrapping_neg();
        }

        Ok(())
    }

    /// Publishes ACPI tables after installation.
    fn publish_tables(&self) -> Result<(), AcpiError> {
        if let Some(rsdp_table) = self.acpi_tables.write().get_mut(&Self::RSDP_KEY) {
            // Cast RSDP to raw pointer for boot services.
            let rsdp_ptr = rsdp_table.as_mut_ptr() as *mut c_void;

            unsafe {
                self.boot_services
                    .get()
                    .ok_or(AcpiError::ProviderNotInitialized)?
                    .install_configuration_table_unchecked(&signature::ACPI_TABLE_GUID, rsdp_ptr)
                    .map_err(|_| AcpiError::InstallConfigurationTableFailed)?;
            }
        }

        Ok(())
    }

    /// Calls the notify functions in `notify_list` upon installation of an ACPI table.
    fn notify_acpi_list(&self, table_key: TableKey) -> Result<(), AcpiError> {
        // Extract the guard as a variable so it lives until the end of this function.
        let read_guard = self.acpi_tables.read();
        let table_for_key = read_guard.get(&table_key);

        // Call each notify fn on the newly installed table.
        if let Some(notify_table) = table_for_key {
            let tbl_header = notify_table.header();
            for notify_fn in self.notify_list.read().iter() {
                (*notify_fn)(tbl_header, ACPI_VERSIONS_GTE_2, table_key.0);
            }
        } else {
            // If the table is not found in the list, we cannot notify.
            return Err(AcpiError::TableNotifyFailed);
        }

        Ok(())
    }

    /// Retrieves a table at a specific index in the list of installed tables.
    /// This is mostly to assist the C protocol.
    ///
    /// This function includes a hack/assumption based on the ordering of the BTreeMap, in order to avoid storing values in a indexed list:
    /// Since the BTreeMap is ordered by key value, and the key values are `usize`s under the hood,
    /// and we give out table keys in a monotonically increasing manner,
    /// tables are always sorted by order of installation.
    /// As such, BtreeMap.values[idx] is equivalent to indexing into a list of installed tables,
    /// assuming we correctly exclude system tables (XSDT, RSDP, FACS, and DSDT), which by spec are not included in the list of installed tables.
    ///
    /// The only downside to the above approach is the non-constant access time for a particular index.
    pub(crate) fn get_table_at_idx(&self, idx: usize) -> Result<(TableKey, AcpiTable), AcpiError> {
        let guard = self.acpi_tables.read();

        // Find the idx-th non-system table
        let found_table = guard
            .iter()
            .filter(|(k, _)| !Self::PRIVATE_SYSTEM_TABLES.contains(k))
            .nth(idx)
            .map(|(&key, table)| (key, *table));

        let table_at_idx = found_table.ok_or(AcpiError::InvalidTableIndex)?;
        Ok(table_at_idx)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use crate::acpi_table::AcpiXsdt;
    use crate::signature::MAX_INITIAL_ENTRIES;

    use super::*;
    use patina_sdk::boot_services::MockBootServices;
    use patina_sdk::component::service::memory::MockMemoryManager;
    use patina_sdk::component::service::memory::StdMemoryManager;
    use r_efi::efi;
    use std::boxed::Box;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

    #[repr(C, packed)]
    struct MockAcpiTable {
        _header: AcpiTableHeader,
        _data1: u8,
    }

    impl MockAcpiTable {
        fn new() -> Self {
            MockAcpiTable {
                _header: AcpiTableHeader {
                    signature: 0x1111,
                    length: (ACPI_HEADER_LEN + 1) as u32,
                    ..Default::default()
                },
                _data1: 23,
            }
        }
    }

    #[test]
    fn test_get_table() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        let mock_table = MockAcpiTable::new();
        let table_key = provider
            .install_standard_table(unsafe {
                AcpiTable::new(mock_table, provider.memory_manager.get().unwrap()).unwrap()
            })
            .unwrap();

        // Call get_acpi_table with a valid key.
        let fetched = provider.get_acpi_table(table_key).expect("table should have been installed");
        assert_eq!(fetched.signature(), 0x1111);
        assert_eq!(fetched.header().length, (ACPI_HEADER_LEN + 1) as u32);

        // Call with an invalid key (should return InvalidTableKey).
        let err = provider.get_acpi_table(TableKey(123123)).unwrap_err();
        assert!(matches!(err, AcpiError::InvalidTableKey));
    }

    #[test]
    fn test_register_notify() {
        fn dummy_notify(_table: &AcpiTableHeader, _value: u32, _key: usize) -> efi::Status {
            efi::Status::SUCCESS
        }

        let notify_fn: AcpiNotifyFn = dummy_notify;

        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(MockMemoryManager::new()))).unwrap();

        provider.register_notify(true, notify_fn).expect("should register notify");
        {
            let list = provider.notify_list.read();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0] as usize, notify_fn as usize);
        }

        // Unregister the notify function.
        provider.register_notify(false, notify_fn).expect("should unregister notify");
        {
            let list = provider.notify_list.read();
            assert!(list.is_empty());
        }

        // Attempt to unregister again — should fail.
        let result = provider.register_notify(false, notify_fn);
        assert!(matches!(result, Err(AcpiError::InvalidNotifyUnregister)));
    }

    #[test]
    fn test_iter() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        let header1 = AcpiTableHeader { signature: 0x1, length: 100, ..Default::default() };
        let table1 = unsafe { AcpiTable::new(header1, provider.memory_manager.get().unwrap()) };
        let header2 = AcpiTableHeader { signature: 0x2, length: 100, ..Default::default() };
        let table2 = unsafe { AcpiTable::new(header2, provider.memory_manager.get().unwrap()) };
        provider.install_standard_table(table1.unwrap()).expect("Install should succeed.");
        provider.install_standard_table(table2.unwrap()).expect("Install should succeed.");

        // Both tables should be in the list and in order
        let result = provider.iter_tables();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].signature(), 0x1);
        assert_eq!(result[0].header().length, 100);
        assert_eq!(result[1].signature(), 0x2);
        assert_eq!(result[1].header().length, 100);
    }

    #[test]
    fn test_get_xsdt_entry() {
        let entry0: u64 = 0x1111_2222_3333_4444;
        let entry1: u64 = 0xAAAA_BBBB_CCCC_DDDD;

        // Total length is header + 2 entries
        let xsdt_len = ACPI_HEADER_LEN + 2 * mem::size_of::<u64>();

        // Byte buffer, we treat this as the XSDT and write entries to it
        let mut buf = vec![0u8; xsdt_len];
        let off0 = ACPI_HEADER_LEN;
        buf[off0..off0 + 8].copy_from_slice(&entry0.to_le_bytes());
        let off1 = ACPI_HEADER_LEN + mem::size_of::<u64>();
        buf[off1..off1 + 8].copy_from_slice(&entry1.to_le_bytes());

        // We should be able to retrieve both XSDT entries
        let ptr = buf.as_ptr();
        let got0 = StandardAcpiProvider::<MockBootServices>::get_xsdt_entry_from_hob(0, ptr, xsdt_len)
            .expect("entry0 should be valid");
        let got1 = StandardAcpiProvider::<MockBootServices>::get_xsdt_entry_from_hob(1, ptr, xsdt_len)
            .expect("entry1 should be valid");
        assert_eq!(got0, entry0);
        assert_eq!(got1, entry1);

        // Index 2 is out of bounds (we have 2 total entries)
        let err = StandardAcpiProvider::<MockBootServices>::get_xsdt_entry_from_hob(2, ptr, xsdt_len).unwrap_err();
        assert!(matches!(err, AcpiError::InvalidXsdtEntry));
    }

    #[test]
    fn test_install_fadt() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        let allocator = provider.memory_manager.get().unwrap().get_allocator(EfiMemoryType::ACPIReclaimMemory).unwrap();

        // Initialize a mock XSDT.
        let mut xsdt_allocated_bytes = Vec::with_capacity_in(1000, allocator);
        let xsdt_info = AcpiXsdt {
            header: AcpiTableHeader {
                signature: signature::XSDT,
                length: ACPI_HEADER_LEN as u32,
                ..Default::default()
            },
        };
        xsdt_allocated_bytes.extend(xsdt_info.header.hdr_to_bytes());
        // Add some extra space after the XSDT so it's safe to write the entry.
        xsdt_allocated_bytes.extend(core::iter::repeat(0u8).take(100));
        let xsdt_metadata = AcpiXsdtMetadata {
            n_entries: 0,
            max_capacity: MAX_INITIAL_ENTRIES,
            slice: xsdt_allocated_bytes.into_boxed_slice(),
        };
        provider.set_xsdt(xsdt_metadata);

        // Create dummy data for the FADT.
        let fadt_header = AcpiTableHeader { signature: signature::FACP, length: 244, ..Default::default() };
        let fadt_info = AcpiFadt { header: fadt_header, ..Default::default() };
        let fadt_table = unsafe { AcpiTable::new(fadt_info, provider.memory_manager.get().unwrap()).unwrap() };
        let key = provider.install_fadt(fadt_table).unwrap();

        // The key should return the FADT.
        let retrieved_fadt = provider.get_acpi_table(key).unwrap();
        assert_eq!(retrieved_fadt.signature(), signature::FADT);
        assert_eq!(retrieved_fadt.header().length, 244);
        // The XSDT should have gained one entry (the FADT).
        assert_eq!(provider.xsdt_metadata.read().as_ref().unwrap().get_length().unwrap(), ACPI_HEADER_LEN as u32 + 8);

        // Any attempt to install the FADT again should fail.
        assert_eq!(provider.install_fadt(fadt_table).unwrap_err(), AcpiError::FadtAlreadyInstalled);
    }

    #[test]
    fn test_install_facs() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        // Create dummy data for FACS and FADT.
        let facs_info = AcpiFacs { signature: signature::FACS, length: 64, ..Default::default() };
        let fadt_header = AcpiTableHeader { signature: signature::FACP, length: 244, ..Default::default() };
        let fadt_info = AcpiFadt { header: fadt_header, ..Default::default() };

        // Install the FADT first.
        let fadt_key = provider
            .install_fadt(unsafe { AcpiTable::new(fadt_info, provider.memory_manager.get().unwrap()).unwrap() })
            .unwrap();
        // Install the FACS.
        let res = provider
            .install_facs(unsafe { AcpiTable::new(facs_info, provider.memory_manager.get().unwrap()).unwrap() });

        // Make sure FACS was installed in the provider.
        assert!(res.is_ok());
        assert_eq!(provider.get_acpi_table(res.unwrap()).unwrap().signature(), signature::FACS);

        // Make sure FACS was installed into FADT.
        assert!({
            let table = provider.get_acpi_table(fadt_key).unwrap();
            unsafe { table.as_ref::<AcpiFadt>() }.x_firmware_ctrl() != 0
        });
    }

    #[test]
    fn test_add_dsdt_to_list() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        // Create dummy data for DSDT and FADT.
        let dsdt_info = AcpiDsdt {
            header: AcpiTableHeader {
                signature: signature::DSDT,
                length: ACPI_HEADER_LEN as u32,
                ..Default::default()
            },
        };
        let fadt_header = AcpiTableHeader { signature: signature::FACP, length: 244, ..Default::default() };
        let fadt_info = AcpiFadt { header: fadt_header, ..Default::default() };
        // Install the FADT first.
        let fadt_key = provider
            .install_fadt(unsafe { AcpiTable::new(fadt_info, provider.memory_manager.get().unwrap()).unwrap() })
            .unwrap();
        // Install the DSDT.
        let res = provider
            .install_dsdt(unsafe { AcpiTable::new(dsdt_info, provider.memory_manager.get().unwrap()).unwrap() });

        // Make sure DSDT was installed in the provider.
        assert!(res.is_ok());
        assert_eq!(provider.get_acpi_table(res.unwrap()).unwrap().signature(), signature::DSDT);

        // Make sure DSDT was installed into FADT.
        assert!({
            let table = provider.get_acpi_table(fadt_key).unwrap();
            unsafe { table.as_ref::<AcpiFadt>() }.x_dsdt() != 0
        });
    }

    #[test]
    fn test_add_and_remove_xsdt() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        let allocator = provider.memory_manager.get().unwrap().get_allocator(EfiMemoryType::ACPIReclaimMemory).unwrap();

        // Initialize a mock XSDT.
        let mut xsdt_allocated_bytes = Vec::with_capacity_in(1000, allocator);
        let xsdt_info = AcpiXsdt {
            header: AcpiTableHeader {
                signature: signature::XSDT,
                length: ACPI_HEADER_LEN as u32,
                ..Default::default()
            },
        };
        xsdt_allocated_bytes.extend(xsdt_info.header.hdr_to_bytes());
        // Add some extra space after the XSDT so it's safe to write the entry.
        xsdt_allocated_bytes.extend(core::iter::repeat(0u8).take(100));
        let xsdt_metadata = AcpiXsdtMetadata {
            n_entries: 0,
            max_capacity: MAX_INITIAL_ENTRIES,
            slice: xsdt_allocated_bytes.into_boxed_slice(),
        };
        provider.set_xsdt(xsdt_metadata);

        const XSDT_ADDR: u64 = 0x1000_0000_0000_0004;

        let result = provider.add_entry_to_xsdt(XSDT_ADDR);
        assert!(result.is_ok());

        // We should now have 1 entry with address 0x1000_0000_0000_0004.
        assert_eq!(provider.xsdt_metadata.read().as_ref().unwrap().n_entries, 1);
        assert_eq!(
            u64::from_le_bytes(
                (provider.xsdt_metadata.read().as_ref().unwrap().slice.get(ACPI_HEADER_LEN..ACPI_HEADER_LEN + 8))
                    .unwrap()
                    .try_into()
                    .unwrap()
            ),
            XSDT_ADDR
        );
        // Length should be ACPI_HEADER_LEN + 1 entry
        assert_eq!(provider.xsdt_metadata.read().as_ref().unwrap().get_length().unwrap(), (ACPI_HEADER_LEN + 8) as u32);

        // Try removing the table.
        provider.remove_table_from_xsdt(XSDT_ADDR).expect("Removal of entry should succeed.");
        assert_eq!(provider.xsdt_metadata.read().as_ref().unwrap().n_entries, 0);
        // XSDT doesn't have to zero trailing entries, but should reduce length to mark the removed entry as invalid.
        assert_eq!(provider.xsdt_metadata.read().as_ref().unwrap().get_length().unwrap(), ACPI_HEADER_LEN as u32);
    }

    #[test]
    fn test_reallocate_xsdt() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        let allocator = provider.memory_manager.get().unwrap().get_allocator(EfiMemoryType::ACPIReclaimMemory).unwrap();

        // Initialize a mock XSDT with a small max_capacity
        let initial_capacity = 2;
        let mut xsdt_allocated_bytes =
            Vec::with_capacity_in(ACPI_HEADER_LEN + initial_capacity * ACPI_XSDT_ENTRY_SIZE, allocator);
        let xsdt_info = AcpiXsdt {
            header: AcpiTableHeader {
                signature: signature::XSDT,
                length: ACPI_HEADER_LEN as u32,
                ..Default::default()
            },
        };
        xsdt_allocated_bytes.extend(xsdt_info.header.hdr_to_bytes());
        xsdt_allocated_bytes.extend(core::iter::repeat(0u8).take(initial_capacity * ACPI_XSDT_ENTRY_SIZE));
        let xsdt_metadata = AcpiXsdtMetadata {
            n_entries: 0,
            max_capacity: initial_capacity,
            slice: xsdt_allocated_bytes.into_boxed_slice(),
        };
        provider.set_xsdt(xsdt_metadata);

        // Add entries up to capacity
        for i in 0..initial_capacity {
            let addr = 0x1000 + i as u64 * 0x10;
            provider.add_entry_to_xsdt(addr).expect("Should add entry");
        }
        assert_eq!(provider.xsdt_metadata.read().as_ref().unwrap().n_entries, initial_capacity);

        // Now add one more entry, which should trigger reallocation
        let new_addr = 0x2000;
        provider.add_entry_to_xsdt(new_addr).expect("Should add entry after reallocation");

        let xsdt_meta = provider.xsdt_metadata.read();
        assert!(xsdt_meta.as_ref().unwrap().max_capacity > initial_capacity);
        assert_eq!(xsdt_meta.as_ref().unwrap().n_entries, initial_capacity + 1);

        // Check that all previous entries are still present
        for i in 0..initial_capacity {
            let offset = ACPI_HEADER_LEN + i * ACPI_XSDT_ENTRY_SIZE;
            let entry_bytes = &xsdt_meta.as_ref().unwrap().slice[offset..offset + ACPI_XSDT_ENTRY_SIZE];
            let entry_addr = u64::from_le_bytes(entry_bytes.try_into().unwrap());
            assert_eq!(entry_addr, 0x1000 + i as u64 * 0x10);
        }
        // Check the new entry
        let offset = ACPI_HEADER_LEN + initial_capacity * ACPI_XSDT_ENTRY_SIZE;
        let entry_bytes = &xsdt_meta.as_ref().unwrap().slice[offset..offset + ACPI_XSDT_ENTRY_SIZE];
        let entry_addr = u64::from_le_bytes(entry_bytes.try_into().unwrap());
        assert_eq!(entry_addr, new_addr);
    }

    #[test]
    fn test_delete_table_facs() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        // Create a dummy XSDT.
        let allocator = provider.memory_manager.get().unwrap().get_allocator(EfiMemoryType::ACPIReclaimMemory).unwrap();
        let mut xsdt_allocated_bytes = Vec::with_capacity_in(100, allocator);
        let xsdt_info = crate::acpi_table::AcpiXsdt {
            header: AcpiTableHeader {
                signature: signature::XSDT,
                length: ACPI_HEADER_LEN as u32,
                ..Default::default()
            },
        };
        xsdt_allocated_bytes.extend(xsdt_info.header.hdr_to_bytes());
        xsdt_allocated_bytes.extend(core::iter::repeat(0u8).take(100));
        let xsdt_metadata = AcpiXsdtMetadata {
            n_entries: 0,
            max_capacity: crate::signature::MAX_INITIAL_ENTRIES,
            slice: xsdt_allocated_bytes.into_boxed_slice(),
        };
        provider.set_xsdt(xsdt_metadata);

        // Install FADT and FACS
        let fadt_header = AcpiTableHeader { signature: signature::FACP, length: 244, ..Default::default() };
        let fadt_info = AcpiFadt { header: fadt_header, ..Default::default() };
        let fadt_key = provider
            .install_fadt(unsafe { AcpiTable::new(fadt_info, provider.memory_manager.get().unwrap()).unwrap() })
            .unwrap();

        let facs_info = AcpiFacs { signature: signature::FACS, length: 64, ..Default::default() };
        let facs_table = unsafe { AcpiTable::new(facs_info, provider.memory_manager.get().unwrap()).unwrap() };
        let facs_key = provider.install_facs(facs_table).unwrap();

        // Delete FACS table
        let facs_addr = provider.get_acpi_table(facs_key).unwrap().as_ptr() as u64;
        let result = provider.delete_table(facs_addr, signature::FACS);
        assert!(result.is_ok());

        // FACS should be removed
        assert!(matches!(provider.get_acpi_table(facs_key).unwrap_err(), AcpiError::InvalidTableKey));

        // FADT's x_firmware_ctrl should be zero
        let fadt = provider.get_acpi_table(fadt_key).unwrap();
        assert_eq!(unsafe { fadt.as_ref::<AcpiFadt>() }.x_firmware_ctrl(), 0);
    }

    #[test]
    fn test_delete_table_dsdt() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        // Create a dummy XSDT.
        let allocator = provider.memory_manager.get().unwrap().get_allocator(EfiMemoryType::ACPIReclaimMemory).unwrap();
        let mut xsdt_allocated_bytes = Vec::with_capacity_in(100, allocator);
        let xsdt_info = crate::acpi_table::AcpiXsdt {
            header: AcpiTableHeader {
                signature: signature::XSDT,
                length: ACPI_HEADER_LEN as u32,
                ..Default::default()
            },
        };
        xsdt_allocated_bytes.extend(xsdt_info.header.hdr_to_bytes());
        xsdt_allocated_bytes.extend(core::iter::repeat(0u8).take(100));
        let xsdt_metadata = AcpiXsdtMetadata {
            n_entries: 0,
            max_capacity: crate::signature::MAX_INITIAL_ENTRIES,
            slice: xsdt_allocated_bytes.into_boxed_slice(),
        };
        provider.set_xsdt(xsdt_metadata);

        // Install FADT and DSDT
        let fadt_header = AcpiTableHeader { signature: signature::FACP, length: 244, ..Default::default() };
        let fadt_info = AcpiFadt { header: fadt_header, ..Default::default() };
        let fadt_key = provider
            .install_fadt(unsafe { AcpiTable::new(fadt_info, provider.memory_manager.get().unwrap()).unwrap() })
            .unwrap();

        let dsdt_info = AcpiDsdt {
            header: AcpiTableHeader {
                signature: signature::DSDT,
                length: ACPI_HEADER_LEN as u32,
                ..Default::default()
            },
        };
        let dsdt_table = unsafe { AcpiTable::new(dsdt_info, provider.memory_manager.get().unwrap()).unwrap() };
        let dsdt_key = provider.install_dsdt(dsdt_table).unwrap();

        // Delete DSDT table
        let dsdt_addr = provider.get_acpi_table(dsdt_key).unwrap().as_ptr() as u64;
        let result = provider.delete_table(dsdt_addr, signature::DSDT);
        assert!(result.is_ok());

        // DSDT should be removed
        assert!(matches!(provider.get_acpi_table(dsdt_key).unwrap_err(), AcpiError::InvalidTableKey));

        // FADT's x_dsdt should be zero
        let fadt = provider.get_acpi_table(fadt_key).unwrap();
        assert_eq!(unsafe { fadt.as_ref::<AcpiFadt>() }.x_dsdt(), 0);
    }

    fn mock_rsdp(rsdp_signature: u64, include_xsdt: bool, xsdt_length: usize, xsdt_signature: u32) -> u64 {
        let xsdt_ptr = if include_xsdt {
            // Build a buffer for the fake XSDT
            let mut xsdt_buf = vec![0u8; xsdt_length];

            // Write the length field of the XSDT
            let len_bytes = (xsdt_length as u32).to_le_bytes();
            xsdt_buf[4..8].copy_from_slice(&len_bytes);

            // Write the signature field of the XSDT
            let xsdt_sig = xsdt_signature.to_le_bytes();
            xsdt_buf[0..4].copy_from_slice(&xsdt_sig);

            // Leak the XSDT memory so that it persists during testing
            let static_xsdt: &'static [u8] = Box::leak(xsdt_buf.into_boxed_slice());
            static_xsdt.as_ptr() as u64
        } else {
            0
        };

        // Build a buffer for the fake RSDP
        let rsdp_size = size_of::<AcpiRsdp>();
        let mut rsdp_buf = vec![0u8; rsdp_size];

        // Copy the XSDT address to the RSDP
        let xsdt_addr_bytes = (xsdt_ptr as u64).to_le_bytes();
        rsdp_buf[24..32].copy_from_slice(&xsdt_addr_bytes);

        // Copy the desired signature to the signature field of the RSDP
        let sig_bytes = rsdp_signature.to_le_bytes();
        rsdp_buf[0..8].copy_from_slice(&sig_bytes);

        // Leak the RSDP memory so that it persists during testing
        let static_rsdp: &'static [u8] = Box::leak(rsdp_buf.into_boxed_slice());
        static_rsdp.as_ptr() as u64
    }

    #[test]
    fn test_get_xsdt_address() {
        // RSDP is null
        assert_eq!(
            StandardAcpiProvider::<MockBootServices>::get_xsdt_address_from_rsdp(0).unwrap_err(),
            AcpiError::NullRsdpFromHob
        );

        // The RSDP has signature 0 (invalid)
        assert_eq!(
            StandardAcpiProvider::<MockBootServices>::get_xsdt_address_from_rsdp(mock_rsdp(0, false, 0, 0))
                .unwrap_err(),
            AcpiError::InvalidSignature
        );

        // The RSDP has a valid signature, but the XSDT is null
        assert_eq!(
            StandardAcpiProvider::<MockBootServices>::get_xsdt_address_from_rsdp(mock_rsdp(
                signature::ACPI_RSDP_TABLE,
                false,
                0,
                0,
            ))
            .unwrap_err(),
            AcpiError::XsdtNotInitializedFromHob
        );

        // The RSDP is valid, but the XSDT has an invalid signature
        assert_eq!(
            StandardAcpiProvider::<MockBootServices>::get_xsdt_address_from_rsdp(mock_rsdp(
                signature::ACPI_RSDP_TABLE,
                true,
                ACPI_HEADER_LEN,
                0,
            ))
            .unwrap_err(),
            AcpiError::InvalidSignature
        );

        // The RSDP is valid, but the XSDT has an invalid length
        assert_eq!(
            StandardAcpiProvider::<MockBootServices>::get_xsdt_address_from_rsdp(mock_rsdp(
                signature::ACPI_RSDP_TABLE,
                true,
                ACPI_HEADER_LEN - 1,
                signature::XSDT,
            ))
            .unwrap_err(),
            AcpiError::XsdtInvalidLengthFromHob
        );

        // Both the RSDP and XSDT are valid
        assert!(
            StandardAcpiProvider::<MockBootServices>::get_xsdt_address_from_rsdp(mock_rsdp(
                signature::ACPI_RSDP_TABLE,
                true,
                ACPI_HEADER_LEN,
                signature::XSDT,
            ))
            .is_ok()
        );
    }

    #[test]
    fn test_install_tables_from_hob_normal_table() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        // Create a dummy normal ACPI table (not FADT, FACS, DSDT)
        let normal_header =
            AcpiTableHeader { signature: 0x010101, length: ACPI_HEADER_LEN as u32, ..Default::default() };
        let normal_table_box = Box::new(normal_header);
        let normal_table_addr = Box::into_raw(normal_table_box) as u64;

        // Create a dummy XSDT with one entry (the normal table)
        let xsdt_len = ACPI_HEADER_LEN + mem::size_of::<u64>();
        let mut xsdt_buf = vec![0u8; xsdt_len];
        // Write XSDT header
        let xsdt_header = AcpiTableHeader { signature: signature::XSDT, length: xsdt_len as u32, ..Default::default() };
        xsdt_buf[..ACPI_HEADER_LEN].copy_from_slice(&xsdt_header.hdr_to_bytes());
        // Write normal table address as the first entry
        xsdt_buf[ACPI_HEADER_LEN..ACPI_HEADER_LEN + 8].copy_from_slice(&normal_table_addr.to_le_bytes());
        let xsdt_ptr = Box::into_raw(xsdt_buf.into_boxed_slice()) as *const u8 as u64;

        // Create a dummy RSDP pointing to the XSDT
        let rsdp = AcpiRsdp { signature: signature::ACPI_RSDP_TABLE, xsdt_address: xsdt_ptr, ..Default::default() };
        let rsdp_ptr = Box::into_raw(Box::new(rsdp)) as u64;

        // Create the HOB
        let acpi_hob = Hob::mock(vec![AcpiMemoryHob { rsdp_address: rsdp_ptr }]);

        // Call install_tables_from_hob
        let result = provider.install_tables_from_hob(acpi_hob);
        assert!(result.is_ok());

        // The normal table should be installed
        let installed_tables = provider.iter_tables();
        assert_eq!(installed_tables.len(), 1);
        assert_eq!(installed_tables[0].signature(), 0x010101);
        assert_eq!(installed_tables[0].header().length, ACPI_HEADER_LEN as u32);
    }

    #[test]
    fn test_initialize_error_cases() {
        let provider = StandardAcpiProvider::new_uninit();
        let mock_boot_services = MockBootServices::new();
        let mock_memory_manager: Service<dyn MemoryManager> = Service::mock(Box::new(StdMemoryManager::new()));

        // First initialization should succeed
        assert!(provider.initialize(mock_boot_services, mock_memory_manager.clone()).is_ok());

        // Second initialization with boot services should fail
        let err = provider.initialize(MockBootServices::new(), mock_memory_manager.clone()).unwrap_err();
        assert_eq!(err, AcpiError::BootServicesAlreadyInitialized);

        // Try initializing again with a new provider, but memory manager already set
        let provider2 = StandardAcpiProvider::new_uninit();
        // Set boot services first
        assert!(provider2.boot_services.set(MockBootServices::new()).is_ok());
        // Set memory manager first
        assert!(provider2.memory_manager.set(mock_memory_manager.clone()).is_ok());
        // Now initialize should fail for both fields
        let err = provider2.initialize(MockBootServices::new(), mock_memory_manager.clone()).unwrap_err();
        assert!(matches!(err, AcpiError::BootServicesAlreadyInitialized | AcpiError::MemoryManagerAlreadyInitialized));
    }

    #[test]
    fn test_install_fadt_tables_from_hob() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        // Create dummy FACS and DSDT tables
        let facs = AcpiFacs { signature: signature::FACS, length: 64, ..Default::default() };
        let dsdt = AcpiDsdt {
            header: AcpiTableHeader {
                signature: signature::DSDT,
                length: ACPI_HEADER_LEN as u32,
                ..Default::default()
            },
        };

        // Allocate FACS and DSDT in memory and get their addresses
        let facs_box = Box::new(facs);
        let facs_addr = Box::into_raw(facs_box) as u64;
        let dsdt_box = Box::new(dsdt);
        let dsdt_addr = Box::into_raw(dsdt_box) as u64;

        // Create FADT pointing to FACS and DSDT
        let mut fadt = AcpiFadt {
            header: AcpiTableHeader { signature: signature::FADT, length: 244, ..Default::default() },
            ..Default::default()
        };
        fadt.inner.x_firmware_ctrl = facs_addr;
        fadt.inner.x_dsdt = dsdt_addr;

        // Call install_fadt_tables_from_hob
        let result = provider.install_fadt_tables_from_hob(&fadt);
        assert!(result.is_ok());

        // FACS and DSDT should be installed
        let facs_table = provider.get_acpi_table(StandardAcpiProvider::<MockBootServices>::FACS_KEY);
        assert!(facs_table.is_ok());
        assert_eq!(facs_table.unwrap().signature(), signature::FACS);

        let dsdt_table = provider.get_acpi_table(StandardAcpiProvider::<MockBootServices>::DSDT_KEY);
        assert!(dsdt_table.is_ok());
        assert_eq!(dsdt_table.unwrap().signature(), signature::DSDT);
    }
    #[test]
    fn test_remove_table_from_xsdt_removes_correct_entry() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        let allocator = provider.memory_manager.get().unwrap().get_allocator(EfiMemoryType::ACPIReclaimMemory).unwrap();

        // Initialize a mock XSDT with capacity for 3 entries
        let initial_capacity = 3;
        let mut xsdt_allocated_bytes =
            Vec::with_capacity_in(ACPI_HEADER_LEN + initial_capacity * ACPI_XSDT_ENTRY_SIZE, allocator);
        let xsdt_info = AcpiXsdt {
            header: AcpiTableHeader {
                signature: signature::XSDT,
                length: ACPI_HEADER_LEN as u32,
                ..Default::default()
            },
        };
        xsdt_allocated_bytes.extend(xsdt_info.header.hdr_to_bytes());
        xsdt_allocated_bytes.extend(core::iter::repeat(0u8).take(initial_capacity * ACPI_XSDT_ENTRY_SIZE));
        let xsdt_metadata = AcpiXsdtMetadata {
            n_entries: 0,
            max_capacity: initial_capacity,
            slice: xsdt_allocated_bytes.into_boxed_slice(),
        };
        provider.set_xsdt(xsdt_metadata);

        // Add three entries
        let addr1 = 0x1000;
        let addr2 = 0x2000;
        let addr3 = 0x3000;
        provider.add_entry_to_xsdt(addr1).expect("Should add entry 1");
        provider.add_entry_to_xsdt(addr2).expect("Should add entry 2");
        provider.add_entry_to_xsdt(addr3).expect("Should add entry 3");

        // Remove the middle entry (addr2)
        provider.remove_table_from_xsdt(addr2).expect("Should remove entry 2");

        let xsdt_meta = provider.xsdt_metadata.read();
        let xsdt_data = xsdt_meta.as_ref().unwrap();

        // n_entries should be 2
        assert_eq!(xsdt_data.n_entries, 2);

        // The remaining entries should be addr1 and addr3, in order
        let entry_bytes_0 = &xsdt_data.slice[ACPI_HEADER_LEN..ACPI_HEADER_LEN + ACPI_XSDT_ENTRY_SIZE];
        let entry_addr_0 = u64::from_le_bytes(entry_bytes_0.try_into().unwrap());
        assert_eq!(entry_addr_0, addr1);

        let entry_bytes_1 =
            &xsdt_data.slice[ACPI_HEADER_LEN + ACPI_XSDT_ENTRY_SIZE..ACPI_HEADER_LEN + 2 * ACPI_XSDT_ENTRY_SIZE];
        let entry_addr_1 = u64::from_le_bytes(entry_bytes_1.try_into().unwrap());
        assert_eq!(entry_addr_1, addr3);

        // The removed slot should be zeroed
        let removed_bytes =
            &xsdt_data.slice[ACPI_HEADER_LEN + 2 * ACPI_XSDT_ENTRY_SIZE..ACPI_HEADER_LEN + 3 * ACPI_XSDT_ENTRY_SIZE];
        assert!(removed_bytes.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_notify_acpi_list_called_on_install() {
        static NOTIFY_CALLED: AtomicBool = AtomicBool::new(false);

        fn notify_fn(_table: &AcpiTableHeader, _version: u32, _key: usize) -> efi::Status {
            NOTIFY_CALLED.store(true, AtomicOrdering::SeqCst);
            efi::Status::SUCCESS
        }

        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        // Register notify function
        provider.register_notify(true, notify_fn).expect("should register notify");

        // Install a standard table and check if notify was called.
        let header = AcpiTableHeader { signature: 0x0101, length: 100, ..Default::default() };
        let table = unsafe { AcpiTable::new(header, provider.memory_manager.get().unwrap()).unwrap() };
        let _ = provider.install_acpi_table(table).unwrap();

        // notify_acpi_list should have been called by install_standard_table
        assert!(NOTIFY_CALLED.load(AtomicOrdering::SeqCst));
    }

    #[test]
    fn test_notify_acpi_list_error_on_invalid_key() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        // Try to notify with an invalid key
        let result = provider.notify_acpi_list(TableKey(99999));
        assert!(matches!(result, Err(AcpiError::TableNotifyFailed)));
    }

    #[test]
    fn test_get_table_at_idx_basic() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        // Install two standard tables
        let header1 = AcpiTableHeader { signature: 0x10, length: 101, ..Default::default() };
        let table1 = unsafe { AcpiTable::new(header1, provider.memory_manager.get().unwrap()).unwrap() };
        let key1 = provider.install_standard_table(table1).unwrap();

        let header2 = AcpiTableHeader { signature: 0x11, length: 102, ..Default::default() };
        let table2 = unsafe { AcpiTable::new(header2, provider.memory_manager.get().unwrap()).unwrap() };
        let key2 = provider.install_standard_table(table2).unwrap();

        // Index 0 should return the first table
        let (got_key1, got_table1) = provider.get_table_at_idx(0).unwrap();
        assert_eq!(got_key1, key1);
        assert_eq!(got_table1.signature(), 0x10);
        assert_eq!(got_table1.header().length, 101);

        // Index 1 should return the second table
        let (got_key2, got_table2) = provider.get_table_at_idx(1).unwrap();
        assert_eq!(got_key2, key2);
        assert_eq!(got_table2.signature(), 0x11);
        assert_eq!(got_table2.header().length, 102);

        // Index out of bounds should return error
        let err = provider.get_table_at_idx(3).unwrap_err();
        assert!(matches!(err, AcpiError::InvalidTableIndex));
    }

    #[test]
    fn test_get_table_at_idx_excludes_system_tables() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        let facs_info = AcpiFacs { signature: signature::FACS, length: 64, ..Default::default() };
        provider
            .install_facs(unsafe { AcpiTable::new(facs_info, provider.memory_manager.get().unwrap()).unwrap() })
            .unwrap();

        let dsdt_info =
            AcpiDsdt { header: AcpiTableHeader { signature: signature::DSDT, length: 128, ..Default::default() } };
        provider
            .install_dsdt(unsafe { AcpiTable::new(dsdt_info, provider.memory_manager.get().unwrap()).unwrap() })
            .unwrap();

        // Install a standard table
        let header = AcpiTableHeader { signature: 0x11, length: 111, ..Default::default() };
        let table = unsafe { AcpiTable::new(header, provider.memory_manager.get().unwrap()).unwrap() };
        let key = provider.install_standard_table(table).unwrap();

        // Only the standard table should be returned at index 0
        let (got_key, got_table) = provider.get_table_at_idx(0).unwrap();
        assert_eq!(got_table.signature(), 0x11);
        assert_eq!(got_key, key);
    }
}
