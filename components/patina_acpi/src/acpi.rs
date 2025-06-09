use core::{
    cell::OnceCell,
    mem,
    ptr::{self, copy_nonoverlapping, NonNull},
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering},
};

use crate::{
    acpi, acpi_table::{AcpiTableHeader, MemoryAcpiTable}, alloc::vec::Vec, signature::ACPI_CHECKSUM_OFFSET
};
use crate::{alloc::vec, service::AcpiNotifyFn};

use patina_sdk::boot_services::{BootServices, StandardBootServices};
use patina_sdk::{
    base::UEFI_PAGE_SIZE,
    component::{
        hob::Hob,
        service::{
            memory::{AllocationOptions, MemoryManager, PageAllocation, PageAllocationStrategy},
            IntoService, Service,
        },
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
    signature::{self, ACPI_HEADER_LEN, MAX_INITIAL_ENTRIES},
};

pub static ACPI_TABLE_INFO: StandardAcpiProvider<StandardBootServices> = StandardAcpiProvider::new_uninit();

/// Standard implementation of ACPI services. The service interface can be found in `service.rs`
#[derive(IntoService)]
#[service(dyn AcpiProvider)]
pub(crate) struct StandardAcpiProvider<B: BootServices + 'static> {
    /// Supported ACPI versions.
    /// This implementation only supports ACPI 2.0+. Setting the ACPI 1.0 field does nothing.
    pub(crate) version: AtomicU32,
    /// Whether the platform should reclaim freed ACPI memory.
    /// If true, the ACPI code will try to reclaim unused memory when possible.
    /// If false, all allocations will be made to NVS.
    pub(crate) should_reclaim_memory: AtomicBool,
    /// Known ACPI system tables, such as the FADT, DSDT, etc.
    pub(crate) system_tables: SystemTables,
    /// Platform-installed ACPI tables.
    /// If installing a non-standard ACPI table, the platform is responsible for writing its own handler and parser.
    acpi_tables: RwLock<Vec<MemoryAcpiTable>>,
    /// Stores a monotnically increasing unique table key for installation.
    next_table_key: AtomicUsize,
    /// Stores notify callbacks, which are called upon table installation.
    notify_list: RwLock<Vec<AcpiNotifyFn>>,
    /// Provides boot services.
    pub(crate) boot_services: OnceCell<B>,
    /// Provides memory services.
    pub(crate) memory_manager: OnceCell<Service<dyn MemoryManager>>,
    /// Addresses of currently installed ACPI tables.
    entries: RwLock<Vec<u64>>,
    /// The maximum number of tables that can be installed with currently-allocated ACPI memory.
    /// If `max_entries` is exceeded, the entries will have to be reallocated to a new larger memory space.
    max_entries: AtomicUsize,
}

/// Holds pointers to known system tables used by the ACPI provider.
/// A table may either be null, indicating it is not yet installed or not present on the platform, or point to a valid table in ACPI memory.
pub(crate) struct SystemTables {
    fadt: AtomicPtr<AcpiFadt>,
    facs: AtomicPtr<AcpiFacs>,
    dsdt: AtomicPtr<AcpiDsdt>,
    rsdp: AtomicPtr<AcpiRsdp>,
    pub(crate) xsdt: AtomicPtr<AcpiXsdt>,
}

impl SystemTables {
    pub const fn new() -> Self {
        Self {
            fadt: AtomicPtr::new(core::ptr::null_mut()),
            facs: AtomicPtr::new(core::ptr::null_mut()),
            dsdt: AtomicPtr::new(core::ptr::null_mut()),
            rsdp: AtomicPtr::new(core::ptr::null_mut()),
            xsdt: AtomicPtr::new(core::ptr::null_mut()),
        }
    }
}

/// Functions to retrieve system tables from ACPI memory
impl SystemTables {
    pub fn fadt_mut(&self) -> Option<&mut AcpiFadt> {
        let ptr = self.fadt.load(Ordering::Acquire);
        if ptr.is_null() {
            None
        } else {
            // SAFETY: the pointer is checked to be non-null
            // The caller must make sure that, upon construction, this AtomicPointer holds a valid FADT reference
            Some(unsafe { &mut *ptr })
        }
    }

    pub fn facs_mut(&self) -> Option<&mut AcpiFacs> {
        let ptr = self.facs.load(Ordering::Acquire);
        if ptr.is_null() {
            None
        } else {
            // SAFETY: the pointer is checked to be non-null
            // The caller must make sure that, upon construction, this AtomicPointer holds a valid FADT reference
            Some(unsafe { &mut *ptr })
        }
    }

