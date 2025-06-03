use core::{
    cell::OnceCell,
    mem,
    ptr::{self, copy_nonoverlapping, NonNull},
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering},
};

use crate::{
    acpi_table::AcpiHeader,
    alloc::{boxed::Box, vec::Vec},
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
    acpi_table::{AcpiDsdt, AcpiFacs, AcpiFadt, AcpiInstallable, AcpiRsdp, AcpiTable, AcpiXsdt},
    component::AcpiMemoryHob,
    error::AcpiError,
    service::{AcpiProvider, TableKey},
    signature::{self, ACPI_HEADER_LEN, MAX_INITIAL_ENTRIES},
};

pub static ACPI_TABLE_INFO: StandardAcpiProvider<StandardBootServices> = StandardAcpiProvider::new_uninit();

#[derive(IntoService)]
#[service(dyn AcpiProvider)]
pub(crate) struct StandardAcpiProvider<B: BootServices + 'static> {
    pub(crate) version: AtomicU32,
    pub(crate) should_reclaim_memory: AtomicBool,
    pub(crate) system_tables: SystemTables,
    acpi_tables: RwLock<Vec<NonNull<AcpiTable>>>,
    next_table_key: AtomicUsize,
    notify_list: RwLock<Vec<AcpiNotifyFn>>,
    pub(crate) boot_services: OnceCell<B>,
    pub(crate) memory_manager: OnceCell<Service<dyn MemoryManager>>,
    entries: RwLock<Vec<u64>>,
    max_entries: AtomicUsize,
}

pub(crate) struct SystemTables {
    fadt: AtomicPtr<AcpiFadt>,
    facs: AtomicPtr<AcpiFacs>,
    dsdt: AtomicPtr<AcpiDsdt>,
    rsdp: AtomicPtr<AcpiRsdp>,
    pub xsdt: AtomicPtr<AcpiXsdt>,
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

    pub fn set_rsdp(&self, rsdp: &mut AcpiRsdp) {
        self.system_tables.rsdp.store(rsdp as *mut AcpiRsdp, Ordering::Release);
    }

    pub fn set_xsdt(&self, xsdt: &mut AcpiXsdt) {
        self.system_tables.xsdt.store(xsdt as *mut AcpiXsdt, Ordering::Release);
    }
}

impl<B> AcpiProvider for StandardAcpiProvider<B>
where
    B: BootServices,
{
    fn install_acpi_table(&self, acpi_table: &dyn AcpiInstallable) -> Result<TableKey, AcpiError> {
        let table_key = if acpi_table.signature() == signature::FACS {
            // FACS table has a unique structure and installation requirements
            self.install_facs(acpi_table.phys_addr(), acpi_table.length() as usize)?
        } else {
            // All other tables must follow the generic ACPI table format
            let acpi_table_generic = acpi_table.downcast_ref::<AcpiTable>().ok_or(AcpiError::InvalidTableFormat)?;
            self.add_table_to_list(acpi_table_generic, false)?
        };

        self.publish_tables()?;

        // The FACS is not part of the list of tables, so no notification occurs.
        if acpi_table.signature() != signature::FACS {
            self.notify_acpi_list(table_key)?;
        }

        Ok(table_key)
    }

    fn uninstall_acpi_table(&self, table_key: TableKey) -> Result<(), AcpiError> {
        self.remove_table_from_list(table_key)?;
        self.publish_tables()?;
        Ok(())
    }

    fn get_acpi_table(&self, index: usize) -> Result<&AcpiTable, AcpiError> {
        let acpi_tables = self.acpi_tables.read();
        let table = acpi_tables.get(index).ok_or(AcpiError::InvalidTableIndex)?;
        // SAFETY: the table references in `get` are derived from tables installed in `install_acpi_table`
        // If successfully installed, they are guaranteed to be valid table references
        Ok(unsafe { table.as_ref() })
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

    fn iter<'a>(&'a self) -> Box<dyn Iterator<Item = &'a AcpiTable> + 'a> {
        let guard = self.acpi_tables.read();
        // SAFETY: the table references in `iter` are derived from tables installed in `install_acpi_table`
        // If successfully installed, they are guaranteed to be valid table references
        let acpi_table_refs: Vec<&AcpiTable> = guard.iter().map(|ptr| unsafe { ptr.as_ref() }).collect();
        drop(guard);
        Box::new(acpi_table_refs.into_iter())
    }
}

