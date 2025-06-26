use alloc::boxed::Box;
use core::{
    any::TypeId,
    cell::OnceCell,
    ffi::c_void,
    mem::{self, offset_of},
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use crate::{
    acpi_table::{AcpiTableHeader, AcpiXsdtMetadata, MemoryAcpiTable, RawAcpiTable, StandardAcpiTable},
    alloc::vec::Vec,
    signature::{ACPI_CHECKSUM_OFFSET, ACPI_VERSIONS_GTE_2, ACPI_XSDT_ENTRY_SIZE},
};
use crate::{alloc::vec, service::AcpiNotifyFn};

use patina_sdk::boot_services::{BootServices, StandardBootServices};
use patina_sdk::{
    component::{
        hob::Hob,
        service::{memory::MemoryManager, IntoService, Service},
    },
    efi_types::EfiMemoryType,
    uefi_size_to_pages,
};

use spin::rwlock::RwLock;

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
    /// Whether the platform should reclaim freed ACPI memory.
    /// If true, the ACPI code will try to reclaim unused memory when possible.
    /// If false, all allocations will be made to NVS.
    pub(crate) should_reclaim_memory: AtomicBool,
    /// Known ACPI system tables, such as the FADT, DSDT, etc.
    pub(crate) system_tables: SystemTables,
    /// Platform-installed ACPI tables.
    /// If installing a non-standard ACPI table, the platform is responsible for writing its own handler and parser.
    acpi_tables: RwLock<Vec<MemoryAcpiTable>>,
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
    /// Creates a new `StandardAcpiProvider` with uninitialized fields.
    /// Attempting to use `StandardAcpiProvider` before initialization will cause a panic.
    pub const fn new_uninit() -> Self {
        let system_tables = SystemTables::new();
        Self {
            should_reclaim_memory: AtomicBool::new(false),
            system_tables: system_tables,
            acpi_tables: RwLock::new(vec![]),
            next_table_key: AtomicUsize::new(1),
            notify_list: RwLock::new(vec![]),
            boot_services: OnceCell::new(),
            memory_manager: OnceCell::new(),
            xsdt_metadata: RwLock::new(None),
        }
    }

    /// Fills in `StandardAcpiProvider` fields at runtime.
    /// This function must be called before any attempts to use `StandardAcpiProvider`, or any usages will fail.
    /// Attempting to initialize a single `StandardAcpiProvider` instance more than once will also cause a failure.
    pub fn initialize(
        &self,
        should_reclaim_memory: bool,
        bs: B,
        memory_manager: Service<dyn MemoryManager>,
    ) -> Result<(), AcpiError>
    where
        B: BootServices,
    {
        self.should_reclaim_memory.store(should_reclaim_memory, Ordering::Release);
        if self.boot_services.set(bs).is_err() {
            return Err(AcpiError::BootServicesAlreadyInitialized);
        }
        if self.memory_manager.set(memory_manager).is_err() {
            return Err(AcpiError::MemoryManagerAlreadyInitialized);
        }
        Ok(())
    }

    /// Sets the pointer for the RSDP.
    pub fn set_rsdp(&self, rsdp: Box<AcpiRsdp, &'static dyn alloc::alloc::Allocator>) {
        let mut write_guard = self.system_tables.rsdp.write();
        *write_guard = Some(rsdp);
    }

    /// Sets the pointer for the XSDT.
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
    fn install_acpi_table(&self, acpi_table: &dyn StandardAcpiTable) -> Result<TableKey, AcpiError> {
        let table_signature = acpi_table.header().signature;
        let mut table_key = 0;
        if table_signature == signature::FACP {
            table_key =
                self.install_fadt_in_memory(acpi_table.downcast_ref::<AcpiFadt>().ok_or(AcpiError::InvalidSignature)?)?
        } else if table_signature == signature::DSDT {
            self.add_dsdt_to_list(acpi_table.downcast_ref::<AcpiDsdt>().ok_or(AcpiError::InvalidSignature)?)?;
        } else {
            table_key = self.install_acpi_table_in_memory(acpi_table, acpi_table.type_id())?;
        }

        self.publish_tables()?;
        self.notify_acpi_list(table_key)?;

        Ok(table_key)
    }

    fn install_facs(&self, facs_info: &AcpiFacs) -> Result<(), AcpiError> {
        // Allocate FACS in ACPI NVS memory
        let allocator = self
            .memory_manager
            .get()
            .ok_or(AcpiError::ProviderNotInitialized)?
            .get_allocator(self.allocation_memory_type(signature::FACS))
            .map_err(|_e| AcpiError::AllocationFailed)?;
        let facs_allocated = Box::new_in(*facs_info, allocator);

        // Get address of the FACS to point FADT to it
        let facs_ptr = &*facs_allocated as *const AcpiFacs as u64;

        // Write FACS to system tables
        {
            let mut write_guard = self.system_tables.facs.write();
            *write_guard = Some(facs_allocated);
        }

        // Update the FADT's address pointer to the FACS
        let mut fadt_lock = self.system_tables.fadt.write();
        if let Some(ref mut fadt) = *fadt_lock {
            fadt.set_x_firmware_ctrl(facs_ptr);
        }

        self.checksum_common_tables()?;

        // FACS is not added to the list of installed tables.
        // It is tracked only through the `x_firmware_ctrl` field of the FADT and has no associated table key.
        Ok(())
    }

    fn uninstall_acpi_table(&self, table_key: TableKey) -> Result<(), AcpiError> {
        self.remove_table_from_list(table_key)?;
        self.publish_tables()?;
        Ok(())
    }

    fn get_acpi_table(&self, table_key: TableKey) -> Result<MemoryAcpiTable, AcpiError> {
        let acpi_tables = self.acpi_tables.read();
        for memory_table in acpi_tables.iter() {
            if memory_table.table_key == table_key {
                return Ok(memory_table.clone());
            }
        }
        Err(AcpiError::InvalidTableKey)
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

    /// Returns a `Vec<&AcpiTableHeader>` borrowing directly from each `MemoryAcpiTable`’s `NonNull<…>` pointer.
    fn iter(&self) -> Vec<&AcpiTableHeader> {
        // Read the list of installed ACPI tables and collect their NonNull pointers
        let ptrs: Vec<NonNull<AcpiTableHeader>> = {
            let guard = self.acpi_tables.read();
            guard.iter().map(|table| table.header).collect()
        };

        // Guard has been dropped, but the pointers are still valid
        // Turn each NonNull pointer into a &AcpiTableHeader
        ptrs.into_iter()
            .map(|nn| {
                // SAFETY: ACPI table pointers remain valid as long as they are in the list of installed tables.
                unsafe { nn.as_ref() }
            })
            .collect()
    }
}

impl<B> StandardAcpiProvider<B>
where
    B: BootServices,
{
    /// Retrieves a specific entry from the XSDT.
    /// The XSDT has a standard ACPI header followed by a variable-length list of entries in ACPI memory.
    fn get_xsdt_entry(idx: usize, xsdt_start_ptr: *const u8, xsdt_len: usize) -> Result<u64, AcpiError> {
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

    /// Extracts the XSDT address after performing validation on the RSDP and XSDT
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

    /// Installs the FADT if provided in the HOB list.
    fn install_fadt_tables_from_hob(&self, fadt: &AcpiFadt) -> Result<(), AcpiError> {
        // SAFETY: we assume the FADT set up in the HOB points to a valid FACS if the pointer is non-null
        if fadt.x_firmware_ctrl() != 0 {
            // SAFETY: The FACS has been checked to be non-null.
            // The caller must ensure that the FACS in the HOB is valid
            let facs_ptr = unsafe { &mut *(fadt.inner.x_firmware_ctrl as *mut AcpiFacs) };
            if facs_ptr.signature != signature::FACS {
                return Err(AcpiError::InvalidSignature);
            }
            self.install_facs(facs_ptr)?;
        }

        if fadt.x_dsdt() != 0 {
            // The DSDT has a standard ACPI header. Interpret the first 36 bytes as a header.
            // SAFETY: The DSDT has been checked to be non-null.
            let dsdt_table = unsafe { &*(fadt.x_dsdt() as *mut AcpiDsdt) };
            self.install_acpi_table_in_memory(dsdt_table, TypeId::of::<AcpiDsdt>())?;
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
            // Find the address value of the next XSDT entry
            let entry_addr = Self::get_xsdt_entry(i, xsdt_ptr as *const u8, xsdt_length as usize)?;

            // Each entry points to a table
            let table_from_addr = RawAcpiTable::new_from_address(entry_addr)?;
            // Because we are installing from raw pointers, information about the type of the table cannot be extracted.
            self.install_acpi_table_in_memory(&table_from_addr, TypeId::of::<RawAcpiTable>())?;

            // If this table points to other system tables, install them too
            if table_from_addr.header().signature == signature::FACP {
                // SAFETY: assuming the XSDT entry is written correctly, this points to a valid ACPI table
                // and the signature has been verified to match that of the FADT
                let fadt = unsafe { &*(entry_addr as *const AcpiFadt) };
                self.install_fadt_tables_from_hob(fadt)?;
            }

            Self::acpi_table_update_checksum(table_from_addr.as_bytes_mut(), ACPI_CHECKSUM_OFFSET);
        }
        self.publish_tables()?;
        Ok(())
    }

    // Determines the type of memory to allocate for the table based on platform and table properties.
    fn allocation_memory_type(&self, signature: u32) -> EfiMemoryType {
        let mut memory_type = self.memory_type();

        // FACS and UEFI table needs to be aligned to 64B
        if signature == signature::FACS || signature == signature::UEFI {
            // FACS and UEFI table must be allocated in NVS, even if reclaim is enabled
            memory_type = EfiMemoryType::ACPIMemoryNVS;
        }

        return memory_type;
    }

    /// Allocates memory for the FADT and adds it  to the list of installed tables
    fn install_fadt_in_memory(&self, fadt_info: &AcpiFadt) -> Result<TableKey, AcpiError> {
        if self.system_tables.fadt.read().is_some() {
            // FADT already installed. By spec, only one copy of the FADT should ever be installed, and it cannot be replaced.
            return Err(AcpiError::FadtAlreadyInstalled);
        }

        let allocator = self
            .memory_manager
            .get()
            .ok_or(AcpiError::ProviderNotInitialized)?
            .get_allocator(self.allocation_memory_type(signature::FACS))
            .map_err(|_e| AcpiError::AllocationFailed)?;
        let mut fadt_allocated = Box::new_in(*fadt_info, allocator);

        // Get addresses of tables pointed to by the FADT
        if let Some(facs_address) = self.system_tables.facs_address() {
            fadt_allocated.set_x_firmware_ctrl(facs_address);
        }
        if let Some(dsdt_address) = self.system_tables.dsdt_address() {
            fadt_allocated.set_x_dsdt(dsdt_address);
        }

        // Checksum the FADT after modifying fields
        Self::acpi_table_update_checksum(fadt_allocated.as_bytes_mut(), ACPI_CHECKSUM_OFFSET);

        // Store a pointer to the header.
        let allocated_header = fadt_allocated.as_mut() as *mut AcpiFadt as *mut AcpiTableHeader;
        let physical_addr = fadt_allocated.as_ref() as *const AcpiFadt as usize;

        // The FADT is stored in the XSDT like a normal table. Get the next available table key.
        let next_table_key = self.next_table_key.load(Ordering::Acquire);
        self.next_table_key.store(next_table_key + 1, Ordering::Release);
        // Add the FADT to the XSDT.
        self.add_entry_to_xsdt(physical_addr as u64)?;

        // Add the FADT to the list of installed tables.
        let mut installed_table = MemoryAcpiTable::new_from_ptr(allocated_header)?;
        installed_table.table_key = next_table_key;
        installed_table.physical_address = Some(physical_addr);
        installed_table.type_id = TypeId::of::<AcpiFadt>();

        self.acpi_tables.write().push(installed_table);

        // Store FADT pointer.
        {
            let mut write_guard = self.system_tables.fadt.write();
            *write_guard = Some(fadt_allocated);
        }

        // RSDP derives OEM ID from FADT.
        if let Some(ref mut rsdp) = *self.system_tables.rsdp.write() {
            rsdp.oem_id = fadt_info.header.oem_id;
        }

        // XSDT derives OEM information from FADT.
        if let Some(ref mut xsdt_data) = *self.xsdt_metadata.write() {
            xsdt_data.set_oem_id(fadt_info.header.oem_id);
            xsdt_data.set_oem_table_id(fadt_info.header.oem_table_id);
            xsdt_data.set_oem_revision(fadt_info.header.oem_revision);
        }

        // Checksum root tables after modifying fields.
        self.checksum_common_tables()?;

        Ok(next_table_key)
    }

    /// Allocates memory for the DSDT and installs it.
    /// The DSDT is not in the list of XSDT entries and does not have an associated table key.
    /// It is only accessed through the 'x_dsdt` field of the FADT.
    fn add_dsdt_to_list(&self, dsdt_info: &AcpiDsdt) -> Result<(), AcpiError> {
        let allocator = self
            .memory_manager
            .get()
            .ok_or(AcpiError::ProviderNotInitialized)?
            .get_allocator(self.allocation_memory_type(signature::FACS))
            .map_err(|_e| AcpiError::AllocationFailed)?;
        let dsdt_allocated = Box::new_in(*dsdt_info, allocator);

        // Checksum the newly allocated DSDT
        Self::acpi_table_update_checksum(dsdt_allocated.as_bytes_mut(), ACPI_CHECKSUM_OFFSET);

        // Get address of newly allocated DSDT and set the FADT pointer.
        let dsdt_address = &*dsdt_allocated as *const AcpiDsdt as u64;
        if let Some(ref mut fadt) = *self.system_tables.fadt.write() {
            fadt.set_x_dsdt(dsdt_address);
        }

        // Store pointer to DSDT.
        {
            let mut write_guard = self.system_tables.dsdt.write();
            *write_guard = Some(dsdt_allocated);
        }

        Ok(())
    }

    /// Allocates ACPI memory for a new table and adds the table to the list of installed ACPI tables.
    pub(crate) fn install_acpi_table_in_memory(
        &self,
        table_info: &dyn StandardAcpiTable,
        type_id: TypeId,
    ) -> Result<TableKey, AcpiError> {
        // Copy the bytes into ACPI memory.
        let table_len = table_info.header().length as usize;
        let allocator = self
            .memory_manager
            .get()
            .ok_or(AcpiError::ProviderNotInitialized)?
            .get_allocator(self.allocation_memory_type(signature::FACS))
            .map_err(|_e| AcpiError::AllocationFailed)?;
        let mut table_allocated_bytes = Vec::with_capacity_in(table_len as usize, allocator);
        table_allocated_bytes.extend_from_slice(table_info.as_bytes());
        let mut raw_table = table_allocated_bytes.into_boxed_slice();

        // Recalculate checksum for the newly installed table.
        Self::acpi_table_update_checksum(&mut raw_table, ACPI_CHECKSUM_OFFSET);

        // Store the header. The header is always the first field of any standard ACPI table.
        let allocated_header = raw_table.as_mut_ptr() as *mut AcpiTableHeader;
        let physical_addr = raw_table.as_ptr() as usize;

        // Keys must be unique - here we use monotonically increasing.
        let next_table_key = self.next_table_key.load(Ordering::Acquire);
        self.next_table_key.store(next_table_key + 1, Ordering::Release);

        // Add the table to the list of installed tables.
        let mut installed_table = MemoryAcpiTable::new_from_ptr(allocated_header)?;
        installed_table.table_key = next_table_key;
        installed_table.physical_address = Some(physical_addr);
        installed_table.type_id = type_id;
        self.acpi_tables.write().push(installed_table);

        // Add table to the XSDT.
        self.add_entry_to_xsdt(physical_addr as u64)?;

        // Since XSDT was modified, recalculate checksum for root tables.
        self.checksum_common_tables()?;
        Ok(next_table_key)
    }

    /// Determines whether memory allocations should reclaim or store everything in NVS.
    pub(crate) fn memory_type(&self) -> EfiMemoryType {
        if self.should_reclaim_memory.load(Ordering::Acquire) {
            EfiMemoryType::ACPIReclaimMemory
        } else {
            EfiMemoryType::ACPIMemoryNVS
        }
    }

    /// Adds an address entry to the XSDT.
    fn add_entry_to_xsdt(&self, new_table_addr: u64) -> Result<(), AcpiError> {
        if self.xsdt_metadata.read().is_none() {
            return Ok(());
        }

        let max_capacity = self.xsdt_metadata.read().as_ref().unwrap().max_capacity;
        let curr_capacity = self.xsdt_metadata.read().as_ref().unwrap().nentries;
        // XSDT is full. Reallocate buffer.
        if curr_capacity == max_capacity {
            self.reallocate_xsdt()?;
        }

        if let Some(ref mut xsdt_data) = *self.xsdt_metadata.write() {
            // Next entry goes after header + existing address entries.
            let entry_offset = ACPI_HEADER_LEN + xsdt_data.nentries * ACPI_XSDT_ENTRY_SIZE;
            // Fill in the bytes of the new address entry.
            xsdt_data.slice[entry_offset..entry_offset + ACPI_XSDT_ENTRY_SIZE]
                .copy_from_slice(&new_table_addr.to_le_bytes());

            // Increase XSDT entries by 1.
            xsdt_data.nentries += 1;
            let curr_len = xsdt_data.get_length()?;
            // Write the new length into the header.
            xsdt_data.set_length(curr_len + ACPI_XSDT_ENTRY_SIZE as u32);

            // Checksum the XSDT after modifying it.
            Self::acpi_table_update_checksum(&mut xsdt_data.slice, ACPI_CHECKSUM_OFFSET);
        }

        Ok(())
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
            if let Some(ref mut rsdp) = *self.system_tables.rsdp.write() {
                rsdp.xsdt_address = xsdt_addr;
            }

            // Point to the newly allocated data.
            xsdt_data.slice = xsdt_allocated_bytes.into_boxed_slice();
        }

        Ok(())
    }

    /// Removes a table from the list of installed tables.
    fn remove_table_from_list(&self, table_key: TableKey) -> Result<(), AcpiError> {
        let mut table_for_key = None;
        let mut table_idx = None;

        // Search ACPI tables for corresponding table.
        let acpi_tables = self.acpi_tables.read();
        for (i, memory_table) in acpi_tables.iter().enumerate() {
            // SAFETY: The tables in `self.acpi_tables` are derived from `install_acpi_table`
            // If installation succeeds, they must be valid table references
            if memory_table.table_key == table_key {
                table_for_key = Some(memory_table);
                table_idx = Some(i);
            }
        }

        if let Some(table_to_delete) = table_for_key {
            if let Some(physical_addr) = table_to_delete.physical_address {
                self.delete_table(physical_addr, table_to_delete.signature(), table_to_delete.length() as usize)?;
            } else {
                // Cannot delete a table that was never installed
                return Err(AcpiError::TableNotPresentInMemory);
            }
        } else {
            // No table found with the given key.
            return Err(AcpiError::InvalidTableKey);
        }

        if let Some(table_index) = table_idx {
            self.acpi_tables.write().remove(table_index);
        }

        Ok(())
    }

    /// Deletes a table from the list of installed tables and frees its memory.
    fn delete_table(&self, physical_addr: usize, signature: u32, table_length: usize) -> Result<(), AcpiError> {
        match signature {
            signature::FACP => {
                let mut write_guard = self.system_tables.fadt.write();
                *write_guard = None;
            }
            signature::FACS => {
                let mut write_guard = self.system_tables.facs.write();
                *write_guard = None;

                // Clear out the FACS pointer in the FADT
                if let Some(ref mut fadt) = *self.system_tables.fadt.write() {
                    fadt.set_x_firmware_ctrl(0);
                    Self::acpi_table_update_checksum(fadt.as_bytes_mut(), ACPI_CHECKSUM_OFFSET);
                }
            }
            signature::DSDT => {
                let mut write_guard = self.system_tables.dsdt.write();
                *write_guard = None;

                // Clear out the FACS pointer in the FADT
                if let Some(ref mut fadt) = *self.system_tables.fadt.write() {
                    fadt.set_x_dsdt(0);
                    Self::acpi_table_update_checksum(fadt.as_bytes_mut(), ACPI_CHECKSUM_OFFSET);
                }
            }
            _ => {
                self.remove_table_from_xsdt(physical_addr as u64)?;
            }
        }

        self.free_table_memory(table_length, physical_addr)?;
        Ok(())
    }

    /// Frees memory for a table when it is uninstalled.
    fn free_table_memory(&self, table_length: usize, physical_addr: usize) -> Result<(), AcpiError> {
        // SAFETY: the caller must ensure `table` points to a valid table previously allocated by the same memory manager
        unsafe {
            self.memory_manager
                .get()
                .ok_or(AcpiError::ProviderNotInitialized)?
                .free_pages(physical_addr, uefi_size_to_pages!(table_length))
                .map_err(|_e| AcpiError::FreeFailed)?
        };

        Ok(())
    }

    /// Removes an address entry from the XSDT when a table is uninstalled.
    fn remove_table_from_xsdt(&self, table_address: u64) -> Result<(), AcpiError> {
        if let Some(ref mut xsdt_data) = *self.xsdt_metadata.write() {
            // Calculate where entries are in the slice.
            let entries_nbytes = ACPI_XSDT_ENTRY_SIZE * xsdt_data.nentries;
            let entries_bytes = xsdt_data
                .slice
                .get(ACPI_HEADER_LEN..ACPI_HEADER_LEN + entries_nbytes)
                .ok_or(AcpiError::XsdtOverflow)?;
            // Look for the corresponding entry.
            let index_opt: Option<usize> = entries_bytes
                .chunks_exact(ACPI_XSDT_ENTRY_SIZE)
                .position(|chunk| u64::from_le_bytes(chunk.try_into().unwrap()) == table_address);

            if let Some(idx) = index_opt {
                let start_ptr = ACPI_HEADER_LEN + idx * ACPI_XSDT_ENTRY_SIZE; // Find where the target entry starts.
                let end_ptr = ACPI_HEADER_LEN + xsdt_data.nentries * ACPI_XSDT_ENTRY_SIZE; // Find where the XSDT ends.

                // Shift all entries after the one being removed to the left.
                // [.. before .. | target | <- .. after .. ]
                // becomes [.. before .. | .. after.. ]
                xsdt_data.slice.copy_within(start_ptr + ACPI_XSDT_ENTRY_SIZE..end_ptr, start_ptr);

                // Decrement entries.
                xsdt_data.nentries -= 1;

                // Decrease XSDT length.
                xsdt_data.set_length(xsdt_data.get_length()? - ACPI_XSDT_ENTRY_SIZE as u32);
            }
        }

        self.checksum_common_tables()?;
        Ok(())
    }

    /// Recalculates the checksum for an ACPI table.
    /// According to ACPI spec, all bytes of an ACPI table must sum to zero.
    fn acpi_table_update_checksum(table: &mut [u8], chksm_offset: usize) {
        // Zero the old checksum byte.
        table[chksm_offset] = 0;

        // Sum all bytes (wrapping since the checksum is a u8 between 0-255).
        let sum_of_bytes: u8 = table.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));

        // Write new checksum: equivalent to -1 * `sum_of_bytes` (so the sum is zero modulo 256).
        table[chksm_offset] = sum_of_bytes.wrapping_neg();
    }

    // Performs checksums on shared ACPI tables (the RSDP and XSDT).
    pub(crate) fn checksum_common_tables(&self) -> Result<(), AcpiError> {
        if let Some(ref mut rsdp) = *self.system_tables.rsdp.write() {
            Self::acpi_table_update_checksum(rsdp.as_bytes_mut(), offset_of!(AcpiRsdp, extended_checksum));
        }

        if let Some(ref mut xsdt_data) = *self.xsdt_metadata.write() {
            Self::acpi_table_update_checksum(&mut xsdt_data.slice, ACPI_CHECKSUM_OFFSET);
        }

        Ok(())
    }

    /// Publishes ACPI tables after installation.
    fn publish_tables(&self) -> Result<(), AcpiError> {
        let mut guard = self.system_tables.rsdp.write();

        if let Some(ref mut boxed_rsdp) = *guard {
            // Cast RSDP to raw pointer for boot services
            let ptr: *mut c_void = (&mut (**boxed_rsdp) as *mut AcpiRsdp).cast::<c_void>();

            unsafe {
                self.boot_services
                    .get()
                    .ok_or(AcpiError::ProviderNotInitialized)?
                    // this is usually named something like `install_configuration_table_raw`
                    .install_configuration_table_unchecked(&signature::ACPI_TABLE_GUID, ptr)
                    .map_err(|_| AcpiError::InstallConfigurationTableFailed)?;
            }
        }

        Ok(())
    }

    /// Calls the notify functions in `notify_list` upon installation of an ACPI table.
    fn notify_acpi_list(&self, table_key: TableKey) -> Result<(), AcpiError> {
        let acpi_tables = self.acpi_tables.read();

        // Find the index of the table with the given key.
        if let Some(index) = acpi_tables.iter().position(|table| table.table_key == table_key) {
            // We only perform notify on installed tables.
            if let Some(physical_address) = acpi_tables[index].physical_address {
                // Get a pointer to the APCI table installed in memory.
                // SAFETY: the table references in `iter` are derived from tables installed in `install_acpi_table`
                // If successfully installed, they are guaranteed to be valid table references
                let table_in_memory = unsafe { &*(physical_address as *const AcpiTableHeader) };
                for notify_fn in self.notify_list.read().iter() {
                    (*notify_fn)(table_in_memory, ACPI_VERSIONS_GTE_2, table_key)?;
                }
            } else {
                // If the table is not installed, we cannot notify.
                return Err(AcpiError::TableNotPresentInMemory);
            }
        } else {
            // If the table is not found in the list, we cannot notify.
            return Err(AcpiError::TableNotifyFailed);
        }

        Ok(())
    }

    /// Retrieves a table at a specific index in the list of installed tables.
    /// This is mostly to assist the C protocol.
    pub(crate) fn get_table_at_idx(&self, idx: usize) -> Result<MemoryAcpiTable, AcpiError> {
        let acpi_tables = self.acpi_tables.read();
        if idx < acpi_tables.len() {
            Ok(acpi_tables[idx].clone())
        } else {
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

    // use std::sync::Once;

    // static INIT: Once = Once::new();

    // fn init_logger() {
    //     INIT.call_once(|| {
    //         env_logger::builder()
    //             .is_test(true) // Ensures logs go to stdout during tests
    //             .init();
    //     });
    // }

    struct MockAcpiTable {}

    impl StandardAcpiTable for MockAcpiTable {
        fn header(&self) -> &AcpiTableHeader {
            Box::leak(Box::new(AcpiTableHeader { signature: 0x1111, length: 123, ..Default::default() }))
        }
    }

    #[test]
    fn test_get_table() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(true, MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        let mock_table = MockAcpiTable {};
        let table_key = provider.install_acpi_table_in_memory(&mock_table, TypeId::of::<MockAcpiTable>()).unwrap();

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
        };
        let mut header2 = AcpiTableHeader { signature: 0x2, length: 20, ..Default::default() };
        let header2_ptr = NonNull::new(&mut header2 as *mut AcpiTableHeader).unwrap();
        let table2 = MemoryAcpiTable {
            header: header2_ptr,
            type_id: TypeId::of::<u32>(),
            table_key: 123,
            physical_address: Some(123),
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
        let got0 =
            StandardAcpiProvider::<MockBootServices>::get_xsdt_entry(0, ptr, xsdt_len).expect("entry0 should be valid");
        let got1 =
            StandardAcpiProvider::<MockBootServices>::get_xsdt_entry(1, ptr, xsdt_len).expect("entry1 should be valid");
        assert_eq!(got0, entry0);
        assert_eq!(got1, entry1);

        // Index 2 is out of bounds (we have 2 total entries)
        let err = StandardAcpiProvider::<MockBootServices>::get_xsdt_entry(2, ptr, xsdt_len).unwrap_err();
        assert!(matches!(err, AcpiError::InvalidXsdtEntry));
    }

    #[test]
    fn test_add_facs_to_list() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(true, MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new()))).unwrap();

        // Create dummy data for FACS and FADT.
        let facs_info = AcpiFacs { signature: signature::FACS, length: 64, ..Default::default() };
        let fadt_header = AcpiTableHeader { signature: signature::FACP, length: 244, ..Default::default() };
        let fadt_info = AcpiFadt { header: fadt_header, inner: FadtData::default(), ..Default::default() };
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
            nentries: 0,
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
        assert_eq!(provider.xsdt_metadata.read().as_ref().unwrap().nentries, 1);
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
        assert_eq!(provider.xsdt_metadata.read().as_ref().unwrap().nentries, 0);
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
        let xsdt_data = AcpiXsdtMetadata { nentries: 0, max_capacity: 1, slice: xsdt_allocated.into_boxed_slice() };
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
        // Existing entries should be preseved
        assert_eq!(provider.xsdt_metadata.read().as_ref().unwrap().nentries, 1);
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
        // init_logger();
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

        let result = provider.delete_table(dsdt_addr as usize, signature::DSDT, size_of::<AcpiDsdt>());
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