    pub fn rsdp_mut(&self) -> Option<&mut AcpiRsdp> {
        let ptr = self.rsdp.load(Ordering::Acquire);
        if ptr.is_null() {
            None
        } else {
            // SAFETY: the pointer is checked to be non-null
            // The caller must make sure that, upon construction, this AtomicPointer holds a valid FADT reference
            Some(unsafe { &mut *ptr })
        }
    }

    pub fn xsdt_mut(&self) -> Option<&mut AcpiXsdt> {
        let ptr = self.xsdt.load(Ordering::Acquire);
        if ptr.is_null() {
            None
        } else {
            // SAFETY: the pointer is checked to be non-null
            // The caller must make sure that, upon construction, this AtomicPointer holds a valid FADT reference
            Some(unsafe { &mut *ptr })
        }
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
            version: AtomicU32::new(0),
            should_reclaim_memory: AtomicBool::new(false),
            system_tables: system_tables,
            acpi_tables: RwLock::new(vec![]),
            next_table_key: AtomicUsize::new(1),
            notify_list: RwLock::new(vec![]),
            boot_services: OnceCell::new(),
            memory_manager: OnceCell::new(),
            entries: RwLock::new(vec![]),
            max_entries: AtomicUsize::new(MAX_INITIAL_ENTRIES),
        }
    }

    /// Fills in `StandardAcpiProvider` fields at runtime.
    /// This function must be called before any attempts to use `StandardAcpiProvider`, or a panic will occur.
    /// Attempting to initialize a single `StandardAcpiProvider` instance more than once will also cause a panic.
    pub fn initialize(
        &self,
        version: u32,
        should_reclaim_memory: bool,
        bs: B,
        memory_manager: Service<dyn MemoryManager>,
    ) where
        B: BootServices,
    {
        self.version.store(version, Ordering::Release);
        self.should_reclaim_memory.store(should_reclaim_memory, Ordering::Release);
        if self.boot_services.set(bs).is_err() {
            panic!("Cannot initialize boot services twice.");
        }
        if self.memory_manager.set(memory_manager).is_err() {
            panic!("Cannot initialize memory manager twice.");
        }
    }

    /// Sets the pointer for the RSDP.
    pub fn set_rsdp(&self, rsdp: &mut AcpiRsdp) {
        self.system_tables.rsdp.store(rsdp as *mut AcpiRsdp, Ordering::Release);
    }

    /// Sets the pointer for the XSDT.
    pub fn set_xsdt(&self, xsdt: &mut AcpiXsdt) {
        self.system_tables.xsdt.store(xsdt as *mut AcpiXsdt, Ordering::Release);
    }
}

