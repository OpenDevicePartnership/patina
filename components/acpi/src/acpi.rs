use core::ptr::NonNull;
use core::{mem, ptr};

use core::cell::OnceCell;
use core::ptr::copy_nonoverlapping;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use spin::RwLockReadGuard;
use spin::{Mutex, Once, RwLock};
use uefi_sdk::boot_services::tpl::Tpl;
use uefi_sdk::component::hob::Hob;
use uefi_sdk::component::service::memory::{
    self, AllocationOptions, MemoryManager, PageAllocation, PageAllocationStrategy,
};
use uefi_sdk::component::service::{IntoService, Service};
use uefi_sdk::efi_types::EfiMemoryType;
use uefi_sdk::tpl_mutex::TplMutex;

use alloc::vec;
use bitflags::bitflags;
use uefi_sdk::{
    base::UEFI_PAGE_SIZE,
    boot_services::{
        allocation::{AllocType, MemoryType},
        BootServices, StandardBootServices,
    },
    uefi_size_to_pages,
};

use crate::acpi_table::{AcpiDsdt, AcpiFacs, AcpiFadt, AcpiInstallable, AcpiRsdp, AcpiTable, AcpiXsdt};
use crate::component::AcpiMemoryHob;
use crate::error::AcpiError;
use crate::service::{AcpiNotifyFn, AcpiProvider, TableKey};
use crate::signature::ACPI_HEADER_LEN;
use crate::signature::{self};

pub static ACPI_TABLE_INFO: StandardAcpiProvider<StandardBootServices> = StandardAcpiProvider::new_uninit();

