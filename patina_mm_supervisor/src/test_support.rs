//! MM Supervisor Test Support
//!
//! Shared fixtures and helpers for the unit tests across this crate: a silent logger that
//! keeps logging branches reachable under coverage, and builders for the HOB lists, page
//! tables, and allocator state the start-up tests run against.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#![cfg_attr(coverage, coverage(off))]

use crate::comm_buffer::{MM_COMMON_REGION_HOB_GUID, MmCommonRegionHobData};
use crate::hob::pass_down::{MM_SUPV_PASS_DOWN_HOB_GUID, MM_SUPV_PASS_DOWN_HOB_REVISION, MmSupvPassDownHobData};
use crate::memory::SharedPagingAllocator;
use crate::mmcore::init::MP_INFORMATION_HOB_GUID;
use crate::mmcore::mseg::MSEG_SMRAM_HOB_GUID;
use crate::mmcore::smi_idt_patch::{FIXUP64_SMI_HANDLER_IDTR, PerCoreMmiEntryStructHdr};
use crate::{
    MmSupervisorCore, PlatformInfo,
    memory::{
        AllocationType, PageAllocator,
        mmram::MmramRegion,
        page_allocator::{SMM_SMRAM_MEMORY_GUID, SmramDescriptor, SmramReserveHobData},
        page_ownership::PageOwnership,
    },
    state::{InitState, security_state},
};
use core::ffi::c_void;
use core::{alloc::Layout, mem::size_of, ptr::NonNull};
use patina::management_mode::comm_buffer_hob::MM_COMM_BUFFER_HOB_GUID;
use patina::management_mode::supervisor::{MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID};
use patina::pi::guid::HOB_MEMORY_ALLOC_MODULE_GUID;
use patina::{UEFI_PAGE_SIZE, management_mode::comm_buffer_hob::MmCommonBufferHobData};
use patina::{
    management_mode::supervisor::MM_SUPERVISOR_CORE_GUID,
    pi::BootMode,
    pi::hob::{
        END_OF_HOB_LIST, GUID_EXTENSION, GuidHob, HANDOFF, HobHeader, MEMORY_ALLOCATION, MemoryAllocationHeader,
        MemoryAllocationModule, PhaseHandoffInformationTable,
    },
    standard::efi,
};
use patina_internal_cpu::save_state::PROCESSOR_INFO_ENTRY_SIZE;
use patina_paging::MemoryAttributes;
use patina_paging::{PageTable, PagingType, x64::X64PageTable};
use std::{
    alloc::{alloc_zeroed, dealloc, handle_alloc_error},
    panic::{AssertUnwindSafe, catch_unwind},
};

pub(crate) fn owned_as(owner: Option<PageOwnership>) -> impl FnOnce(u64, u64) -> Option<PageOwnership> {
    move |_, _| owner
}
pub(crate) fn overlaps(value: bool) -> impl FnOnce(u64, u64) -> bool {
    move |_, _| value
}
pub(crate) struct TestPlatform;

impl PlatformInfo for TestPlatform {}
pub(crate) struct RawHobList {
    pub(crate) storage: Vec<u64>,
    pub(crate) byte_len: usize,
}
impl RawHobList {
    pub(crate) fn new() -> Self {
        let mut list = Self { storage: Vec::new(), byte_len: 0 };
        list.push_struct(PhaseHandoffInformationTable {
            header: HobHeader {
                r#type: HANDOFF,
                length: size_of::<PhaseHandoffInformationTable>() as u16,
                reserved: 0,
            },
            version: 0x0001_0000,
            boot_mode: BootMode::BootWithFullConfiguration,
            memory_top: 0,
            memory_bottom: 0,
            free_memory_top: 0,
            free_memory_bottom: 0,
            end_of_hob_list: 0,
        });
        list
    }

    pub(crate) fn push_struct<T>(&mut self, value: T) {
        assert!(self.byte_len.is_multiple_of(core::mem::align_of::<T>()));
        let new_len = self.byte_len.checked_add(size_of::<T>()).expect("test HOB list size should fit usize");
        self.storage.resize(new_len.div_ceil(size_of::<u64>()), 0);

        // SAFETY: `storage` is u64-aligned, the offset alignment is asserted above, and
        // resizing reserved at least `size_of::<T>()` writable bytes.
        unsafe {
            core::ptr::write(self.storage.as_mut_ptr().cast::<u8>().add(self.byte_len).cast::<T>(), value);
        }
        self.byte_len = new_len;
    }