impl<B> StandardAcpiProvider<B>
where
    B: BootServices,
{
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
    /// SHERRY: need to test
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
        let xsdt_header = rsdp.xsdt_address as *const AcpiHeader;
        if (unsafe { *xsdt_header }).signature != signature::XSDT {
            return Err(AcpiError::InvalidSignature);
        }

        // SAFETY: We validate that the XSDT is non-null and contains the right signature.
        let xsdt_ptr = rsdp.xsdt_address as *mut AcpiXsdt;
        let xsdt = unsafe { &*(xsdt_ptr) };

        if xsdt.length < ACPI_HEADER_LEN as u32 {
            return Err(AcpiError::XsdtInvalidLengthFromHob);
        }

        Ok(rsdp.xsdt_address)
    }

    fn install_fadt_tables_from_hob(&self, fadt: AcpiFadt) -> Result<(), AcpiError> {
        // SAFETY: we assume the FADT set up in the HOB points to a valid FACS if the pointer is non-null
        if fadt.x_firmware_ctrl != 0 {
            // SAFETY: The FACS has been checked to be non-null
            // The caller must ensure that the FACS in the HOB is valid
            let facs_ptr = fadt.x_firmware_ctrl as *mut AcpiFacs;
            let facs_len = unsafe { (*facs_ptr).length as usize };
            self.install_facs(Some(facs_ptr as usize), facs_len)?;
        }

        if fadt.x_dsdt != 0 {
            let dsdt_ptr = fadt.x_dsdt as *mut AcpiTable;

            // SAFETY: The FADT in the HOB must have a valid pointer to the DSDT
            // Set the physical address for installation, since it already exists from the HOB
            unsafe {
                (*dsdt_ptr).physical_address = Some(dsdt_ptr as usize);
            }

            // SAFETY: The FADT in the HOB must have a valid pointer to the DSDT
            self.add_table_to_list(unsafe { &*(dsdt_ptr) }, true)?;
        }

        Ok(())
    }

    pub fn install_tables_from_hob(&self, acpi_hob: Hob<AcpiMemoryHob>) -> Result<(), AcpiError> {
        let xsdt_address = Self::get_xsdt_address_from_rsdp(acpi_hob.rsdp_address)?;
        let xsdt_ptr = xsdt_address as *const AcpiXsdt;

        // SAFETY: `get_xsdt_address_from_rsdp` should perform necessary validations on XSDT
        let xsdt_length = (unsafe { *xsdt_ptr }).length;

        let entries = (xsdt_length as usize - ACPI_HEADER_LEN) / mem::size_of::<u64>();
        for i in 0..entries {
            // Find the address value of the next XSDT entry
            let entry_addr = Self::get_xsdt_entry(i, xsdt_ptr as *const u8, xsdt_length as usize)?;

            // Each entry points to a table
            // SAFETY: we assume that the HOB passed in contains valid ACPI table entries
            let table = unsafe { &*(entry_addr as *const AcpiTable) };

            self.add_table_to_list(table, true)?;

            // If this table points to other system tables, install them too
            if table.signature == signature::FACP {
                // Note that because we clone here, this AcpiFadt is a a local (Rust stack) variable
                // And does not actually point to an AcpiFadt in ACPI memory
                // However, this is safe because we only care about the fields of the FADT, not the struct itself
                let fadt = AcpiFadt::try_from(table.clone())?;
                self.install_fadt_tables_from_hob(fadt)?;
            }

            let checksum_offset = memoffset::offset_of!(AcpiTable, checksum);
            Self::acpi_table_update_checksum(entry_addr as *mut u8, table.length as usize, checksum_offset);
            self.add_table_to_list(table, true)?;
        }
        self.publish_tables()?;
        Ok(())
    }
}