/// Implementations of ACPI services.
/// For more information on operation and interfaces, see `service.rs`.
impl<B> AcpiProvider for StandardAcpiProvider<B>
where
    B: BootServices,
{
    fn install_acpi_table(&self, acpi_table: &AcpiTableHeader) -> Result<TableKey, AcpiError> {
        let table_key = self.install_acpi_table_in_memory(acpi_table)?;

        self.publish_tables()?;
        self.notify_acpi_list(table_key)?;

        Ok(table_key)
    }

    fn install_facs(&self, facs_info: &AcpiFacs) -> Result<(), AcpiError> {
        let facs_install_addr =
            self.allocate_table_addr(signature::FACS, uefi_size_to_pages!(facs_info.length as usize))?;

        // Point the `x_firmware_ctrl` field of the FADT to the FACS
        let facs_ptr = facs_install_addr as u64;
        self.system_tables.facs.store(facs_ptr as *mut AcpiFacs, Ordering::Release);
        if let Some(fadt) = self.system_tables.fadt_mut() {
            fadt.x_firmware_ctrl = facs_ptr;
        }

        // Update the new in-memory FACS
        if let Some(facs) = self.system_tables.facs_mut() {
            facs.signature = signature::FACS;
            facs.length = facs_info.length;
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

    fn get_acpi_table(&self, index: usize) -> Result<MemoryAcpiTable, AcpiError> {
        let acpi_tables = self.acpi_tables.read();
        let table_at_idx = acpi_tables.get(index).ok_or(AcpiError::InvalidTableIndex)?;
        Ok(table_at_idx.clone())
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

    fn iter(&self) -> Vec<MemoryAcpiTable> {
        self.acpi_tables.read().clone()
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
        if fadt.x_firmware_ctrl != 0 {
            // SAFETY: The FACS has been checked to be non-null.
            // The caller must ensure that the FACS in the HOB is valid
            let facs_ptr = unsafe { &mut *(fadt.x_firmware_ctrl as *mut AcpiFacs) };
            if facs_ptr.signature != signature::FACS {
                return Err(AcpiError::InvalidSignature);
            }
            self.install_facs(facs_ptr)?;
        }

        if fadt.x_dsdt != 0 {
            // The DSDT has a standard ACPI header. Interpret the first 36 bytes as a header.
            // SAFETY: The DSDT has been checked to be non-null.
            let dsdt_header = unsafe { &*(fadt.x_dsdt as *mut AcpiTableHeader) };
            self.install_acpi_table_in_memory(dsdt_header)?;
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
            let table_header = unsafe { &*(entry_addr as *mut AcpiTableHeader) };
            self.install_acpi_table_in_memory(table_header)?;

            // If this table points to other system tables, install them too
            if table_header.signature == signature::FACP {
                // SAFETY: assuming the XSDT entry is written correctly, this points to a valid ACPI table
                // and the signature has been verified to match that of the FADT
                let fadt = unsafe { &*(entry_addr as *const AcpiFadt) };
                self.install_fadt_tables_from_hob(fadt)?;
            }

            Self::acpi_table_update_checksum(entry_addr as *mut u8, table_header.length as usize, ACPI_CHECKSUM_OFFSET);
        }
        self.publish_tables()?;
        Ok(())
    }
}

impl<B> StandardAcpiProvider<B>
where
    B: BootServices,
{
    /// Allocates a new memory location for a given ACPI table, if necessary
    fn allocate_table_addr(&self, signature: u32, n_pages: usize) -> Result<usize, AcpiError> {
        let mut memory_type = self.memory_type();

        // FACS and UEFI table needs to be aligned to 64B
        if signature == signature::FACS || signature == signature::UEFI {
            if UEFI_PAGE_SIZE % 64 != 0 {
                return Err(AcpiError::FacsUefiNot64BAligned);
            }

            // FACS and UEFI table must be allocated in NVS, even if reclaim is enabled
            memory_type = EfiMemoryType::ACPIMemoryNVS;
        }

        let alloc_options =
            AllocationOptions::new().with_memory_type(memory_type).with_strategy(PageAllocationStrategy::Any);
        let page_alloc = self
            .memory_manager
            .get()
            .expect("Memory manager not initialized")
            .allocate_zero_pages(n_pages, alloc_options)
            .map_err(|_e| AcpiError::AllocationFailed)?;

        Ok(page_alloc.into_raw_ptr::<u8>() as usize)
    }

    /// Adds the FADT to the list of installed tables
    fn add_fadt_to_list(
        &self,
        fadt_address: usize,
        fadt_length: usize,
        oem_id: [u8; 6],
        oem_table_id: [u8; 8],
        oem_revision: u32,
    ) -> Result<(), AcpiError> {
        if !(self.system_tables.fadt.load(Ordering::Acquire).is_null()) {
            // FADT already installed, abort
            // SAFETY: By design, MemoryAcpiTable points to a table installed in ACPI memory, so it is safe to free that address
            unsafe {
                self.memory_manager
                    .get()
                    .expect("Memory manager not initialized")
                    .free_pages(fadt_address, uefi_size_to_pages!(fadt_length as usize))
                    .map_err(|_e| AcpiError::FreeFailed)?
            };
            return Err(AcpiError::FadtAlreadyInstalled);
        }

        self.system_tables.fadt.store(fadt_address as *mut AcpiFadt, Ordering::Release);

        let facs_ptr = self.system_tables.facs.load(Ordering::Acquire) as u64;
        let dsdt_ptr = self.system_tables.dsdt.load(Ordering::Acquire) as u64;

        if let Some(fadt) = self.system_tables.fadt_mut() {
            fadt.x_firmware_ctrl = facs_ptr;
            fadt.x_dsdt = dsdt_ptr;
        }

        if let Some(rsdp) = self.system_tables.rsdp_mut() {
            rsdp.oem_id = oem_id;
        }

        // XSDT derives OEM information from FADT, but the FADT does NOT get added to the XSDT entries
        let xsdt_ptr = self.system_tables.xsdt_mut();
        if let Some(xsdt) = xsdt_ptr {
            xsdt.header.oem_id = oem_id;
            xsdt.header.oem_table_id = oem_table_id;
            xsdt.header.oem_revision = oem_revision;
        }

        Ok(())
    }

    /// Points the FADT to the DSDT when the DSDT is installed.
    fn add_dsdt_to_list(&self, physical_addr: usize) {
        let dsdt_ptr = physical_addr as u64;
        if let Some(fadt) = self.system_tables.fadt_mut() {
            fadt.x_dsdt = dsdt_ptr;
        }

        self.system_tables.dsdt.store(physical_addr as *mut AcpiDsdt, Ordering::Release);
    }

    /// Allocates ACPI memory for a new table and adds the table to the list of installed ACPI tables.
    pub(crate) fn install_acpi_table_in_memory(&self, table_header: &AcpiTableHeader) -> Result<TableKey, AcpiError> {
        let physical_addr =
            self.allocate_table_addr(table_header.signature, uefi_size_to_pages!(table_header.length as usize))?;

        // Copy the desired header into the new memory location
        // SAFETY: If allocation suceeds, `dst_ptr` is non-null and large enough to hold the full table.
        let dst_ptr = physical_addr as *mut u8;
        unsafe {
            // Bitwise copy the desired header into the newly allocated table.
            let header_src = table_header as *const AcpiTableHeader as *const u8;
            ptr::copy_nonoverlapping(header_src, dst_ptr, ACPI_HEADER_LEN);
            // Bitwise copy the trailing data into the newly allocated table.
            let payload_dst = dst_ptr.add(ACPI_HEADER_LEN);
            let payload_src = header_src.add(ACPI_HEADER_LEN);
            ptr::copy_nonoverlapping(payload_src, payload_dst, table_header.length as usize - ACPI_HEADER_LEN);
        }

        // Keys must be unique - here we use monotonically increasing
        let next_table_key = self.next_table_key.load(Ordering::Acquire);
        self.next_table_key.store(next_table_key + 1, Ordering::Release);

        // Add the table to the list of installed tables
        let mut installed_table = MemoryAcpiTable::new_from_ptr(dst_ptr as *mut AcpiTableHeader)?;
        installed_table.table_key = next_table_key;
        installed_table.physical_address = Some(physical_addr);
        self.acpi_tables.write().push(installed_table);

        let mut add_to_xsdt = true;
        // Fix up FADT pointers if this table is the FADT or DSDT
        match table_header.signature {
            signature::FACP => {
                add_to_xsdt = false;
                self.add_fadt_to_list(
                    physical_addr,
                    table_header.length as usize,
                    table_header.oem_id,
                    table_header.oem_table_id,
                    table_header.oem_revision,
                )?;
            }
            signature::DSDT => {
                add_to_xsdt = false;
                self.add_dsdt_to_list(physical_addr);
            }
            _ => {}
        }

        let checksum_offset = memoffset::offset_of!(AcpiTableHeader, checksum);
        Self::acpi_table_update_checksum(dst_ptr, table_header.length as usize, checksum_offset);

        if add_to_xsdt {
            self.add_entry_to_xsdt(physical_addr as u64)?;
        }

        self.checksum_common_tables()?;
        Ok(next_table_key)
    }

    /// Determines whether memory allocations should reclaim or store everything in NVS
    pub(crate) fn memory_type(&self) -> EfiMemoryType {
        if self.should_reclaim_memory.load(Ordering::Acquire) {
            EfiMemoryType::ACPIReclaimMemory
        } else {
            EfiMemoryType::ACPIMemoryNVS
        }
    }

    /// Adds an address entry to the XSDT.
    fn add_entry_to_xsdt(&self, new_table_addr: u64) -> Result<(), AcpiError> {
        let xsdt_ptr = self.system_tables.xsdt_mut();

        if let Some(xsdt) = xsdt_ptr {
            // We need to reallocate the XSDT if the buffer exceeds the maximum allocated size
            if self.entries.read().len() == self.max_entries.load(Ordering::Acquire) {
                self.reallocate_xsdt(self.entries.read().len(), xsdt.header.length as usize)?;
            }

            // Calculate the memory location for the new entry (end of XSDT)
            let entry_offset = ACPI_HEADER_LEN + self.entries.read().len() * core::mem::size_of::<u64>();
            let base = self.system_tables.xsdt.load(Ordering::Acquire) as *mut u8;
            // SAFETY: Post-reallocation, we are guaranteed to have enough memory to write the new entry
            // SAFETY: All entries in the XSDT are guaranteed to be u64
            let dst = unsafe { base.add(entry_offset) as *mut u64 };
            // Write entry to ACPI memory
            // This may be unaligned due to the 36B length of the header
            unsafe {
                core::ptr::write_unaligned(dst, new_table_addr);
            }

            // Fix up XSDT struct fields
            self.entries.write().push(new_table_addr);
            xsdt.header.length += mem::size_of::<u64>() as u32;
        }

        Ok(())
    }

    /// Allocates a new, larger memory space for the XSDT when it is full and relocates all entries to the newly allocated memory.
    fn reallocate_xsdt(&self, curr_entries: usize, curr_size: usize) -> Result<(), AcpiError> {
        // Geometrically resize the number of entries in the XSDT
        let new_size = curr_entries * 2 * mem::size_of::<u64>() + ACPI_HEADER_LEN;
        self.max_entries.store(curr_entries * 2, Ordering::Release);

        let alloc_options =
            AllocationOptions::new().with_memory_type(self.memory_type()).with_strategy(PageAllocationStrategy::Any);
        let page_alloc: PageAllocation = self
            .memory_manager
            .get()
            .expect("Memory manager not initialized")
            .allocate_zero_pages(uefi_size_to_pages!(new_size), alloc_options)
            .map_err(|_e| AcpiError::AllocationFailed)?;
        let physical_addr = page_alloc.into_raw_ptr::<u8>();

        let old_addr = self.system_tables.xsdt.load(Ordering::Acquire) as usize;

        // Update RSDP with new XSDT address
        if let Some(rsdp) = self.system_tables.rsdp_mut() {
            rsdp.xsdt_address = physical_addr as u64;
        }

        // Copy over old data to the new XSDT address
        // SAFETY: `physical_addr` is a valid address if allocation succeeds
        // `old_addr` is a valid XSDT in `system_tables`
        unsafe { copy_nonoverlapping(old_addr as *const u8, physical_addr, curr_size) };
        self.system_tables.xsdt.store(physical_addr as *mut AcpiXsdt, Ordering::Release);

        // Free the old XSDT
        // SAFETY: `old_addr` is a valid XSDT in `system_tables`, and was previously allocated during installation by the same memory manager
        unsafe {
            self.memory_manager
                .get()
                .expect("Memory manager not initialized.")
                .free_pages(old_addr, uefi_size_to_pages!(curr_size))
                .map_err(|_e| AcpiError::FreeFailed)?
        };

        Ok(())
    }

    /// Removes a table from the list of installed tables.
    fn remove_table_from_list(&self, table_key: TableKey) -> Result<(), AcpiError> {
        let mut table_for_key = None;
        let mut table_idx = None;

        // Search ACPI tables for corresponding table.
        {
            let acpi_tables = self.acpi_tables.read();
            for (i, memory_table) in acpi_tables.iter().enumerate() {
                // SAFETY: The tables in `self.acpi_tables` are derived from `install_acpi_table`
                // If installation succeeds, they must be valid table references
                if memory_table.table_key == table_key {
                    table_for_key = Some(memory_table.clone());
                    table_idx = Some(i);
                }
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
        let mut remove_from_xsdt = true;
        if signature == signature::FACS || signature == signature::DSDT || signature == signature::FACP {
            remove_from_xsdt = false;
        }

        if remove_from_xsdt {
            self.remove_table_from_xsdt(physical_addr);
        }

        match signature {
            signature::FACP => {
                self.system_tables.fadt.store(core::ptr::null_mut(), Ordering::Release);
            }
            signature::FACS => {
                self.system_tables.facs.store(core::ptr::null_mut(), Ordering::Release);
                // Clear out the FACS pointer in the FADT
                if let Some(fadt) = self.system_tables.fadt_mut() {
                    fadt.x_firmware_ctrl = 0;
                    Self::acpi_table_update_checksum(
                        fadt as *mut AcpiFadt as *mut u8,
                        fadt.header.length as usize,
                        ACPI_CHECKSUM_OFFSET,
                    );
                }
            }
            signature::DSDT => {
                self.system_tables.dsdt.store(core::ptr::null_mut(), Ordering::Release);
                if let Some(fadt) = self.system_tables.fadt_mut() {
                    // Clear out the xDSDT pointer in the FADT
                    fadt.x_dsdt = 0;
                    Self::acpi_table_update_checksum(
                        fadt as *mut AcpiFadt as *mut u8,
                        fadt.header.length as usize,
                        ACPI_CHECKSUM_OFFSET,
                    );
                }
            }
            _ => {}
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
                .expect("Memory manager not initialized")
                .free_pages(physical_addr, uefi_size_to_pages!(table_length))
                .map_err(|_e| AcpiError::FreeFailed)?
        };

        Ok(())
    }

    /// Removes an address entry from the XSDT when a table is uninstalled.
    fn remove_table_from_xsdt(&self, table_address: usize) {
        if self.system_tables.xsdt.load(Ordering::Acquire).is_null() {
            return;
        }

        if let Some(xsdt) = self.system_tables.xsdt_mut() {
            let num_entries = self.entries.read().len();
            // SAFETY: `xsdt` has been verified to be a valid pointer
            let header_ptr = xsdt as *mut AcpiXsdt as *mut u8;
            let entries_base = unsafe { header_ptr.add(ACPI_HEADER_LEN) };

            // Read entries
            // Entries may be unaligned, since the 36-byte ACPI header only guarantees a 4-byte alignment
            let mut entries: Vec<u64> = Vec::with_capacity(num_entries);
            for i in 0..num_entries {
                let ptr_i = unsafe { entries_base.add(i * core::mem::size_of::<u64>()) } as *const u64;
                let entry = unsafe { ptr_i.read_unaligned() };
                entries.push(entry);
            }

            // Find and remove the matching entry
            if let Some(index) = entries.iter().position(|&e| e == table_address as u64) {
                entries.remove(index);

                // Write the entries back to memory, minus the removed one
                for (i, &val) in entries.iter().enumerate() {
                    let ptr_i = unsafe { entries_base.add(i * core::mem::size_of::<u64>()) } as *mut u64;
                    unsafe { ptr_i.write_unaligned(val) };
                }

                // Reduce XSDT length by sizeof(entry)
                xsdt.header.length = xsdt.header.length.saturating_sub(core::mem::size_of::<u64>() as u32);

                // Update local (Rust) list of entries
                self.entries.write().retain(|&e| e != table_address as u64);
            }

            Self::acpi_table_update_checksum(
                xsdt as *mut AcpiXsdt as *mut u8,
                xsdt.header.length as usize,
                ACPI_CHECKSUM_OFFSET,
            );
        }
    }

    /// Recalculates the checksum for an ACPI table.
    /// According to ACPI spec, all bytes of an ACPI table must sum to zero.
    fn acpi_table_update_checksum(table_ptr: *mut u8, table_length: usize, offset: usize) {
        // SAFETY: the caller must ensure `table_length` corresponds to the length of the ACPI table in memory
        let table_bytes = unsafe { core::slice::from_raw_parts_mut(table_ptr, table_length) };
        table_bytes[offset] = 0;

        let total_without_checksum: u32 = table_bytes.iter().map(|&b| b as u32).sum();
        // Negate through complement
        let new_checksum = (!total_without_checksum as u8).wrapping_add(1);
        // SAFETY: The caller must ensure `offset` points to the right checksum offset in the table
        unsafe { ptr::write(table_ptr.add(offset), new_checksum) };
    }

    // Performs checksums on shared ACPI tables (the RSDP and XSDT).
    pub(crate) fn checksum_common_tables(&self) -> Result<(), AcpiError> {
        if let Some(rsdp) = self.system_tables.rsdp_mut() {
            Self::acpi_table_update_checksum(
                rsdp as *mut AcpiRsdp as *mut u8,
                rsdp.length as usize,
                memoffset::offset_of!(AcpiRsdp, extended_checksum),
            );
        }

        if let Some(xsdt) = self.system_tables.xsdt_mut() {
            Self::acpi_table_update_checksum(
                xsdt as *mut AcpiXsdt as *mut u8,
                xsdt.header.length as usize,
                ACPI_CHECKSUM_OFFSET,
            );
        }

        Ok(())
    }

    /// Publishes ACPI tables after installation.
    fn publish_tables(&self) -> Result<(), AcpiError> {
        let rsdp_ptr = self.system_tables.rsdp.load(Ordering::Acquire);
        // SAFETY: If initialization of AcpiProvider succeeds, the RSDP should always be valid
        unsafe {
            self.boot_services
                .get()
                .expect("Boot services not initialized")
                .install_configuration_table(&signature::ACPI_TABLE_GUID, rsdp_ptr)
                .map_err(|_| AcpiError::InstallConfigurationTableFailed)
        }?;

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
                    (*notify_fn)(table_in_memory, self.version.load(Ordering::Acquire), table_key)?;
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
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use patina_sdk::boot_services::MockBootServices;
    use patina_sdk::component::service::memory::MockMemoryManager;
    use patina_sdk::component::service::memory::StdMemoryManager;
    use std::boxed::Box;
    use std::ptr;

    // use std::sync::Once;

    // static INIT: Once = Once::new();

    // fn init_logger() {
    //     INIT.call_once(|| {
    //         env_logger::builder()
    //             .is_test(true) // Ensures logs go to stdout during tests
    //             .init();
    //     });
    // }

    #[test]
    fn test_get_table() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(MockMemoryManager::new())));

        let mut header = AcpiTableHeader { signature: 0x1111, length: 123, ..Default::default() };
        let header_ptr = NonNull::new(&mut header as *mut AcpiTableHeader).unwrap();
        let table = MemoryAcpiTable { header: header_ptr, table_key: 123, physical_address: Some(123) };

        provider.acpi_tables.write().push(table);

        // Call get_acpi_table(0) (should succeed)
        let fetched = provider.get_acpi_table(0).expect("table 0 should exist");
        assert_eq!(fetched.signature(), 0x1111);
        assert_eq!(fetched.length(), 123);

        // Call with an invalid index (should return InvalidTableIndex)
        let err = provider.get_acpi_table(1).unwrap_err();
        assert!(matches!(err, AcpiError::InvalidTableIndex));
    }

    #[test]
    fn test_register_notify() {
        fn dummy_notify(_table: &AcpiTableHeader, _value: u32, _key: TableKey) -> Result<(), AcpiError> {
            Ok(())
        }

        let notify_fn: AcpiNotifyFn = dummy_notify;

        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(MockMemoryManager::new())));

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
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(MockMemoryManager::new())));

        let header1 = AcpiTableHeader { signature: 0x1, length: 10, ..Default::default() };
        let table1 = MemoryAcpiTable { header: header1, ..Default::default() };
        let header2 = AcpiTableHeader { signature: 0x2, length: 20, ..Default::default() };
        let table2 = MemoryAcpiTable { header: header2, ..Default::default() };
        {
            let mut vec = provider.acpi_tables.write();
            vec.push(table1);
            vec.push(table2);
        }

        // Both tables should be in the list and in order
        let result = provider.iter();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].signature(), 0x1);
        assert_eq!(result[0].length(), 10);
        assert_eq!(result[1].signature(), 0x2);
        assert_eq!(result[1].length(), 20);
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
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new())));

        // Dummy FACS and FADT
        let facs = AcpiFacs { signature: signature::FACS, length: 64, ..Default::default() };
        let fadt_header = AcpiTableHeader { signature: signature::FACP, length: 244, ..Default::default() };
        let fadt = AcpiFadt { header: fadt_header, x_firmware_ctrl: 0, ..Default::default() };
        // Store the dummy FADT so it seems like it's been "installed"
        let fadt_ptr = Box::into_raw(Box::new(fadt));
        provider.system_tables.fadt.store(fadt_ptr, Ordering::Release);

        // Make sure FACS pointer was set in `system_tables`
        let res = provider.install_facs(&facs);
        assert!(res.is_ok());
        assert!(!provider.system_tables.facs.load(Ordering::Acquire).is_null());

        // Make sure FACS was installed into FADT
        unsafe {
            let fadt_ref: &AcpiFadt = &*fadt_ptr;
            assert!(fadt_ref.get_x_firmware_ctrl() != 0);
        }
    }

    #[test]
    fn test_add_fadt_to_list() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(MockMemoryManager::new())));

        let fadt_header = AcpiTableHeader {
            signature: signature::FACP,
            length: 128,
            revision: 1,
            checksum: 0,
            oem_id: [0x0, 0x1, 0x2, 0x3, 0x4, 0x5],
            oem_table_id: [0x0, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7],
            oem_revision: 1,
            creator_id: 0,
            creator_revision: 0,
            ..Default::default()
        };
        let fadt_table = MemoryAcpiTable { header: fadt_header, ..Default::default() };
        let boxed_table = Box::new(fadt_table.clone());
        // Treat heap-allocated FADT as if it were in ACPI memory
        let raw_ptr = Box::into_raw(boxed_table);

        let result = provider.add_fadt_to_list(
            raw_ptr as usize,
            fadt_header.length as usize,
            fadt_header.oem_id,
            fadt_header.oem_table_id,
            fadt_header.oem_revision,
        );
        assert!(result.is_ok());

        // FADT should have been added to list
        assert_eq!(provider.system_tables.fadt.load(Ordering::Acquire) as usize, raw_ptr as usize);

        // Clean up
        unsafe { drop(Box::from_raw(raw_ptr)) };
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

        // Calculate the total memory needed (XSDT header + 1 entry written during the test)
        let total_bytes = ACPI_HEADER_LEN + 2 * size_of::<u64>();

        // Get XSDT as pointer
        let mut boxed_buf = vec![0u8; total_bytes].into_boxed_slice();
        let buf_ptr = boxed_buf.as_mut_ptr();
        let xsdt_ptr = buf_ptr as *mut AcpiXsdt;
        unsafe {
            // Fill in exiting XSDT data
            ptr::write(xsdt_ptr, xsdt_table);
        }

        // Initialize XSDT
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(MockMemoryManager::new())));
        provider.system_tables.xsdt.store(xsdt_ptr, Ordering::Relaxed);

        const XSDT_ADDR: u64 = 0x1000_0000_0000_0004;

        let result = provider.add_entry_to_xsdt(XSDT_ADDR);
        assert!(result.is_ok());

        // we should now have 2 entries, the second of which is 0xCAFEBABE
        {
            let new_entries = provider.entries.read();
            assert_eq!(new_entries.len(), 1);
            assert_eq!(new_entries[0], XSDT_ADDR);
        }

        // Verify the memory write of XSDT entry too
        let entry_offset = ACPI_HEADER_LEN;
        let raw_ptr = provider.system_tables.xsdt.load(Ordering::Acquire) as *const u8;
        let written = unsafe {
            // read the 8 bytes at that offset
            let ptr64 = raw_ptr.add(entry_offset) as *const u64;
            ptr64.read_unaligned()
        };
        assert_eq!(written, XSDT_ADDR, "entry was not written correctly to memory");

        // Try removing the table
        provider.remove_table_from_xsdt(XSDT_ADDR as usize);
        {
            let new_entries = provider.entries.read();
            assert_eq!(new_entries.len(), 0);
        }
        // XSDT doesn't have to zero trailing entries, but should reduce length to mark the removed entry as invalid
        assert_eq!(provider.system_tables.xsdt_mut().unwrap().header.length, ACPI_HEADER_LEN as u32);
    }

    #[test]
    fn test_reallocate_xsdt() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new())));

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

        // Write the XSDT into "ACPI" memory (really the Rust heap since we're using StdMemoryManager)
        let page_alloc = provider
            .memory_manager
            .get()
            .unwrap()
            .allocate_zero_pages(uefi_size_to_pages!(xsdt_table.header.length as usize), AllocationOptions::new())
            .unwrap();
        let old_ptr = page_alloc.into_raw_ptr::<u8>();
        let xsdt_ptr = old_ptr as *mut AcpiXsdt;
        unsafe {
            ptr::write(xsdt_ptr, xsdt_table);
        }

        // Add the XSDT to system tables
        provider.system_tables.xsdt.store(xsdt_ptr, Ordering::Relaxed);

        provider
            .reallocate_xsdt(MAX_INITIAL_ENTRIES, xsdt_table.header.length as usize)
            .expect("reallocation should succeed");

        // The XSDT should be moved to a new address
        assert_ne!(old_ptr as usize, provider.system_tables.xsdt.load(Ordering::Acquire) as usize);
        // Max entries should increase
        assert_eq!(provider.max_entries.load(Ordering::Acquire), MAX_INITIAL_ENTRIES * 2);
    }

    #[test]
    fn test_delete_table_dsdt() {
        // init_logger();
        let mut mock_memory_manager = MockMemoryManager::new();
        mock_memory_manager.expect_free_pages().return_once(|_addr, _pages| Ok(()));

        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(mock_memory_manager)));

        // Dummy FADT in system_tables
        let fadt_header = AcpiTableHeader { signature: signature::FACP, length: 100, ..Default::default() };
        let mut fadt = Box::new(AcpiFadt { header: fadt_header, ..Default::default() });
        fadt.x_firmware_ctrl = 0xdead;
        let fadt_ptr = Box::into_raw(fadt);
        provider.system_tables.fadt.store(fadt_ptr, Ordering::Release);

        // Dummy DSDT pointed to by FADT
        let dsdt_header = AcpiTableHeader { signature: signature::DSDT, ..Default::default() };
        let dsdt = Box::new(AcpiDsdt { header: dsdt_header });
        let dsdt_ptr = Box::into_raw(dsdt);
        // Cast and store as AcpiTable pointer
        provider.system_tables.dsdt.store(dsdt_ptr, Ordering::Release);

        let result = provider.delete_table(dsdt_ptr as usize, signature::DSDT, size_of::<AcpiDsdt>());
        assert!(result.is_ok());

        // Should have cleared DSDT pointer
        let dsdt_cleared = provider.system_tables.dsdt.load(Ordering::Acquire);
        assert!(dsdt_cleared.is_null());

        // FADT should no longer point to DSDT
        let fadt_ref = unsafe { &*fadt_ptr };
        assert_eq!(unsafe { fadt_ref.get_x_dsdt() }, 0);
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
        StandardAcpiProvider::<MockBootServices>::acpi_table_update_checksum(
            table.as_mut_ptr(),
            table_length,
            checksum_offset,
        );

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
