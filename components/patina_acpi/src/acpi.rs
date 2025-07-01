use alloc::{
    boxed::Box,
    collections::btree_map::{BTreeMap, Values},
};
use core::{
    any::{Any, TypeId},
    cell::OnceCell,
    ffi::c_void,
    mem::{self, offset_of},
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

use crate::{
    acpi_table::{AcpiTable, AcpiTableHeader, RawAcpiTable, StandardAcpiTable},
    alloc::vec::Vec,
    signature::{ACPI_CHECKSUM_OFFSET, ACPI_VERSIONS_GTE_2, ACPI_XSDT_ENTRY_SIZE, FACS, MAX_INITIAL_ENTRIES},
};
use crate::{alloc::vec, service::AcpiNotifyFn};

use patina_sdk::boot_services::{BootServices, StandardBootServices};
use patina_sdk::{
    component::{
        hob::Hob,
        service::{memory::MemoryManager, IntoService, Service},
    },
    efi_types::EfiMemoryType,
};

use spin::{rwlock::RwLock, RwLockReadGuard};

use crate::{
    acpi_table::{AcpiDsdt, AcpiFacs, AcpiFadt, AcpiRsdp, AcpiXsdt},
    component::AcpiMemoryHob,
    error::AcpiError,
    service::{AcpiProvider, TableKey},
    signature::{self, ACPI_HEADER_LEN},
};

pub static ACPI_TABLE_INFO: StandardAcpiProvider<StandardBootServices> = StandardAcpiProvider::new_uninit();

/// Standard implementation of ACPI services. The service interface can be found in `service.rs`
#[derive(IntoService)]
#[service(dyn AcpiProvider)]
pub(crate) struct StandardAcpiProvider<B: BootServices + 'static> {
    /// Platform-installed ACPI tables.
    /// If installing a non-standard ACPI table, the platform is responsible for writing its own handler and parser.
    acpi_tables: RwLock<BTreeMap<TableKey, AcpiTable>>,
    /// Stores a monotonically increasing unique table key for installation.
    next_table_key: AtomicUsize,
    /// Stores notify callbacks, which are called upon table installation.
    notify_list: RwLock<Vec<AcpiNotifyFn>>,
    /// Provides boot services.
    pub(crate) boot_services: OnceCell<B>,
    /// Provides memory services.
    pub(crate) memory_manager: OnceCell<Service<dyn MemoryManager>>,
    /// Current number of installed tables.
    pub(crate) n_tables: AtomicUsize,
    /// Currently allocated capacity for storing table entries.
    pub(crate) max_tables: AtomicUsize,
}

type FadtLock = RwLock<Option<Box<AcpiFadt, &'static dyn alloc::alloc::Allocator>>>;

/// Holds pointers to known system tables used by the ACPI provider.
/// A table may either be null, indicating it is not yet installed or not present on the platform, or point to a valid table in ACPI memory.
pub(crate) struct SystemTables {
    fadt: FadtLock,
    facs: RwLock<Option<Box<AcpiFacs, &'static dyn alloc::alloc::Allocator>>>, // Box<Facs, Global> -> get_allocator() -> &dyn Allocator
    dsdt: RwLock<Option<Box<AcpiDsdt, &'static dyn alloc::alloc::Allocator>>>,
    rsdp: RwLock<Option<Box<AcpiRsdp, &'static dyn alloc::alloc::Allocator>>>,
}

impl SystemTables {
    pub const fn new() -> Self {
        // A the time of construction these are not valid ACPI tables.
        // They are filled in during installation.

        // SHERRY: these have to be const for static init
        Self { fadt: RwLock::new(None), facs: RwLock::new(None), dsdt: RwLock::new(None), rsdp: RwLock::new(None) }
    }
}

impl SystemTables {
    pub(crate) fn facs_address(&self) -> Option<u64> {
        self.facs.read().as_ref().map(|boxed_facs| {
            let ptr: *const AcpiFacs = &**boxed_facs;
            ptr as u64
        })
    }

    pub(crate) fn dsdt_address(&self) -> Option<u64> {
        self.dsdt.read().as_ref().map(|boxed_dsdt| {
            let ptr: *const AcpiDsdt = &**boxed_dsdt;
            ptr as u64
        })
    }
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
    const XSDT_KEY: TableKey = TableKey(3);
    const RSDP_KEY: TableKey = TableKey(4);
    const FADT_KEY: TableKey = TableKey(5);