    pub(crate) fn push_guid_hob(&mut self, name: patina::BinaryGuid, data: &[u8]) {
        assert!((size_of::<GuidHob>() + data.len()).is_multiple_of(size_of::<u64>()));
        self.push_struct(guid_hob(name, data.len()));

        let new_len = self.byte_len.checked_add(data.len()).expect("test HOB list size should fit usize");
        self.storage.resize(new_len.div_ceil(size_of::<u64>()), 0);
        let destination = self.storage.as_mut_ptr().cast::<u8>();
        // SAFETY: resizing reserved `data.len()` bytes at `byte_len`; source and destination
        // are separate allocations and therefore do not overlap.
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), destination.add(self.byte_len), data.len());
        }
        self.byte_len = new_len;
    }

    pub(crate) fn finish(mut self) -> Self {
        let end_offset = self.byte_len;
        self.push_struct(HobHeader { r#type: END_OF_HOB_LIST, length: size_of::<HobHeader>() as u16, reserved: 0 });
        let end_address = self.as_ptr() as u64 + end_offset as u64;

        // SAFETY: `new` placed a properly aligned PHIT at the start of storage, and no
        // references into the vector are live while its final field is updated.
        unsafe {
            (*self.storage.as_mut_ptr().cast::<PhaseHandoffInformationTable>()).end_of_hob_list = end_address;
        }
        self
    }

    pub(crate) fn as_ptr(&self) -> *const c_void {
        self.storage.as_ptr().cast()
    }

    /// Borrows the list's leading Phase Handoff Information Table.
    ///
    /// The HOB list always begins with the PHIT, which is what the init helpers now take.
    pub(crate) fn handoff(&self) -> &PhaseHandoffInformationTable {
        // SAFETY: `new` pushes a PHIT first, and `storage` outlives the borrow.
        unsafe { &*self.as_ptr().cast::<PhaseHandoffInformationTable>() }
    }
}
pub(crate) struct PageAlignedMemory {
    pub(crate) ptr: NonNull<u8>,
    pub(crate) layout: Layout,
}
impl PageAlignedMemory {
    pub(crate) fn new(pages: usize) -> Self {
        let layout = Layout::from_size_align(pages * UEFI_PAGE_SIZE, UEFI_PAGE_SIZE).expect("page allocation layout");
        // SAFETY: `layout` has non-zero size and valid page alignment.
        let ptr = NonNull::new(unsafe { alloc_zeroed(layout) }).unwrap_or_else(|| handle_alloc_error(layout));
        Self { ptr, layout }
    }

    pub(crate) fn base(&self) -> u64 {
        self.ptr.as_ptr() as u64
    }

    pub(crate) fn size(&self) -> u64 {
        self.layout.size() as u64
    }
}
impl Drop for PageAlignedMemory {
    fn drop(&mut self) {
        // SAFETY: `ptr` was allocated with this exact layout and has not been deallocated.
        unsafe { dealloc(self.ptr.as_ptr(), self.layout) };
    }
}

pub(crate) fn pass_down_hob_data(pass_down: &MmSupvPassDownHobData) -> [u8; size_of::<MmSupvPassDownHobData>()] {
    let mut data = [0_u8; size_of::<MmSupvPassDownHobData>()];
    data[0..4].copy_from_slice(&pass_down.revision.to_ne_bytes());
    data[4..8].copy_from_slice(&pass_down.reserved.to_ne_bytes());

    let fields = [
        pass_down.cpl3_stack_base,
        pass_down.cpl3_stack_size,
        pass_down.sm_base,
        pass_down.mm_initialized_buffer,
        pass_down.firmware_policy_buffer,
        pass_down.firmware_policy_buffer_size,
        pass_down.mmi_entry_size,
    ];
    for (index, field) in fields.into_iter().enumerate() {
        let offset = 8 + index * size_of::<u64>();
        data[offset..offset + size_of::<u64>()].copy_from_slice(&field.to_ne_bytes());
    }

    data
}
pub(crate) fn valid_pass_down_hob() -> MmSupvPassDownHobData {
    MmSupvPassDownHobData {
        revision: MM_SUPV_PASS_DOWN_HOB_REVISION,
        reserved: 0,
        cpl3_stack_base: 0x10_0000,
        cpl3_stack_size: 0x4000,
        sm_base: 0x20_0000,
        mm_initialized_buffer: 0x30_0000,
        firmware_policy_buffer: 0x40_0000,
        firmware_policy_buffer_size: 0x2000,
        mmi_entry_size: 0x100,
    }
}
pub(crate) fn supv_comm_buffer_hob_data(
    address: u64,
    pages: u64,
    status_address: u64,
) -> [u8; size_of::<MmCommonRegionHobData>()] {
    let mut data = [0_u8; size_of::<MmCommonRegionHobData>()];
    data[8..16].copy_from_slice(&address.to_ne_bytes());
    data[16..24].copy_from_slice(&pages.to_ne_bytes());
    data[24..32].copy_from_slice(&status_address.to_ne_bytes());
    data
}
pub(crate) fn user_comm_buffer_hob_data(
    address: u64,
    pages: u64,
    status_address: u64,
) -> [u8; size_of::<MmCommonBufferHobData>()] {
    let mut data = [0_u8; size_of::<MmCommonBufferHobData>()];
    data[0..8].copy_from_slice(&address.to_ne_bytes());
    data[8..16].copy_from_slice(&pages.to_ne_bytes());
    data[16..24].copy_from_slice(&status_address.to_ne_bytes());
    data
}
pub(crate) fn allocation_module(
    allocation_name: patina::BinaryGuid,
    module_name: patina::BinaryGuid,
    entry_point: u64,
) -> MemoryAllocationModule {
    MemoryAllocationModule {
        header: HobHeader {
            r#type: MEMORY_ALLOCATION,
            length: size_of::<MemoryAllocationModule>() as u16,
            reserved: 0,
        },
        alloc_descriptor: MemoryAllocationHeader {
            name: allocation_name,
            memory_base_address: 0x10_0000,
            memory_length: 0x20_000,
            memory_type: efi::BOOT_SERVICES_CODE,
            reserved: [0; 4],
        },
        module_name,
        entry_point,
    }
}
pub(crate) fn guid_hob(name: patina::BinaryGuid, data_len: usize) -> GuidHob {
    GuidHob {
        header: HobHeader { r#type: GUID_EXTENSION, length: (size_of::<GuidHob>() + data_len) as u16, reserved: 0 },
        name,
    }
}
pub(crate) fn mp_information_hob_data(number_of_cpus: usize) -> Vec<u8> {
    let mut data = vec![0_u8; 16 + number_of_cpus * PROCESSOR_INFO_ENTRY_SIZE];
    data[0..8].copy_from_slice(&(number_of_cpus as u64).to_le_bytes());
    data
}
pub(crate) fn policy_hob_list_with_cpu_count(number_of_cpus: usize, include_mseg: bool) -> RawHobList {
    let mut list = RawHobList::new();
    list.push_guid_hob(MP_INFORMATION_HOB_GUID, &mp_information_hob_data(number_of_cpus));
    list.push_guid_hob(MM_SUPV_PASS_DOWN_HOB_GUID, &pass_down_hob_data(&valid_pass_down_hob()));
    if include_mseg {
        list.push_guid_hob(MSEG_SMRAM_HOB_GUID, &mseg_smram_hob_data(0x0040_0000, 0x0040_0000, 0x0002_0000));
    }
    list.push_guid_hob(MM_COMMON_REGION_HOB_GUID, &supv_comm_buffer_hob_data(0x10_0000, 2, 0x20_0000));
    list.push_guid_hob(MM_COMM_BUFFER_HOB_GUID, &user_comm_buffer_hob_data(0x30_0000, 3, 0x40_0000));
    list.finish()
}

pub(crate) fn smram_hob_list(memory: &PageAlignedMemory) -> RawHobList {
    let mut data = vec![0_u8; size_of::<SmramReserveHobData>() + size_of::<SmramDescriptor>()];
    data[0..4].copy_from_slice(&1_u32.to_ne_bytes());
    data[8..16].copy_from_slice(&memory.base().to_ne_bytes());
    data[16..24].copy_from_slice(&memory.base().to_ne_bytes());
    data[24..32].copy_from_slice(&memory.size().to_ne_bytes());
    data[32..40].copy_from_slice(&0_u64.to_ne_bytes());

    let mut list = RawHobList::new();
    list.push_guid_hob(SMM_SMRAM_MEMORY_GUID, &data);
    list.finish()
}
pub(crate) fn mmi_entry(fixup64_count: u8, idt_descriptor_address: u64) -> Vec<u8> {
    const PREFIX_SIZE: usize = 8;

    let header_size = size_of::<PerCoreMmiEntryStructHdr>();
    let fixup64_size = usize::from(fixup64_count) * size_of::<u64>();
    let fixup_structure_size = header_size + fixup64_size;
    let mut entry = vec![0_u8; PREFIX_SIZE + fixup_structure_size + size_of::<u32>()];
    let header_start = PREFIX_SIZE;

    entry[header_start..header_start + 4].copy_from_slice(&4_u32.to_ne_bytes());
    entry[header_start + 6] = header_size as u8;
    entry[header_start + 7] = fixup64_count;

    if usize::from(fixup64_count) > FIXUP64_SMI_HANDLER_IDTR {
        let idt_fixup_start = header_start + header_size + FIXUP64_SMI_HANDLER_IDTR * size_of::<u64>();
        entry[idt_fixup_start..idt_fixup_start + size_of::<u64>()]
            .copy_from_slice(&idt_descriptor_address.to_ne_bytes());
    }

    let trailer_start = entry.len() - size_of::<u32>();
    entry[trailer_start..].copy_from_slice(&(fixup_structure_size as u32).to_ne_bytes());
    entry
}

pub(crate) fn scan_regions(hob_list: &RawHobList) -> Vec<MmramRegion> {
    // SAFETY: `hob_list` is a valid contiguous HOB list, so it begins with a Phase Handoff
    // Information Table that stays live for the borrow.
    let handoff = unsafe { &*hob_list.as_ptr().cast::<PhaseHandoffInformationTable>() };
    // SAFETY: `handoff` heads a valid contiguous HOB list.
    let (regions, count) = unsafe { PageAllocator::scan_hob_list(handoff) }.expect("scan SMRAM regions");
    regions[..count].to_vec()
}
pub(crate) fn padded_hob_list(extra: usize) -> RawHobList {
    let mut list = RawHobList::new();
    list.push_guid_hob(MM_SUPERVISOR_CORE_GUID, &vec![0xcd_u8; extra]);
    list.finish()
}
pub(crate) fn init_global_state_over(memory: &PageAlignedMemory) {
    let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
    let hob_list = smram_hob_list(memory);
    let scanned = scan_regions(&hob_list);

    // SAFETY: the scanned descriptor references the live, exclusively owned `memory`.
    unsafe {
        supervisor
            .init_page_allocators(&scanned, security_state().page_allocator(), security_state().paging_allocator())
            .expect("the scanned region supports both allocators");
    }

    let root = security_state().page_allocator().allocate_pages(1).expect("page table root page");
    // SAFETY: `root` is a live, page-aligned allocation nothing else references yet.
    unsafe { core::ptr::write_bytes(root as *mut u8, 0, UEFI_PAGE_SIZE) };
    let allocator = SharedPagingAllocator::new(security_state().paging_allocator());
    // SAFETY: `root` is a zeroed, page-aligned table that stays allocated for the whole test.
    let page_table = unsafe { X64PageTable::from_existing(root, allocator, PagingType::Paging4Level) }
        .expect("page table rooted at the test page");
    *security_state().lock_page_table() = Some(page_table);
}
pub(crate) fn map_supervisor_only(memory: &PageAlignedMemory) {
    let attrs = MemoryAttributes::ExecuteProtect | MemoryAttributes::Supervisor;
    let mut pt_guard = security_state().lock_page_table();
    let pt = pt_guard.as_mut().expect("a page table is installed");
    pt.map_memory_region(memory.base(), memory.size(), attrs).expect("map the external buffer");
}
/// An MM Init module allocated and mapped the way `free_init_module` expects to find it:
/// three pages whose first and last are supervisor read-only executable and whose middle
/// page is non-executable, described by a HOB list alongside the Core module.
///
/// Tests perturb that mapping, by un-mapping a page, clearing its protection, or dropping
/// the page table, and assert the free is refused and nothing is released.
pub(crate) struct MappedInitModule {
    pub(crate) supervisor: MmSupervisorCore<TestPlatform, 4>,
    pub(crate) state: InitState,
    pub(crate) init_module: MemoryAllocationModule,
    pub(crate) core_module: MemoryAllocationModule,
}
impl MappedInitModule {
    pub(crate) fn new() -> Self {
        // These allocations back global state for the lifetime of this nextest process.
        let memory = Box::leak(Box::new(PageAlignedMemory::new(16)));
        let paging_memory = Box::leak(Box::new(PageAlignedMemory::new(16)));
        let smram_hobs = smram_hob_list(memory);
        let allocator = security_state().page_allocator();
        let paging_allocator = security_state().paging_allocator();
        // SAFETY: both pools are distinct, page-aligned, writable and remain live.
        unsafe {
            allocator.init_from_regions(&scan_regions(&smram_hobs)).unwrap();
            paging_allocator.init(paging_memory.base(), 16).unwrap();
        }
        let core_base = allocator.allocate_pages(1).unwrap();
        let init_base = allocator.allocate_pages(3).unwrap();
        let mut page_table =
            X64PageTable::new(SharedPagingAllocator::new(paging_allocator), PagingType::Paging4Level).unwrap();
        let code = MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly;
        page_table.map_memory_region(core_base, UEFI_PAGE_SIZE as u64, code).unwrap();
        page_table.map_memory_region(init_base, 3 * UEFI_PAGE_SIZE as u64, code).unwrap();
        page_table
            .map_memory_region(
                init_base + UEFI_PAGE_SIZE as u64,
                UEFI_PAGE_SIZE as u64,
                MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect,
            )
            .unwrap();
        *security_state().lock_page_table() = Some(page_table);

        let mut init_module = allocation_module(HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, init_base);
        init_module.alloc_descriptor.memory_base_address = init_base;
        init_module.alloc_descriptor.memory_length = 3 * UEFI_PAGE_SIZE as u64;
        let mut core_module =
            allocation_module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_CORE_GUID, core_base);
        core_module.alloc_descriptor.memory_base_address = core_base;
        core_module.alloc_descriptor.memory_length = UEFI_PAGE_SIZE as u64;

        Self { supervisor: MmSupervisorCore::new(), state: InitState::new(), init_module, core_module }
    }

    pub(crate) fn hob_list(&self) -> RawHobList {
        let mut hobs = RawHobList::new();
        hobs.push_struct(self.core_module);
        hobs.push_struct(self.init_module);
        hobs.finish()
    }

    pub(crate) fn free(&self) {
        let hobs = self.hob_list();
        self.supervisor
            .discover_and_store_init_region(hobs.handoff(), &self.state)
            .expect("the synthetic HOB list describes an Init module");
        drop(hobs);
        self.supervisor.free_init_module(&self.state);
    }

    pub(crate) fn assert_rejected(&self, expected: &str) {
        let allocator = security_state().page_allocator();
        let free_pages = allocator.free_page_count();
        let supervisor_pages = allocator.allocated_page_count(AllocationType::Supervisor);
        let user_pages = allocator.allocated_page_count(AllocationType::User);
        let panic = catch_unwind(AssertUnwindSafe(|| self.free())).expect_err("invalid image must be rejected");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("panic must report the validation failure");
        assert!(message.contains(expected), "expected {expected:?}, got {message:?}");
        assert!(!self.state.is_init_module_freed());
        assert_eq!(allocator.free_page_count(), free_pages);
        assert_eq!(allocator.allocated_page_count(AllocationType::Supervisor), supervisor_pages);
        assert_eq!(allocator.allocated_page_count(AllocationType::User), user_pages);
    }
}
pub(crate) fn mseg_smram_hob_data(
    physical_start: u64,
    cpu_start: u64,
    physical_size: u64,
) -> [u8; core::mem::size_of::<SmramDescriptor>()] {
    let mut data = [0; core::mem::size_of::<SmramDescriptor>()];
    data[0..8].copy_from_slice(&physical_start.to_ne_bytes());
    data[8..16].copy_from_slice(&cpu_start.to_ne_bytes());
    data[16..24].copy_from_slice(&physical_size.to_ne_bytes());
    data
}

/// Installs a silent logger for the current test process.
///
/// The logger reports every record as enabled and then discards it. It produces no output,
/// but it causes `log::log_enabled!()` to return `true`, so the formatting and dispatch
/// branch inside each `log::*!` macro still runs during tests and is counted by coverage
/// instrumentation. Without it, a reporting path can execute while the message it builds
/// stays unreached.
///
/// Calling this more than once, from any number of tests, is safe.
pub(crate) fn init_test_logger() {
    use std::sync::OnceLock;
    static INIT: OnceLock<()> = OnceLock::new();

    /// Logger that reports every record as enabled but discards them. Used in tests so
    /// `log::log_enabled!()` returns `true` without producing any output.
    struct AlwaysEnabledSilentLogger;

    impl log::Log for AlwaysEnabledSilentLogger {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, _record: &log::Record<'_>) {}
        fn flush(&self) {}
    }

    static SILENT_LOGGER: AlwaysEnabledSilentLogger = AlwaysEnabledSilentLogger;

    INIT.get_or_init(|| {
        let _ = log::set_logger(&SILENT_LOGGER);
        log::set_max_level(log::LevelFilter::Trace);

        // Exercise the logger once so a logger that failed to install shows up here rather
        // than as messages quietly missing from every test that relies on one.
        assert!(log::log_enabled!(log::Level::Trace), "the test logger reports every level as enabled");
        log::trace!("test logger installed");
        log::logger().flush();
    });
}