impl<B> StandardAcpiProvider<B>
where
    B: BootServices,
{
    /// Adds the FACS to the list of installed tables
    /// Due to the unique format of the FACS, it has different installation requirements than other tables
    pub(crate) fn install_facs(&self, facs_addr: Option<usize>, facs_len: usize) -> Result<TableKey, AcpiError> {
        let facs_install_addr = if let Some(facs_mem_addr) = facs_addr {
            facs_mem_addr
        } else {
            self.allocate_table_addr(signature::FACS, None, false, uefi_size_to_pages!(facs_len))?
        };

        // Point the `x_firmware_ctrl` field of the FADT to the FACS
        let facs_ptr = facs_install_addr as u64;
        self.system_tables.facs.store(facs_ptr as *mut AcpiFacs, Ordering::Release);
        if let Some(fadt) = self.system_tables.fadt_mut() {
            fadt.x_firmware_ctrl = facs_ptr;
        }

        // Update the new in-memory FACS
        if let Some(facs) = self.system_tables.facs_mut() {
            facs.signature = signature::FACS;
            facs.length = facs_len as u32;
        }

        self.checksum_common_tables()?;

        // Return a dummy value for the FACS to match the `install_acpi_table` function format
        Ok(0 as TableKey)
    }

    /// Allocates a new memory location for a given ACPI table, if necessary
    fn allocate_table_addr(
        &self,
        signature: u32,
        table_addr: Option<usize>,
        is_from_hob: bool,
        n_pages: usize,
    ) -> Result<usize, AcpiError> {
        // If the table is from the HOB, it should already be installed in ACPI memory
        if is_from_hob && table_addr.is_none() {
            return Err(AcpiError::HobTableNotInstalled);
        }

        let mut memory_type = self.memory_type();

        // FACS and UEFI table needs to be aligned to 64B
        if signature == signature::FACS || signature == signature::UEFI {
            if UEFI_PAGE_SIZE % 64 != 0 {
                return Err(AcpiError::FacsUefiNot64BAligned);
            }

            // For FACS and UEFI only, reallocation isn't necessary if passed in from the HOB
            // since they are pre-aligned and allocated in NVS memory
            if is_from_hob {
                let table_addr = table_addr.expect("Table should be installed in ACPI memory.");
                return Ok(table_addr);
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
    /// `physical_addr`: the address in ACPI memory to install the FADT
    /// `table`: contains data for the FADT
    fn add_fadt_to_list(&self, physical_addr: usize, table: &AcpiTable) -> Result<(), AcpiError> {
        if !(self.system_tables.fadt.load(Ordering::Acquire).is_null()) {
            // FADT already installed, abort
            // SAFETY: The caller must ensure that `physical_addr` points to a memory location previously allocated by the same memory manager
            unsafe {
                self.memory_manager
                    .get()
                    .expect("Memory manager not initialized")
                    .free_pages(physical_addr, uefi_size_to_pages!(table.length as usize))
                    .map_err(|_e| AcpiError::FreeFailed)?
            };
            return Err(AcpiError::FadtAlreadyInstalled);
        }

        self.system_tables.fadt.store(physical_addr as *mut AcpiFadt, Ordering::Release);

        let facs_ptr = self.system_tables.facs.load(Ordering::Acquire) as u64;
        let dsdt_ptr = self.system_tables.dsdt.load(Ordering::Acquire) as u64;

        if let Some(fadt) = self.system_tables.fadt_mut() {
            fadt.x_firmware_ctrl = facs_ptr;
            fadt.x_dsdt = dsdt_ptr;
        }

        if let Some(rsdp) = self.system_tables.rsdp_mut() {
            rsdp.oem_id = table.oem_id;
        }

        // XSDT derives OEM information from FADT, but the FADT does NOT get added to the XSDT entries
        let xsdt_ptr = self.system_tables.xsdt_mut();
        if let Some(xsdt) = xsdt_ptr {
            xsdt.oem_id = table.oem_id;
            xsdt.oem_table_id = table.oem_table_id;
            xsdt.oem_revision = table.oem_revision;
        }

        Ok(())
    }

    fn add_dsdt_to_list(&self, physical_addr: usize) {
        let dsdt_ptr = physical_addr as u64;
        if let Some(fadt) = self.system_tables.fadt_mut() {
            fadt.x_dsdt = dsdt_ptr;
        }
    }

    pub(crate) fn add_table_to_list(&self, table: &AcpiTable, is_from_hob: bool) -> Result<TableKey, AcpiError> {
        let table_addr = table.phys_addr();

        let physical_addr = self.allocate_table_addr(
            table.signature(),
            table_addr,
            is_from_hob,
            uefi_size_to_pages!(table.length() as usize),
        )?;

        // Copy data from table into new memory location
        let dst_ptr = physical_addr as *mut u8;
        let table_ptr = table as *const AcpiTable as *const u8;
        // SAFETY: caller must ensure `table` is a valid pointer to an ACPI table. `dst_ptr` is always valid assuming memory allocation succeeds
        // Copy the generic AcpiTable data into memory, then the table-specific trailing data
        unsafe {
            ptr::copy_nonoverlapping(table_ptr, dst_ptr, ACPI_HEADER_LEN + table.length as usize);
        }

        // Keys must be unique - here we use monotonically increasing
        let next_table_key = self.next_table_key.load(Ordering::Acquire);
        self.next_table_key.store(next_table_key + 1, Ordering::Release);
        // SAFETY: caller must ensure `table` is a valid pointer to an ACPI table
        let dst_table = unsafe { &mut *(physical_addr as *mut AcpiTable) };

        // Fix up ACPI table struct fields
        dst_table.table_key = next_table_key;
        dst_table.physical_address = Some(physical_addr);

        self.acpi_tables
            .write()
            .push(NonNull::new(dst_ptr as *mut AcpiTable).expect("Allocated table must not be null"));

        let mut add_to_xsdt = true;
        // Fix up FADT pointers if this table is the FADT or DSDT
        match table.signature() {
            signature::FACP => {
                add_to_xsdt = false;
                self.add_fadt_to_list(physical_addr, table)?;
            }
            signature::DSDT => {
                add_to_xsdt = false;
                self.add_dsdt_to_list(physical_addr);
            }
            _ => {}
        }

        let checksum_offset = memoffset::offset_of!(AcpiTable, checksum);
        Self::acpi_table_update_checksum(dst_ptr, table.length as usize, checksum_offset);

        if add_to_xsdt {
            self.add_entry_to_xsdt(physical_addr as u64)?;
        }

        self.checksum_common_tables()?;
        Ok(next_table_key)
    }

    pub(crate) fn memory_type(&self) -> EfiMemoryType {
        if self.should_reclaim_memory.load(Ordering::Acquire) {
            EfiMemoryType::ACPIReclaimMemory
        } else {
            EfiMemoryType::ACPIMemoryNVS
        }
    }

    fn add_entry_to_xsdt(&self, new_table_addr: u64) -> Result<(), AcpiError> {
        let xsdt_ptr = self.system_tables.xsdt_mut();

        if let Some(xsdt) = xsdt_ptr {
            // We need to reallocate the XSDT if the buffer exceeds the maximum allocated size
            if self.entries.read().len() == self.max_entries.load(Ordering::Acquire) {
                self.reallocate_xsdt(self.entries.read().len(), xsdt.length as usize)?;
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
            xsdt.length += mem::size_of::<u64>() as u32;
        }

        Ok(())
    }

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

    fn remove_table_from_list(&self, table_key: TableKey) -> Result<(), AcpiError> {
        let mut table_for_key = None;
        let mut table_idx = None;
        for (i, ptr) in self.acpi_tables.write().iter_mut().enumerate() {
            // SAFETY: The tables in `self.acpi_tables` are derived from `install_acpi_table`
            // If installation succeeds, they must be valid table references
            let table = unsafe { ptr.as_mut() };
            if table.table_key == table_key {
                table_for_key = Some(table);
                table_idx = Some(i);
            }
        }

        if table_for_key.is_none() {
            return Err(AcpiError::InvalidTableKey);
        }

        self.delete_table(table_for_key.unwrap())?;

        if let Some(table_index) = table_idx {
            self.acpi_tables.write().remove(table_index);
        }
        Ok(())
    }

    fn delete_table(&self, table: &mut AcpiTable) -> Result<(), AcpiError> {
        let mut remove_from_xsdt = true;
        let current_signature = table.signature;
        if current_signature == signature::FACS
            || current_signature == signature::DSDT
            || current_signature == signature::FACP
        {
            remove_from_xsdt = false;
        }

        if remove_from_xsdt {
            self.remove_table_from_xsdt(table);
        }

        match table.signature {
            signature::FACP => {
                self.system_tables.fadt.store(core::ptr::null_mut(), Ordering::Release);
            }
            signature::FACS => {
                self.system_tables.facs.store(core::ptr::null_mut(), Ordering::Release);
                // Update other tables pointing to the FACS
                if let Some(fadt) = self.system_tables.fadt_mut() {
                    fadt.x_firmware_ctrl = 0;
                    Self::acpi_table_update_checksum(
                        fadt as *mut AcpiFadt as *mut u8,
                        fadt.length as usize,
                        memoffset::offset_of!(AcpiTable, checksum),
                    );
                }
            }
            signature::DSDT => {
                self.system_tables.dsdt.store(core::ptr::null_mut(), Ordering::Release);

                if let Some(fadt) = self.system_tables.fadt_mut() {
                    fadt.x_dsdt = 0;
                    Self::acpi_table_update_checksum(
                        fadt as *mut AcpiFadt as *mut u8,
                        fadt.length as usize,
                        memoffset::offset_of!(AcpiTable, checksum),
                    );
                }
            }
            _ => {}
        }

        self.free_table_memory(table)?;
        Ok(())
    }

    fn free_table_memory(&self, table: &AcpiTable) -> Result<(), AcpiError> {
        // SAFETY: the caller must ensure `table` points to a valid table previously allocated by the same memory manager
        unsafe {
            self.memory_manager
                .get()
                .expect("Memory manager not initialized")
                .free_pages((table as *const AcpiTable) as usize, uefi_size_to_pages!(table.length as usize))
                .map_err(|_e| AcpiError::FreeFailed)?
        };

        Ok(())
    }

    fn remove_table_from_xsdt(&self, table: &AcpiTable) {
        if self.system_tables.xsdt.load(Ordering::Acquire).is_null() {
            return;
        }

        let table_address = table as *const AcpiTable as usize;
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
                xsdt.length = xsdt.length.saturating_sub(core::mem::size_of::<u64>() as u32);

                // Update local (Rust) list of entries
                self.entries.write().retain(|&e| e != table_address as u64);
            }

            Self::acpi_table_update_checksum(
                xsdt as *mut AcpiXsdt as *mut u8,
                xsdt.length as usize,
                memoffset::offset_of!(AcpiTable, checksum),
            );
        }
    }

    // Sum of all ACPI bytes (should be zero)
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

    // checksum RSDP and XSDT
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
                xsdt.length as usize,
                memoffset::offset_of!(AcpiTable, checksum),
            );
        }

        Ok(())
    }

    fn publish_tables(&self) -> Result<(), AcpiError> {
        let rsdp_ptr = self.system_tables.rsdp.load(Ordering::Acquire);
        // SAFETY: If initialization of AcpiProvider succeeds, the RSDP should always be valid
        unsafe {
            self.boot_services
                .get()
                .expect("Boot services not initialized")
                .install_configuration_table(&signature::ACPI_TABLE_GUID, rsdp_ptr)
                .map_err(|_| AcpiError::InstallTableFailed)
        }?;

        Ok(())
    }

    fn notify_acpi_list(&self, table_key: TableKey) -> Result<(), AcpiError> {
        let acpi_tables = self.acpi_tables.read();
        // SAFETY: the table references in `iter` are derived from tables installed in `install_acpi_table`
        // If successfully installed, they are guaranteed to be valid table references
        if let Some(index) = acpi_tables.iter().position(|&table| unsafe { table.as_ref().table_key } == table_key) {
            let table = unsafe { acpi_tables[index].as_ref() };
            for notify_fn in self.notify_list.read().iter() {
                (*notify_fn)(table, self.version.load(Ordering::Acquire), table_key)?;
            }
        } else {
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

    #[test]
    fn test_get_table() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(MockMemoryManager::new())));

        // Dummy AcpiTable (on heap, not actually in ACPI memory)
        let table = Box::new(AcpiTable { signature: 0x1111, length: 123, ..Default::default() });
        let table_ptr = NonNull::new(Box::into_raw(table)).unwrap();

        provider.acpi_tables.write().push(table_ptr);

        // Call get_acpi_table(0) (should succeed)
        let fetched: &AcpiTable = provider.get_acpi_table(0).expect("table 0 should exist");
        assert_eq!(fetched.signature, 0x1111);
        assert_eq!(fetched.length, 123);

        // Call with an invalid index (should return InvalidTableIndex)
        let err = provider.get_acpi_table(1).unwrap_err();
        assert!(matches!(err, AcpiError::InvalidTableIndex));
    }

    #[test]
    fn test_register_notify() {
        fn dummy_notify(_table: &AcpiTable, _value: u32, _key: TableKey) -> Result<(), AcpiError> {
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

        let table1 = Box::new(AcpiTable { signature: 0x1, length: 10, ..Default::default() });
        let table2 = Box::new(AcpiTable { signature: 0x2, length: 20, ..Default::default() });

        let ptr1 = NonNull::new(Box::into_raw(table1)).unwrap();
        let ptr2 = NonNull::new(Box::into_raw(table2)).unwrap();

        {
            let mut vec = provider.acpi_tables.write();
            vec.push(ptr1);
            vec.push(ptr2);
        }

        // Both tables should be in the list and in order
        let result: Vec<&AcpiTable> = provider.iter().collect();
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
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(MockMemoryManager::new())));

        // Dummy FACS and FADT
        let facs = Box::new(AcpiFacs { signature: signature::FACS, length: 64, ..Default::default() });
        let facs_ptr = Box::into_raw(facs) as usize;
        let fadt =
            Box::new(AcpiFadt { signature: signature::FACP, length: 244, x_firmware_ctrl: 0, ..Default::default() });
        let fadt_ptr = Box::into_raw(fadt);
        provider.system_tables.fadt.store(fadt_ptr, Ordering::Release);

        // This should not fail, and should return 0 (FACS does not have an associated TableKey)
        let res = provider.install_facs(Some(facs_ptr), 64);
        assert_eq!(res.unwrap(), 0);

        // Make sure FACS was installed into FADT
        unsafe {
            let fadt_ref: &AcpiFadt = &*fadt_ptr;
            assert_eq!(fadt_ref.get_x_firmware_ctrl(), facs_ptr as u64);
        }
    }

    #[test]
    fn test_allocate_addr_from_hob() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(MockMemoryManager::new())));
        let fake_addr = 0x12341234;
        let addr_result = provider
            .allocate_table_addr(signature::FACS, Some(fake_addr), true, 1)
            .expect("should return the same HOB address");
        // If installing FACS from HOB, it should use the preallocated address instead of allocating new ACPI memory
        assert_eq!(addr_result, fake_addr);
    }

    #[test]
    fn test_allocate_addr() {
        let mut mock_memory_manager = MockMemoryManager::new();
        mock_memory_manager.expect_allocate_zero_pages().return_once(|_, _| {
            Ok(unsafe {
                PageAllocation::new(UEFI_PAGE_SIZE, 5, Box::leak(Box::new(MockMemoryManager::new()))).unwrap()
            })
        });
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(mock_memory_manager)));

        let returned = provider
            .allocate_table_addr(0, Some(0x1234), false, 1)
            .expect("should succeed via our DummyMemManagerSuccess");

        // When not from a HOB, `allocate_table_addr` use a newly allocated memory address
        assert_eq!(returned, UEFI_PAGE_SIZE);
    }

    #[test]
    fn test_add_fadt_to_list() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(MockMemoryManager::new())));

        let fadt_table = AcpiTable {
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
        let boxed_table = Box::new(fadt_table.clone());
        // Treat heap-allocated FADT as if it were in ACPI memory
        let raw_ptr = Box::into_raw(boxed_table);

        let result = provider.add_fadt_to_list(raw_ptr as usize, &fadt_table);
        assert!(result.is_ok());

        // FADT should have been added to list
        assert_eq!(provider.system_tables.fadt.load(Ordering::Acquire) as usize, raw_ptr as usize);

        // Clean up
        unsafe { drop(Box::from_raw(raw_ptr)) };
    }

    #[test]
    fn test_add_and_remove_xsdt() {
        let xsdt_table = AcpiXsdt {
            signature: signature::XSDT,
            length: ACPI_HEADER_LEN as u32, // XSDT currently has no entries
            revision: 1,
            checksum: 0,
            oem_id: *b"123456",
            oem_table_id: *b"12345678",
            oem_revision: 1,
            creator_id: 0,
            creator_revision: 0,
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
        let removal_addr = unsafe { &*(XSDT_ADDR as *const AcpiTable) };
        provider.remove_table_from_xsdt(removal_addr);
        {
            let new_entries = provider.entries.read();
            assert_eq!(new_entries.len(), 0);
        }
        // XSDT doesn't have to zero trailing entries, but should reduce length to mark the removed entry as invalid
        assert_eq!(provider.system_tables.xsdt_mut().unwrap().length, ACPI_HEADER_LEN as u32);
    }

    #[test]
    fn test_reallocate_xsdt() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(StdMemoryManager::new())));

        // Create a dummy XSDT and add it to system tables
        let xsdt_table = AcpiXsdt {
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
        };

        // Write the XSDT into "ACPI" memory (really the Rust heap since we're using StdMemoryManager)
        let page_alloc = provider
            .memory_manager
            .get()
            .unwrap()
            .allocate_zero_pages(uefi_size_to_pages!(xsdt_table.length as usize), AllocationOptions::new())
            .unwrap();
        let old_ptr = page_alloc.into_raw_ptr::<u8>();
        let xsdt_ptr = old_ptr as *mut AcpiXsdt;
        unsafe {
            ptr::write(xsdt_ptr, xsdt_table);
        }

        // Add the XSDT to system tables
        provider.system_tables.xsdt.store(xsdt_ptr, Ordering::Relaxed);

        provider.reallocate_xsdt(MAX_INITIAL_ENTRIES, xsdt_table.length as usize).expect("reallocation should succeed");

        // The XSDT should be moved to a new address
        assert_ne!(old_ptr as usize, provider.system_tables.xsdt.load(Ordering::Acquire) as usize);
        // Max entries should increase
        assert_eq!(provider.max_entries.load(Ordering::Acquire), MAX_INITIAL_ENTRIES * 2);
    }

    #[test]
    fn test_delete_table_dsdt() {
        let mut mock_memory_manager = MockMemoryManager::new();
        mock_memory_manager.expect_free_pages().return_once(|_addr, _pages| Ok(()));

        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(mock_memory_manager)));

        // Dummy FADT in system_tables
        let mut fadt = Box::new(AcpiFadt { signature: signature::FACP, length: 100, ..Default::default() });
        fadt.x_firmware_ctrl = 0xdead;
        let fadt_ptr = Box::into_raw(fadt);
        provider.system_tables.fadt.store(fadt_ptr, Ordering::Release);

        // Dummy DSDT pointed to by FADT
        let real_dsdt = Box::new(AcpiDsdt { signature: signature::DSDT, ..Default::default() });
        let dsdt_ptr = Box::into_raw(real_dsdt);
        // Cast and store as AcpiTable pointer
        provider.system_tables.dsdt.store(dsdt_ptr, Ordering::Release);

        let table_ref: &mut AcpiTable = unsafe { &mut *(dsdt_ptr as *mut AcpiTable) };
        let result = provider.delete_table(table_ref);
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

            xsdt_buf.into_boxed_slice().as_ptr() as u64
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

        rsdp_buf.into_boxed_slice().as_ptr() as u64
    }

    #[test]
    fn test_get_xsdt_address() {
        /*
        TEST NULL RSDP
         */
        assert_eq!(
            StandardAcpiProvider::<MockBootServices>::get_xsdt_address_from_rsdp(0).unwrap_err(),
            AcpiError::NullRsdpFromHob
        );

        /*
        TEST INVALID RSDP SIGNATURE
        */
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