    /// The first unused key which can be given to callers of `install_acpi_table`.
    const FIRST_FREE_KEY: usize = 6;

    /// Keys which are not available to be indexed or iterated by an end user.
    const PRIVATE_SYSTEM_TABLES: [TableKey; 4] = [Self::RSDP_KEY, Self::XSDT_KEY, Self::FACS_KEY, Self::DSDT_KEY];

    /// Creates a new `StandardAcpiProvider` with uninitialized fields.
    /// Attempting to use `StandardAcpiProvider` before initialization will cause a panic.
    pub const fn new_uninit() -> Self {
        Self {
            acpi_tables: RwLock::new(BTreeMap::new()),
            next_table_key: AtomicUsize::new(Self::FIRST_FREE_KEY),
            notify_list: RwLock::new(vec![]),
            boot_services: OnceCell::new(),
            memory_manager: OnceCell::new(),
            n_tables: AtomicUsize::new(0),
            max_tables: AtomicUsize::new(MAX_INITIAL_ENTRIES),
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
    pub fn set_xsdt(&self, xsdt_table: AcpiTable) {
        self.acpi_tables.write().insert(Self::RSDP_KEY, xsdt_table);
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
            AcpiTable::FACS => self.install_facs(table)?,
            AcpiTable::FADT => self.install_fadt(table)?,
            AcpiTable::DSDT => self.install_dsdt(table)?,
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
        self.acpi_tables
            .read()
            .get(&table_key)
            .cloned() // Option<&AcpiTable> → Option<AcpiTable>
            .ok_or(AcpiError::InvalidTableKey)
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
    /// The RSDP, XSDT, FACS, and DSDT are not considered part of the list of installed tables and should not be iterated over.
    fn iter_tables(&self) -> Vec<AcpiTable> {
        // The following system tables do not count in the list of installed tables and should not be iterated over.
        let installed_tables_list: Vec<AcpiTable> = self
            .acpi_tables
            .read()
            .iter()
            .filter(|(key, _value)| {
                // Keep only those entries whose key is NOT in invalid_table_keys
                !Self::PRIVATE_SYSTEM_TABLES.contains(key)
            })
            .map(|(_key, value)| value)
            .cloned()
            .collect();
        installed_tables_list
    }
}

impl<B> StandardAcpiProvider<B>
where
    B: BootServices,
{
    fn install_facs(&self, facs_info: AcpiTable) -> Result<TableKey, AcpiError> {
        // Update the FADT's address pointer to the FACS.
        if let Some(fadt_table) = self.acpi_tables.write().get_mut(&Self::FADT_KEY) {
            let facs_addr = facs_info.as_ref::<AcpiFacs>() as *const AcpiFacs as u64;
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

        // Read the header to validate the XSDT signature is valid
        // SAFETY: `xsdt_address` has been validated to be non-null
        let xsdt_header = rsdp.xsdt_address as *const AcpiTableHeader;
        if (unsafe { *xsdt_header }).signature != signature::XSDT {
            return Err(AcpiError::InvalidSignature);
        }

        // SAFETY: We validate that the XSDT is non-null and contains the right signature.
        let xsdt_ptr = rsdp.xsdt_address as *mut AcpiXsdt;
        let xsdt = unsafe { &*(xsdt_ptr) };

        if xsdt.header.length < ACPI_HEADER_LEN as u32 {
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
                AcpiTable::new(facs_from_ptr, self.memory_manager.get().ok_or(AcpiError::ProviderNotInitialized)?)
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
                AcpiTable::new(dsdt_from_ptr, self.memory_manager.get().ok_or(AcpiError::ProviderNotInitialized)?)
            };
            self.install_dsdt(dsdt_table)?;
        }

        Ok(())
    }

    /// Installs tables pointed to by the ACPI memory HOB.
    pub fn install_tables_from_hob(&self, acpi_hob: Hob<AcpiMemoryHob>) -> Result<(), AcpiError> {
        let xsdt_address = Self::get_xsdt_address_from_rsdp(acpi_hob.rsdp_address)?;
        let xsdt_ptr = xsdt_address as *const AcpiXsdt;

        // SAFETY: `get_xsdt_address_from_rsdp` should perform necessary validations on XSDT
        let xsdt_length = (unsafe { *xsdt_ptr }).header.length;

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
                AcpiTable::new(tbl_header, self.memory_manager.get().ok_or(AcpiError::ProviderNotInitialized)?)
            };

            self.install_standard_table(table)?;

            // If this table points to other system tables, install them too
            if tbl_header.signature == signature::FADT {
                // SAFETY: assuming the XSDT entry is written correctly, this points to a valid ACPI table
                // and the signature has been verified to match that of the FADT
                let fadt = unsafe { &*(entry_addr as *const AcpiFadt) };
                self.install_fadt_tables_from_hob(fadt)?;
            }

            table.update_checksum();
        }
        self.publish_tables()?;
        Ok(())
    }

    // Determines the type of memory to allocate for the table based on table properties.
    fn allocation_memory_type(&self, signature: u32) -> EfiMemoryType {
        if signature == signature::FACS || signature == signature::UEFI {
            // FACS and UEFI table must be allocated in NVS (by spec).
            EfiMemoryType::ACPIMemoryNVS
        } else {
            EfiMemoryType::ACPIReclaimMemory
        }
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
        fadt_info.update_checksum();

        // Add the FADT to the list of installed tables.
        self.acpi_tables.write().insert(Self::FADT_KEY, fadt_info);

        // RSDP derives OEM ID from FADT.
        if let Some(rsdp) = self.acpi_tables.write().get_mut(&Self::RSDP_KEY) {
            unsafe { rsdp.as_mut::<AcpiRsdp>() }.oem_id = fadt_info.header().oem_id;
        }

        // XSDT derives OEM information from FADT.
        if let Some(xsdt) = self.acpi_tables.write().get_mut(&Self::XSDT_KEY) {
            let xsdt_tbl = unsafe { xsdt.as_mut::<AcpiXsdt>() };
            xsdt_tbl.header.oem_id = fadt_info.header().oem_id;
            xsdt_tbl.header.oem_table_id = fadt_info.header().oem_table_id;
            xsdt_tbl.header.oem_revision = fadt_info.header().oem_revision;
        }

        // Checksum root tables after modifying fields.
        fadt_info.update_checksum();

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

        dsdt_info.update_checksum();

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
        table_info.update_checksum();

        // Get the physical address of the table for the XSDT entry.
        let physical_addr = table_info.as_ptr() as u64;
        self.add_entry_to_xsdt(physical_addr);

        // Since XSDT was modified, recalculate checksum for root tables.
        self.checksum_common_tables()?;
        Ok(curr_key)
    }

    /// Adds an address entry to the XSDT.
    fn add_entry_to_xsdt(&self, new_table_addr: u64) -> Result<(), AcpiError> {
        if let Some(xsdt_table) = self.acpi_tables.read().get(&Self::XSDT_KEY) {
            let max_capacity = self.max_tables.load(Ordering::Acquire);
            let curr_capacity = self.n_tables.load(Ordering::Acquire);
            // XSDT is full. Reallocate buffer.
            if curr_capacity == max_capacity {
                self.reallocate_xsdt()?;
            }

            // Next entry goes after header + existing address entries.
            let entry_offset = ACPI_HEADER_LEN + xsdt_data.n_entries * ACPI_XSDT_ENTRY_SIZE;
            // Fill in the bytes of the new address entry.
            xsdt_data.slice[entry_offset..entry_offset + ACPI_XSDT_ENTRY_SIZE]
                .copy_from_slice(&new_table_addr.to_le_bytes());

            // Increase XSDT entries by 1.
            xsdt_data.n_entries += 1;
            let curr_len = xsdt_data.get_length()?;
            // Write the new length into the header.
            xsdt_data.set_length(curr_len + ACPI_XSDT_ENTRY_SIZE as u32);

            // Checksum the XSDT after modifying it.
            Self::acpi_table_update_checksum(&mut xsdt_data.slice, ACPI_CHECKSUM_OFFSET);

            Ok(())
        } else {
            Err(AcpiError::ProviderNotInitialized)
        }
    }

    /// Allocates a new, larger memory space for the XSDT when it is full and relocates all entries to the newly allocated memory.
    fn reallocate_xsdt(&self) -> Result<(), AcpiError> {
        if let Some(ref mut xsdt_data) = *self.xsdt_metadata.write() {
            // Use a geometric resizing strategy.
            let num_bytes_original = ACPI_HEADER_LEN + xsdt_data.max_capacity * ACPI_XSDT_ENTRY_SIZE;
            let num_bytes_new = ACPI_HEADER_LEN + xsdt_data.max_capacity * ACPI_XSDT_ENTRY_SIZE * 2;
            xsdt_data.max_capacity *= 2;

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
                self.acpi_tables.write().remove(&Self::FACS_KEY);

                // Clear out the FACS pointer in the FADT.
                if let Some(fadt_table) = self.acpi_tables.write().get_mut(&Self::FADT_KEY) {
                    unsafe { fadt_table.as_mut::<AcpiFadt>() }.set_x_firmware_ctrl(0);
                    fadt_table.update_checksum();
                }
            }
            signature::DSDT => {
                self.acpi_tables.write().remove(&Self::DSDT_KEY);

                // Clear out the FACS pointer in the FADT
                if let Some(fadt_table) = self.acpi_tables.write().get_mut(&Self::FADT_KEY) {
                    unsafe { fadt_table.as_mut::<AcpiFadt>() }.set_x_dsdt(0);
                    fadt_table.update_checksum();
                }
            }
            _ => {
                self.remove_table_from_xsdt(physical_addr as u64)?;
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

                // Decrease XSDT length.
                xsdt_data.set_length(xsdt_data.get_length()? - ACPI_XSDT_ENTRY_SIZE as u32);
            }
        }

        self.checksum_common_tables()?;
        Ok(())
    }