#[derive(IntoService)]
#[service(dyn AcpiProvider)]
pub struct StandardAcpiProvider<B: BootServices + 'static> {
    pub version: AtomicU32,
    pub signature: u32,
    pub should_reclaim_memory: AtomicBool,
    pub(crate) system_tables: Mutex<SystemTables>,
    acpi_tables: RwLock<Vec<NonNull<AcpiTable>>>,
    next_table_key: AtomicUsize,
    notify_list: RwLock<Vec<AcpiNotifyFn>>,
    pub(crate) boot_services: OnceCell<B>,
    memory_manager: OnceCell<Service<dyn MemoryManager>>,
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
            Some(unsafe { &mut *ptr })
        }
    }

    pub fn facs_mut(&self) -> Option<&mut AcpiFacs> {
        let ptr = self.facs.load(Ordering::Acquire);
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { &mut *ptr })
        }
    }

    pub fn dsdt_mut(&self) -> Option<&mut AcpiDsdt> {
        let ptr = self.dsdt.load(Ordering::Acquire);
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { &mut *ptr })
        }
    }

    pub fn rsdp_mut(&self) -> Option<&mut AcpiRsdp> {
        let ptr = self.rsdp.load(Ordering::Acquire);
        if ptr.is_null() {
            None
        } else {
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

unsafe impl<B> Sync for StandardAcpiProvider<B> where B: BootServices + Sync {}

unsafe impl<B> Send for StandardAcpiProvider<B> where B: BootServices + Send {}

impl<B> StandardAcpiProvider<B>
where
    B: BootServices,
{
    pub const fn new_uninit() -> Self {
        let system_tables = SystemTables::new();
        Self {
            version: AtomicU32::new(0),
            signature: signature::STAE,
            should_reclaim_memory: AtomicBool::new(false),
            system_tables: Mutex::new(system_tables),
            acpi_tables: RwLock::new(vec![]),
            next_table_key: AtomicUsize::new(1),
            notify_list: RwLock::new(vec![]),
            boot_services: OnceCell::new(),
            memory_manager: OnceCell::new(),
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

    pub fn version(&self) -> u32 {
        self.version.load(Ordering::Acquire)
    }

    pub fn set_rsdp(&self, rsdp: &mut AcpiRsdp) {
        self.system_tables.lock().rsdp.store(rsdp as *mut AcpiRsdp, Ordering::Release);
    }

    pub fn set_xsdt(&self, xsdt: &mut AcpiXsdt) {
        self.system_tables.lock().xsdt.store(xsdt as *mut AcpiXsdt, Ordering::Release);
    }
}

impl<B> AcpiProvider for StandardAcpiProvider<B>
where
    B: BootServices,
{
    fn install_acpi_table(&self, acpi_table: &dyn AcpiInstallable) -> Result<TableKey, AcpiError> {
        let table_key = if acpi_table.signature() == signature::FACS {
            // FACS table has a unique structure and installation requirements
            self.install_facs(acpi_table.phys_addr(), acpi_table.length())?
        } else {
            // All other tables must follow the generic ACPI table format
            let acpi_table_generic = acpi_table.downcast_ref::<AcpiTable>().ok_or(AcpiError::InvalidTableFormat)?;
            self.add_table_to_list(acpi_table_generic, false)?
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

    fn get_acpi_table(&self, index: usize) -> Result<&AcpiTable, AcpiError> {
        let acpi_tables = self.acpi_tables.read();
        let table = acpi_tables.get(index).ok_or(AcpiError::InvalidTableIndex)?;
        Ok(unsafe { table.as_ref() })
    }

    fn register_notify(&self, should_register: bool, notify_fn: AcpiNotifyFn) -> Result<(), AcpiError> {
        if should_register {
            self.notify_list.write().push(notify_fn);
        } else {
            let found_pos = self.notify_list.read().iter().position(|x| *x == notify_fn);
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
        let snapshot: Vec<&AcpiTable> = guard.iter().map(|ptr| unsafe { ptr.as_ref() }).collect();
        drop(guard);
        Box::new(snapshot.into_iter())
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
            let entry_ptr = (xsdt_start_ptr as *const u8).add(offset) as *const u64;
            core::ptr::read_unaligned(entry_ptr)
        };

        Ok(entry_addr)
    }

    pub fn install_tables_from_hob(&self, acpi_hob: Hob<AcpiMemoryHob>) -> Result<(), AcpiError> {
        let acpi_table_addr = acpi_hob.rsdp_address;
        let rsdp = unsafe { &*(acpi_table_addr as *const AcpiRsdp) };
        let xsdt_ptr = rsdp.xsdt_address as *const AcpiXsdt;
        let xsdt = unsafe { &*(xsdt_ptr) };

        if xsdt.length < ACPI_HEADER_LEN as u32 {
            return Err(AcpiError::XsdtNotInitialized);
        }

        let entries = (xsdt.length as usize - ACPI_HEADER_LEN) / mem::size_of::<u64>();
        for i in 0..entries {
            // Find the address value of the next XSDT entry
            let entry_addr = Self::get_xsdt_entry(i, xsdt_ptr as *const u8, xsdt.length as usize)?;

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
                // SAFETY: we assume the FADT set up in the HOB points to a valid FACS if the pointer is non-null
                if fadt.x_firmware_ctrl != 0 {
                    let facs_ptr = fadt.x_firmware_ctrl as usize as *const AcpiFacs;
                    let facs_len = unsafe { (*facs_ptr).length };
                    self.install_facs(Some(fadt.x_firmware_ctrl as usize), facs_len)?;
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
                self.add_table_to_list(table, true)?;
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
    pub(crate) fn install_facs(&self, facs_addr: Option<usize>, facs_len: u32) -> Result<TableKey, AcpiError> {
        // FACS is a special case -- it must reside in firmware memory to be installed
        if facs_addr.is_none() {
            return Err(AcpiError::HobTableNotInstalled);
        }

        // Point the `x_firmware_ctrl` field of the FADT to the FACS
        {
            let system_tables = self.system_tables.lock();
            let facs_ptr = facs_addr.unwrap() as u64;
            system_tables.facs.store(facs_ptr as *mut AcpiFacs, Ordering::Release);
            if let Some(fadt) = system_tables.fadt_mut() {
                fadt.x_firmware_ctrl = facs_ptr;
            }
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
        npages: usize,
    ) -> Result<usize, AcpiError> {
        // If the table is from the HOB, it should already be installed in ACPI memory
        if is_from_hob {
            if table_addr.is_none() {
                return Err(AcpiError::HobTableNotInstalled);
            }
        }

        let table_addr = table_addr.expect("Table should be installed in ACPI memory.");

        let mut memory_type = self.memory_type();

        // FACS and UEFI table needs to be aligned to 64B
        if signature == signature::FACS || signature == signature::UEFI {
            if UEFI_PAGE_SIZE % 64 != 0 {
                return Err(AcpiError::FacsUefiNot64BAligned);
            }

            // For FACS and UEFI only, reallocation isn't necessary if passed in from the HOB
            // since they are pre-aligned and allocated in NVS memory
            if is_from_hob {
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
            .allocate_zero_pages(npages, alloc_options)
            .map_err(|_e| AcpiError::AllocationFailed)?;

        Ok(page_alloc.into_raw_ptr::<u8>() as usize)
    }

    /// Adds the FADT to the list of installed tables
    /// `physical_addr`: the address in ACPI memory to install the FADT
    /// `table`: contains data for the FADT
    fn add_fadt_to_list(&self, physical_addr: usize, table: &AcpiTable) -> Result<(), AcpiError> {
        if !(self.system_tables.lock().fadt.load(Ordering::Acquire).is_null()) {
            // FADT already installed, abort
            unsafe {
                self.memory_manager
                    .get()
                    .expect("Memory manager not initialized")
                    .free_pages(physical_addr, uefi_size_to_pages!(table.length as usize))
                    .map_err(|_e| AcpiError::FreeFailed)?
            };
            return Err(AcpiError::FadtAlreadyInstalled);
        }

        let system_tables = self.system_tables.lock();

        system_tables.fadt.store(physical_addr as *mut AcpiFadt, Ordering::Release);

        let facs_ptr = system_tables.facs.load(Ordering::Acquire) as u64;
        let dsdt_ptr = system_tables.dsdt.load(Ordering::Acquire) as u64;

        if let Some(fadt) = system_tables.fadt_mut() {
            fadt.x_firmware_ctrl = facs_ptr;
            fadt.x_dsdt = dsdt_ptr;
        }

        if let Some(rsdp) = system_tables.rsdp_mut() {
            rsdp.oem_id = table.oem_id;
        }

        // XSDT derives OEM information from FADT, but the FADT does NOT get added to the XSDT entries
        let xsdt_ptr = system_tables.xsdt_mut();
        if let Some(xsdt) = xsdt_ptr {
            xsdt.oem_id = table.oem_id;
            xsdt.oem_table_id = table.oem_table_id;
            xsdt.oem_revision = table.oem_revision;
        }

        Ok(())
    }

    fn add_dsdt_to_list(&self, physical_addr: usize) {
        let system_tables = self.system_tables.lock();

        let dsdt_ptr = physical_addr as u64;
        if let Some(fadt) = system_tables.fadt_mut() {
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
        dst_table.table_key = next_table_key;
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
        self.checksum_common_tables()?;

        if add_to_xsdt {
            self.add_entry_to_xsdt(physical_addr as u64, table);
        }

        Ok(self.next_table_key.load(Ordering::Acquire) as TableKey)
    }

    pub(crate) fn memory_type(&self) -> EfiMemoryType {
        if self.should_reclaim_memory.load(Ordering::Acquire) {
            EfiMemoryType::ACPIReclaimMemory
        } else {
            EfiMemoryType::ACPIMemoryNVS
        }
    }

    fn add_entry_to_xsdt(&self, new_table_addr: u64, table: &AcpiTable) -> Result<(), AcpiError> {
        let system_tables = self.system_tables.lock();
        let xsdt_ptr = system_tables.xsdt_mut();

        if let Some(xsdt) = xsdt_ptr {
            // We need to reallocate the XSDT if the buffer exceeds the maximum allocated size
            if xsdt.entries.len() == xsdt.max_entries {
                self.reallocate_xsdt(xsdt.entries.len(), xsdt.length as usize)?;
            }

            // Calculate the memory location for the new entry (end of XSDT)
            let entry_offset = ACPI_HEADER_LEN + (xsdt.entries.len() - 1) * core::mem::size_of::<u64>();
            let dst = unsafe { system_tables.xsdt.load(Ordering::Acquire).add(entry_offset) as *mut u64 };
            // Write entry to ACPI memory
            unsafe {
                core::ptr::write(dst, new_table_addr);
            }

            // Fix up XSDT struct fields
            xsdt.oem_id = table.oem_id;
            xsdt.oem_table_id = table.oem_table_id;
            xsdt.oem_revision = table.oem_revision;
            xsdt.entries.push(new_table_addr as u64);
            xsdt.length += mem::size_of::<u64>() as u32;
        }

        Ok(())
    }

    fn reallocate_xsdt(&self, curr_entries: usize, curr_size: usize) -> Result<(), AcpiError> {
        // Geometrically resize the number of entries in the XSDT
        let new_size = curr_entries * 2 * mem::size_of::<u64>() + ACPI_HEADER_LEN;

        let alloc_options =
            AllocationOptions::new().with_memory_type(self.memory_type()).with_strategy(PageAllocationStrategy::Any);
        let page_alloc = self
            .memory_manager
            .get()
            .expect("Memory manager not initialized")
            .allocate_zero_pages(uefi_size_to_pages!(new_size), alloc_options)
            .map_err(|_e| AcpiError::AllocationFailed)?;
        let physical_addr = page_alloc.into_raw_ptr::<u8>();

        let system_tables = self.system_tables.lock();
        let old_addr = system_tables.xsdt.load(Ordering::Acquire) as usize;

        // Update RSDP with new XSDT address
        if let Some(rsdp) = system_tables.rsdp_mut() {
            rsdp.xsdt_address = physical_addr as u64;
        }

        // Copy over old data to the new XSDT address
        unsafe { copy_nonoverlapping(old_addr as *const u8, physical_addr, curr_size) };
        system_tables.xsdt.store(physical_addr as *mut AcpiXsdt, Ordering::Release);

        // Free the old XSDT
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
                self.system_tables.lock().fadt.store(core::ptr::null_mut(), Ordering::Release);
            }
            signature::FACS => {
                let system_tables = self.system_tables.lock();
                system_tables.facs.store(core::ptr::null_mut(), Ordering::Release);
                // Update other tables pointing to the FACS
                if let Some(fadt) = system_tables.fadt_mut() {
                    fadt.x_firmware_ctrl = 0;
                    Self::acpi_table_update_checksum(
                        fadt as *mut AcpiFadt as *mut u8,
                        fadt.length as usize,
                        memoffset::offset_of!(AcpiTable, checksum),
                    );
                }
            }
            signature::DSDT => {
                let system_tables = self.system_tables.lock();
                system_tables.dsdt.store(core::ptr::null_mut(), Ordering::Release);

                if let Some(fadt) = system_tables.fadt_mut() {
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
        if self.system_tables.lock().xsdt.load(Ordering::Acquire).is_null() {
            return;
        }

        let table_address = table as *const AcpiTable as usize;
        let system_tables = self.system_tables.lock();
        if let Some(xsdt) = system_tables.xsdt_mut() {
            if let Some(index) = xsdt.entries.iter().position(|&entry| entry == (table_address as u64)) {
                xsdt.entries.remove(index);
            }

            Self::acpi_table_update_checksum(
                xsdt as *mut AcpiXsdt as *mut u8,
                xsdt.length as usize,
                memoffset::offset_of!(AcpiTable, checksum),
            );

            xsdt.length -= mem::size_of::<u64>() as u32;
        }
    }

    // Sum of all ACPI bytes (should be zero)
    fn acpi_table_update_checksum(table_ptr: *mut u8, table_length: usize, offset: usize) {
        let table_bytes = unsafe { core::slice::from_raw_parts_mut(table_ptr, table_length) };
        table_bytes[offset] = 0;

        let total_without_checksum: u32 = table_bytes.iter().map(|&b| b as u32).sum();
        let new_checksum = (!total_without_checksum as u8).wrapping_add(1); // complement + 1 = negative
        unsafe { ptr::write(table_ptr.add(offset), new_checksum) };
    }

    // checksum RSDP and XSDT
    pub(crate) fn checksum_common_tables(&self) -> Result<(), AcpiError> {
        let system_tables = self.system_tables.lock();

        if let Some(rsdp) = system_tables.rsdp_mut() {
            Self::acpi_table_update_checksum(
                rsdp as *mut AcpiRsdp as *mut u8,
                rsdp.length as usize,
                memoffset::offset_of!(AcpiRsdp, checksum),
            );
        }

        if let Some(xsdt) = system_tables.xsdt_mut() {
            Self::acpi_table_update_checksum(
                xsdt as *mut AcpiXsdt as *mut u8,
                xsdt.length as usize,
                memoffset::offset_of!(AcpiTable, checksum),
            );
        }

        Ok(())
    }

    fn publish_tables(&self) -> Result<(), AcpiError> {
        let system_tables = self.system_tables.lock();
        let rsdp_ptr = system_tables.rsdp.load(Ordering::Acquire);
        // SAFETY: pointer needs to point to a value rsdp
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
        if let Some(index) =
            self.acpi_tables.read().iter().position(|&table| unsafe { table.as_ref().table_key } == table_key)
        {
            let table = unsafe { self.acpi_tables.read()[index].as_ref() };
            for notify_fn in self.notify_list.read().iter() {
                (*notify_fn)(table, self.version.load(Ordering::Acquire), table_key)?;
            }
        } else {
            return Err(AcpiError::AllocationFailed);
        }

        Ok(())
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct AcpiVersion: u8 {
        const ACPI_NONE = (1 << 0);
        const ACPI_1_0B = (1 << 1);
        const ACPI_2_0  = (1 << 2);
        const ACPI_3_0  = (1 << 3);
        const ACPI_4_0  = (1 << 4);
        const ACPI_5_0  = (1 << 5);
    }
}

impl AcpiVersion {
    pub fn is_gte_2_0(self) -> bool {
        self.intersects(Self::ACPI_2_0 | Self::ACPI_3_0 | Self::ACPI_4_0 | Self::ACPI_5_0)
    }
}

// Trait mostly for mocking helper methods for testing
#[cfg_attr(test, mockall::automock)]
pub trait AcpiHelper {
    fn install_tables_from_hob<'h>(&self, acpi_hob: Hob<'h, AcpiMemoryHob>) -> Result<(), AcpiError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockall::{mock, predicate::*};
    use std::boxed::Box;
    use std::ptr;
    use uefi_sdk::boot_services::MockBootServices;
    use uefi_sdk::component::service::memory::MockMemoryManager;

    unsafe impl Send for MockMemoryManager {}
    unsafe impl Sync for MockMemoryManager {}

    #[test]
    fn test_get_table() {
        let provider = StandardAcpiProvider::new_uninit();
        provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(MockMemoryManager::new())));

        // Dummy AcpiTable (on heap, not actually in ACPI memory)
        let mut table = Box::new(AcpiTable { signature: 0x1111, length: 123, ..Default::default() });
        let table_ptr = NonNull::new(Box::into_raw(table)).unwrap();

        provider.acpi_tables.write().push(table_ptr);

        // Call get_acpi_table(0) (should succeed)
        let fetched: &AcpiTable = provider.get_acpi_table(0).expect("table 0 should exist");
        assert_eq!(fetched.signature, 0x1111);
        assert_eq!(fetched.length, 123);

        // Call with an invalid index (should return InvalidTableIndex)
        let err = provider.get_acpi_table(1).unwrap_err();
        assert!(matches!(err, AcpiError::InvalidTableIndex));

        // Clean up heap
        unsafe {
            Box::from_raw(table_ptr.as_ptr());
        }
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

        // Drop heap memory
        unsafe {
            Box::from_raw(ptr1.as_ptr());
            Box::from_raw(ptr2.as_ptr());
        }
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

        // FACS address not initialized
        let err = provider.install_facs(None, 123).unwrap_err();
        assert!(matches!(err, AcpiError::HobTableNotInstalled));

        // Dummy FACS and FADT
        let facs = Box::new(AcpiFacs { signature: signature::FACS, length: 64, ..Default::default() });
        let facs_ptr = Box::into_raw(facs) as usize;
        let fadt =
            Box::new(AcpiFadt { signature: signature::FACP, length: 244, x_firmware_ctrl: 0, ..Default::default() });
        let fadt_ptr = Box::into_raw(fadt);
        {
            let mut sys = provider.system_tables.lock();
            sys.fadt.store(fadt_ptr, Ordering::Release);
        }

        // This should not fail, and should return 0 (FACS does not have an associated TableKey)
        let res = provider.install_facs(Some(facs_ptr), 64);
        assert_eq!(res.unwrap(), 0);

        // Make sure FACS was installed into FADT
        unsafe {
            let fadt_ref: &AcpiFadt = &*fadt_ptr;
            assert_eq!(fadt_ref.get_x_firmware_ctrl(), facs_ptr as u64);
        }

        // Drop heap memory
        unsafe {
            Box::from_raw(fadt_ptr);
            Box::from_raw(facs_ptr as *mut AcpiFacs);
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

    // #[test]
    // fn test_allocate_addr() {
    //     let mut mock_memory_manager = MockMemoryManager::new();
    //     // This is just a dummy variable to fill out the PageAllocation
    //     let mock_memory_manager2 = MockMemoryManager::new();
    //     let mock_page_alloc = unsafe { PageAllocation::new(0x12341234, 1, &mock_memory_manager2) }.unwrap();
    //     mock_memory_manager.expect_allocate_zero_pages().returning(|_, _| Ok(mock_page_alloc));
    //     let provider = StandardAcpiProvider::new_uninit();
    //     provider.initialize(2, true, MockBootServices::new(), Service::mock(Box::new(mock_memory_manager)));

    //     let returned = provider
    //         .allocate_table_addr(0, Some(0x1234), false, 1)
    //         .expect("should succeed via our DummyMemManagerSuccess");

    //     // When not from a HOB, `allocate_table_addr` should return the value allocated by the memory manager
    //     assert_eq!(returned, 0x12341234);
    // }
}