    /// Recalculates the checksum for an ACPI table.
    /// According to ACPI spec, all bytes of an ACPI table must sum to zero.
    fn acpi_table_update_checksum(table: &mut [u8], acpi_checksum_offset: usize) {
        // Zero the old checksum byte.
        table[acpi_checksum_offset] = 0;

        // Sum all bytes (wrapping since the checksum is a u8 between 0-255).
        let sum_of_bytes: u8 = table.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));

        // Write new checksum: equivalent to -1 * `sum_of_bytes` (so the sum is zero modulo 256).
        table[acpi_checksum_offset] = sum_of_bytes.wrapping_neg();
    }

    // Performs checksums on shared ACPI tables (the XSDT and RSDP track other tables).
    pub(crate) fn checksum_common_tables(&self) -> Result<(), AcpiError> {
        if let Some(rsdp_table) = self.acpi_tables.write().get_mut(&Self::RSDP_KEY) {
            rsdp_table.update_checksum();
        }

        if let Some(xsdt_table) = self.acpi_tables.write().get_mut(&Self::XSDT_KEY) {
            xsdt_table.update_checksum();
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

        if let Some(notify_table) = table_for_key {
            let tbl_header = notify_table.header();
            for notify_fn in self.notify_list.read().iter() {
                (*notify_fn)(tbl_header, ACPI_VERSIONS_GTE_2, table_key);
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
        let read_guard = self.acpi_tables.read();

        // Build a vector of (key, table) EXCLUDING invalid system tables.
        let installed: Vec<(TableKey, AcpiTable)> = read_guard
            .iter()
            .filter(|(k, _)| !Self::PRIVATE_SYSTEM_TABLES.contains(k))
            .map(|(&k, v)| (k, v.clone()))
            .collect();

        if let Some(pair) = installed.get(idx) {
            Ok(pair.clone())
        } else {
            // Out-of-bounds index provided.
            Err(AcpiError::InvalidTableIndex)
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use crate::acpi_table::FadtData;
    use crate::signature::MAX_INITIAL_ENTRIES;

    use super::*;
    use core::any::TypeId;
    use core::ptr::NonNull;
    use patina_sdk::boot_services::MockBootServices;
    use patina_sdk::component::service::memory::MockMemoryManager;
    use patina_sdk::component::service::memory::StdMemoryManager;
    use std::boxed::Box;

    struct MockAcpiTable {}

    impl StandardAcpiTable for MockAcpiTable {
        fn header(&self) -> &AcpiTableHeader {
            Box::leak(Box::new(AcpiTableHeader { signature: 0x1111, length: 123, ..Default::default() }))
        }
    }

    #[test]
    fn test_get_table() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        let mock_table = MockAcpiTable {};
        let table_key = provider.install_standard_table(&mock_table, TypeId::of::<MockAcpiTable>()).unwrap();

        // Call get_acpi_table with a valid key
        let fetched = provider.get_acpi_table(table_key).expect("table should have been installed");
        assert_eq!(fetched.signature(), 0x1111);
        assert_eq!(fetched.length(), 123);

        // Call with an invalid key (should return InvalidTableKey)
        let err = provider.get_acpi_table(19283712837218).unwrap_err();
        assert!(matches!(err, AcpiError::InvalidTableKey));
    }

    #[test]
    fn test_register_notify() {
        fn dummy_notify(_table: &AcpiTableHeader, _value: u32, _key: TableKey) -> Result<(), AcpiError> {
            Ok(())
        }

        let notify_fn: AcpiNotifyFn = dummy_notify;

        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(true, MockBootServices::new(), Service::mock(Box::new(MockMemoryManager::new()))).unwrap();

        provider.register_notify(true, notify_fn).expect("should register notify");
        {
            let list = provider.notify_list.read();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0] as usize, notify_fn as usize);
        }

        // Unregister the notify function
        provider.register_notify(false, notify_fn).expect("should unregister notify");
        {
            let list = provider.notify_list.read();
            assert!(list.is_empty());
        }

        // Attempt to unregister again — should fail
        let result = provider.register_notify(false, notify_fn);
        assert!(matches!(result, Err(AcpiError::InvalidNotifyUnregister)));
    }

    #[test]
    fn test_iter() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(true, MockBootServices::new(), Service::mock(Box::new(MockMemoryManager::new()))).unwrap();

        let mut header1 = AcpiTableHeader { signature: 0x1, length: 10, ..Default::default() };
        let header1_ptr = NonNull::new(&mut header1 as *mut AcpiTableHeader).unwrap();
        let table1 = MemoryAcpiTable {
            header: header1_ptr,
            type_id: TypeId::of::<u32>(),
            table_key: 123,
            physical_address: Some(123),
            table_buf: None,
        };
        let mut header2 = AcpiTableHeader { signature: 0x2, length: 20, ..Default::default() };
        let header2_ptr = NonNull::new(&mut header2 as *mut AcpiTableHeader).unwrap();
        let table2 = MemoryAcpiTable {
            header: header2_ptr,
            type_id: TypeId::of::<u32>(),
            table_key: 123,
            physical_address: Some(123),
            table_buf: None,
        };
        {
            let mut vec = provider.acpi_tables.write();
            vec.push(table1);
            vec.push(table2);
        }

        // Both tables should be in the list and in order
        let result = provider.iter();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].signature, 0x1);
        assert_eq!(result[0].length, 10);
        assert_eq!(result[1].signature, 0x2);
        assert_eq!(result[1].length, 20);
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
    fn test_add_facs_to_list() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(true, MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        // Create dummy data for FACS and FADT.
        let facs_info = AcpiFacs { signature: signature::FACS, length: 64, ..Default::default() };
        let fadt_header = AcpiTableHeader { signature: signature::FACP, length: 244, ..Default::default() };
        let fadt_info = AcpiFadt { header: fadt_header, ..Default::default() };
        // Store the dummy FADT so it seems like it's been "installed" since the FACS is only accessible through the FADT.
        let allocator = provider.memory_manager.get().unwrap().get_allocator(EfiMemoryType::BootServicesData).unwrap();
        let fadt_allocated = Box::new_in(fadt_info, allocator);
        {
            let mut write_guard = provider.system_tables.fadt.write();
            *write_guard = Some(fadt_allocated);
        }

        // Install FACS.
        let res = provider.install_facs(&facs_info);

        // Make sure FACS pointer was set in `system_tables`.
        assert!(res.is_ok());
        assert!(provider.system_tables.facs.read().is_some());
        assert!(provider.system_tables.facs.read().as_ref().unwrap().signature == signature::FACS);

        // Make sure FACS was installed into FADT.
        assert!(provider.system_tables.fadt.read().as_ref().unwrap().x_firmware_ctrl() != 0);
    }

    #[test]
    fn test_add_dsdt_to_list() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(true, MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

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
        // Store the dummy FADT so it seems like it's been "installed" since the DSDT is only accessible through the FADT.
        let allocator = provider.memory_manager.get().unwrap().get_allocator(EfiMemoryType::BootServicesData).unwrap();
        let fadt_allocated = Box::new_in(fadt_info, allocator);
        {
            let mut write_guard = provider.system_tables.fadt.write();
            *write_guard = Some(fadt_allocated);
        }

        // Install DSDT.
        let res = provider.add_dsdt_to_list(&dsdt_info);

        // Make sure DSDT pointer was set in `system_tables`.
        assert!(res.is_ok());
        assert!(provider.system_tables.dsdt.read().is_some());
        assert!(provider.system_tables.dsdt.read().as_ref().unwrap().header.signature == signature::DSDT);

        // Make sure DSDT was installed into FADT.
        assert!(provider.system_tables.fadt.read().as_ref().unwrap().x_dsdt() != 0);
    }

    #[test]
    fn test_add_fadt_to_list() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(true, MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        let fadt_header =
            AcpiTableHeader { signature: signature::FACP, length: ACPI_HEADER_LEN as u32, ..Default::default() };
        let fadt_info = AcpiFadt { header: fadt_header, ..Default::default() };

        let result = provider.install_fadt_in_memory(&fadt_info);
        assert!(result.is_ok());

        // FADT should have been added to list
        assert!(provider.system_tables.fadt.read().is_some());
    }

    #[test]
    fn test_add_and_remove_xsdt() {
        let xsdt_table = AcpiXsdt {
            header: AcpiTableHeader {
                signature: signature::XSDT,
                length: ACPI_HEADER_LEN as u32, // XSDT currently has no entries
                revision: 1,
                checksum: 0,
                oem_id: *b"123456",
                oem_table_id: *b"12345678",
                oem_revision: 1,
                creator_id: 0,
                creator_revision: 0,
            },
        };

        // Initialize XSDT
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(true, MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();
        let allocator = provider.memory_manager.get().unwrap().get_allocator(EfiMemoryType::BootServicesData).unwrap();
        // Add some extra space for the entries.
        let mut xsdt_allocated = Vec::with_capacity_in(ACPI_HEADER_LEN + 16, allocator);
        xsdt_allocated.extend_from_slice(xsdt_table.as_bytes());
        // Fill in trailing space with zeros so it is accessible (Vec length != Vec capacity).
        xsdt_allocated.extend(core::iter::repeat(0u8).take(16));
        let xsdt_data = AcpiXsdtMetadata {
            n_entries: 0,
            max_capacity: MAX_INITIAL_ENTRIES,
            slice: xsdt_allocated.into_boxed_slice(),
        };
        {
            let mut write_guard = provider.xsdt_metadata.write();
            *write_guard = Some(xsdt_data);
        }

        const XSDT_ADDR: u64 = 0x1000_0000_0000_0004;

        let result = provider.add_entry_to_xsdt(XSDT_ADDR);
        assert!(result.is_ok());

        // We should now have 1 entry with address 0x1000_0000_0000_0004
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

        // Try removing the table
        provider.remove_table_from_xsdt(XSDT_ADDR).expect("Removal of entry should succeed.");
        assert_eq!(provider.xsdt_metadata.read().as_ref().unwrap().n_entries, 0);
        // XSDT doesn't have to zero trailing entries, but should reduce length to mark the removed entry as invalid
        assert_eq!(provider.xsdt_metadata.read().as_ref().unwrap().get_length().unwrap(), ACPI_HEADER_LEN as u32);
    }

    #[test]
    fn test_reallocate_xsdt() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(true, MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        // Create a dummy XSDT and add it to system tables
        let xsdt_table = AcpiXsdt {
            header: AcpiTableHeader {
                signature: signature::XSDT,
                // The XSDT is currently "full"
                length: (ACPI_HEADER_LEN + mem::size_of::<u64>() * MAX_INITIAL_ENTRIES) as u32,
                revision: 1,
                checksum: 0,
                oem_id: *b"123456",
                oem_table_id: *b"12345678",
                oem_revision: 1,
                creator_id: 0,
                creator_revision: 0,
            },
        };

        // Fill in the XSDT field.
        let allocator = provider.memory_manager.get().unwrap().get_allocator(EfiMemoryType::BootServicesData).unwrap();
        let mut xsdt_allocated = Vec::with_capacity_in(ACPI_HEADER_LEN + 8, allocator);
        xsdt_allocated.extend_from_slice(xsdt_table.as_bytes());
        // Fill in trailing space with zeros so it is accessible (Vec length != Vec capacity).
        xsdt_allocated.extend(core::iter::repeat(0u8).take(8));
        // Give a small max capacity so the XSDT is forced to reallocate.
        let xsdt_data = AcpiXsdtMetadata { n_entries: 0, max_capacity: 1, slice: xsdt_allocated.into_boxed_slice() };
        {
            let mut write_guard = provider.xsdt_metadata.write();
            *write_guard = Some(xsdt_data);
        }

        // Add one entry for testing.
        const XSDT_ADDR: u64 = 0x1000_0000_0000_0004;
        provider.add_entry_to_xsdt(XSDT_ADDR).expect("Add entry should succeed.");

        provider.reallocate_xsdt().expect("reallocation should succeed");

        // Max entries should increase
        assert_eq!(provider.xsdt_metadata.read().as_ref().unwrap().max_capacity, 2);
        // Existing entries should be preserved
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
    }

    #[test]
    fn test_delete_table_dsdt() {
        let mock_memory_manager = StdMemoryManager::new();

        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(true, MockBootServices::new(), Service::mock(Box::new(mock_memory_manager))).unwrap();
        let allocator = provider.memory_manager.get().unwrap().get_allocator(EfiMemoryType::BootServicesData).unwrap();

        // Create dummy DSDT (pointed to by FADT).
        let dsdt_header = AcpiTableHeader { signature: signature::DSDT, ..Default::default() };
        let dsdt_info = AcpiDsdt { header: dsdt_header };
        let dsdt_allocated = Box::new_in(dsdt_info, allocator);
        let dsdt_addr = dsdt_allocated.as_ref() as *const AcpiDsdt as u64;
        {
            let mut write_guard = provider.system_tables.dsdt.write();
            *write_guard = Some(dsdt_allocated);
        }

        // Dummy FADT in system_tables.
        let fadt_header = AcpiTableHeader { signature: signature::FACP, length: 100, ..Default::default() };
        let fadt_info = AcpiFadt {
            header: fadt_header,
            inner: FadtData { x_dsdt: dsdt_addr, ..Default::default() }, // FADT points to DSDT.
        };
        // Store the dummy FADT so it seems like it's been "installed" since the DSDT is only accessible through the FADT.
        let fadt_allocated = Box::new_in(fadt_info, allocator);
        {
            let mut write_guard = provider.system_tables.fadt.write();
            *write_guard = Some(fadt_allocated);
        }

        let result = provider.delete_table(dsdt_addr as usize, signature::DSDT);
        assert!(result.is_ok());

        // Should have cleared DSDT pointer
        assert!(provider.system_tables.dsdt.read().as_ref().is_none());

        // FADT should no longer point to DSDT
        assert_eq!(provider.system_tables.fadt.read().as_ref().unwrap().x_dsdt(), 0);
    }

    #[test]
    fn test_acpi_table_update_checksum() {
        // Create a fake table of length 16
        let table_length = 16;
        let checksum_offset = 4; // arbitrary offset within [0..16)

        // Fill with a known pattern
        // e.g. bytes = [1,2,3,4,5,...]
        let mut table = (1u8..).take(table_length).collect::<Vec<u8>>();

        // Call the checksum updater
        StandardAcpiProvider::<MockBootServices>::acpi_table_update_checksum(table.as_mut_slice(), checksum_offset);

        // Verify that the sum of all bytes modulo 256 is zero
        let total: u8 = table.iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
        assert_eq!(total, 0, "ACPI checksum failed: table sum = {}", total);
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
        assert!(StandardAcpiProvider::<MockBootServices>::get_xsdt_address_from_rsdp(mock_rsdp(
            signature::ACPI_RSDP_TABLE,
            true,
            ACPI_HEADER_LEN,
            signature::XSDT,
        ))
        .is_ok());
    }
}
