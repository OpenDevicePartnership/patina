//! `MmSupervisorCore` Initialization Phase
//!
//! The one-time setup every core runs on its first entry into MM. The BSP
//! also performs the system-wide work in
//! [`bsp_init`](MmSupervisorCore::bsp_init): validating the incoming HOB list,
//! programming the SMRRs, committing the allocators and page table, and
//! publishing a read-only copy of the HOB list for the demoted user core.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{ffi::c_void, sync::atomic::AtomicU8};

use patina::{
    MemoryRange, UEFI_PAGE_SIZE,
    management_mode::{
        comm_buffer_hob::MM_COMM_BUFFER_HOB_GUID,
        supervisor::{MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, MM_SUPERVISOR_USER_GUID},
    },
    pi::{
        guid::HOB_MEMORY_ALLOC_MODULE_GUID,
        hob::{self, PhaseHandoffInformationTable, find_guid_hob_data, find_memory_allocation_module},
    },
};
use patina_internal_cpu::{
    interrupts::Interrupts,
    save_state::{PROCESSOR_INFO_ENTRY_SIZE, vendor_name},
};
use patina_paging::{MemoryAttributes, PageTable, PagingType, x64::X64PageTable};

use crate::{
    MmSupervisorCore, PlatformInfo,
    comm_buffer::{CommBufferConfig, CommBufferError, CommChannel, MM_COMMON_REGION_HOB_GUID},
    error::MmSupervisorResult,
    hob::validation::{self, HobValidationError},
    intrinsics::read_cr3,
    memory::{
        self, AllocationType, PAGE_STEP, PageAllocator, SharedPagingAllocator,
        mmram::MmramRegion,
        mmram::is_range_inside_mmram,
        page_ownership::{PageOwnership, query_address_ownership},
    },
    policy::{self, MemDescriptorV1_0, dump_policy, gate::PolicyGate, walk_page_table},
    state::{init_state, security_state},
    user_access_guard::with_user_access,
};

use super::CoreInitError;
use super::mseg::{MSEG_SMRAM_HOB_GUID, parse_mseg_smram_hob};
use super::smi_idt_patch::patch_smi_handler_idt;
use super::smrr::{configure_smm_code_access, smrr_initialize};
use crate::hob::pass_down::{MM_SUPV_PASS_DOWN_HOB_GUID, MmSupvPassDownHobData, PassDownHobError};
use crate::memory::mmram::{establish_mmram_bound, supervisor_image_anchor};
use crate::privilege_mgmt::save_state::{SaveStateInfo, log_save_state_map, validate_save_state_regions};

// GUID for gMpInformationHobGuid (StandaloneMmPkg/Include/Guid/MpInformation.h)
// { 0xba33f15d, 0x4000, 0x45c1, { 0x8e, 0x88, 0xf9, 0x16, 0x92, 0xd4, 0x57, 0xe3 } }
/// GUID for the MP Information HOB, which carries the processor count and the
/// `EFI_PROCESSOR_INFORMATION` array (APIC IDs) used by the save-state read path.
pub(crate) const MP_INFORMATION_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("ba33f15d-4000-45c1-8e88-f91692d457e3");

/// Number of memory policy descriptors one page holds.
///
/// The page-table walk writes `MemDescriptorV1_0` entries into a single page, and the policy gate
/// keeps that same page as its snapshot buffer. Both are bounded by this count. It is named once
/// because the two take it as a number of entries, not a number of bytes, and a page length
/// passed in its place would let the walk write past the page.
const MEMORY_POLICY_DESCRIPTORS_PER_PAGE: usize = UEFI_PAGE_SIZE / core::mem::size_of::<MemDescriptorV1_0>();

const _: () = {
    assert!(
        MEMORY_POLICY_DESCRIPTORS_PER_PAGE * core::mem::size_of::<MemDescriptorV1_0>() <= UEFI_PAGE_SIZE,
        "the memory policy descriptor count must fit the page the page-table walk writes into"
    );
};

/// Checks that one page of the MM Init image carries the protection it was mapped with.
///
/// Called for every page of the image just before it is freed, so a page that was never mapped
/// read-only and supervisor-only is caught while the image is still identifiable. A page that is
/// not executable carries data rather than code and is accepted as is.
pub(crate) fn validate_init_code_page(address: u64, attributes: MemoryAttributes) {
    if attributes.contains(MemoryAttributes::ExecuteProtect) {
        return;
    }
    assert!(
        attributes.contains(MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly)
            && !attributes.contains(MemoryAttributes::ReadProtect),
        "MM Init code page at 0x{address:016x} must be supervisor-only, read-only and executable: {attributes:?}"
    );
}

impl<P: PlatformInfo, const MAX_CPUS: usize> MmSupervisorCore<P, MAX_CPUS> {
    /// BSP-only, one-time initialization.
    ///
    /// Loads the IDT, parses and validates the incoming HOB list, programs the SMRR, initializes
    /// the page and paging allocators and the global page table, discovers the user module entry
    /// point, initializes the security policy, and publishes a copy of the HOB list the demoted
    /// user core can read.
    ///
    /// Returns the address of the read-only HOB list copy to hand the user core.
    pub(crate) fn bsp_init(
        &'static self,
        hob_hand_off_table: &PhaseHandoffInformationTable,
    ) -> MmSupervisorResult<u64> {
        log::info!("BSP performing one-time initialization...");

        // The GDT and TSS loaded on MM entry stay in place, so only the IDT is loaded. That GDT has a
        // 64-bit code segment at 0x38, which the IDT gates use.
        Interrupts::new().initialize_idt(&[]).map_err(CoreInitError::InterruptManagerInit)?;

        // Check the HOB headers before anything walks the list, so no walk below can loop forever
        // or read past the end of the list.
        // SAFETY: `hob_hand_off_table` is the MM IPL's HOB list (the caller asserts it is
        // non-null). The walk reads only the headers it reaches and stops at the first bad one.
        let hob_list_len = unsafe { validation::validate_hob_list_structure(hob_hand_off_table)? };

        // Parse the producer's SMRAM descriptors into stack metadata. Nothing in MMRAM is written
        // until they have been anchored and validated below.
        // SAFETY: the HOB list passed `validate_hob_list_structure` above.
        let scanned = unsafe { PageAllocator::scan_hob_list_for_smram_regions(hob_hand_off_table)? };
        let scanned_regions = scanned.as_slice();

        let smrr_range = establish_mmram_bound(scanned_regions, supervisor_image_anchor())?;

        validation::validate_incoming_hobs_pre_paging_init(hob_hand_off_table, hob_list_len, scanned_regions)?;

        // Program the range before the allocator makes the first write into MMRAM. Enabling it is
        // left to `smrr_enable` on the next SMI entry: finalizing it here makes the range enforcing
        // across the `RSM` back to the non-MM world, which faults that world on this platform.
        smrr_initialize(smrr_range)?;
        init_state().set_smrr_range(smrr_range);

        // SAFETY: the descriptors were anchored and validated above, so the free regions they
        // describe are MMRAM the supervisor owns exclusively.
        unsafe {
            self.init_page_allocators(
                scanned_regions,
                security_state().page_allocator(),
                security_state().paging_allocator(),
            )?;
        }

        self.init_page_table()?;

        // Validate the HOBs that need the page table, now that it exists. The checks that only
        // need the page allocator ran above.
        validation::validate_incoming_hobs_post_paging_init(hob_hand_off_table)?;

        self.discover_and_store_user_entry(hob_hand_off_table, init_state())?;
        self.discover_and_store_init_region(hob_hand_off_table, init_state())?;
        self.init_policy_and_validate(hob_hand_off_table)?;
        let user_hob_list = self.publish_hob_list_to_user(hob_hand_off_table)?;

        log::info!("BSP one-time initialization complete.");
        Ok(user_hob_list)
    }

    /// Commits the validated SMRAM regions to the page allocator and initializes the paging
    /// allocator from a pool reserved out of it.
    ///
    /// These are the first writes into the memory the producer described, so `scanned` must
    /// already have been validated.
    ///
    /// ## Safety
    ///
    /// Every non-pre-allocated region in `scanned` must be valid, exclusively owned MMRAM.
    pub(crate) unsafe fn init_page_allocators(
        &self,
        scanned: &[MmramRegion],
        page_allocator: &memory::PageAllocator,
        paging_allocator: &memory::PagingPoolAllocator,
    ) -> MmSupervisorResult<()> {
        // SAFETY: the caller guarantees that the free regions in `scanned` are valid, exclusively
        // owned MMRAM.
        unsafe {
            page_allocator.init_from_regions(scanned)?;
        }

        // Reserve pages from the page allocator for paging structures. This is
        // done before paging is initialized to avoid a circular dependency.
        let paging_pool_base = page_allocator.allocate_pages(memory::DEFAULT_PAGING_POOL_PAGES)?;

        log::info!(
            "Reserved {} pages at 0x{:016x} for paging structures",
            memory::DEFAULT_PAGING_POOL_PAGES,
            paging_pool_base
        );

        // Initialize the paging allocator with the reserved pool.
        // SAFETY: `paging_pool_base` was just reserved from the page allocator, so it is a
        // page-aligned region of `DEFAULT_PAGING_POOL_PAGES` pages in SMRAM owned exclusively by
        // the paging allocator.
        unsafe {
            paging_allocator.init(paging_pool_base, memory::DEFAULT_PAGING_POOL_PAGES)?;
        }

        Ok(())
    }

    /// Initializes the global page table from the active CR3.
    ///
    /// This allows the supervisor to modify page attributes on newly allocated
    /// pages. Must be called after [`init_page_allocators`](Self::init_page_allocators)
    /// because the page table draws its backing memory from the paging allocator.
    ///
    /// ## Errors
    ///
    /// Returns `PageTableFromCr3Failed` if the hierarchy at CR3 cannot be adopted.
    fn init_page_table(&self) -> MmSupervisorResult<()> {
        let cr3 = read_cr3();
        let paging_alloc = SharedPagingAllocator::new(security_state().paging_allocator());
        // SAFETY: `cr3` is read from the active control register, so it points to
        // the valid page table hierarchy currently in use by this core, and
        // `paging_alloc` owns the pool from which new paging structures are drawn.
        let page_table =
            unsafe { X64PageTable::from_existing(cr3, paging_alloc, PagingType::Paging4Level) }.map_err(|err| {
                log::error!("Failed to create page table from active CR3=0x{cr3:016x}: {err:?}");
                CoreInitError::PageTableFromCr3Failed { cr3 }
            })?;
        *security_state().lock_page_table() = Some(page_table);
        log::info!("Page table initialized from CR3=0x{cr3:016x}");
        Ok(())
    }

    /// Finds the MM Supervisor User module in the HOB list and stores its entry point for
    /// request processing.
    ///
    /// The module is the `EFI_HOB_TYPE_MEMORY_ALLOCATION` HOB whose
    /// `MemoryAllocationHeader.Name` is `gMmSupervisorHobMemoryAllocModuleGuid`
    /// and whose `ModuleName` is `gMmSupervisorUserGuid`. Returns
    /// [`CoreInitError::UserEntryPointMissing`] if there is none.
    fn discover_and_store_user_entry(
        &self,
        hob_hand_off_table: &PhaseHandoffInformationTable,
        state: &crate::state::InitState,
    ) -> MmSupervisorResult<()> {
        // SAFETY: `hob_hand_off_table` is the in-place PHIT of the validated HOB list handed to
        // `bsp_init`, which is terminated by `END_OF_HOB_LIST`.
        let module = unsafe {
            find_memory_allocation_module(
                hob_hand_off_table,
                MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
                MM_SUPERVISOR_USER_GUID,
            )
        };

        if let Some(module) = module {
            let entry = module.entry_point;
            log::info!("Discovered MM User module entry point: 0x{entry:016x}");
            state.set_user_entry_point(entry);
        } else {
            return Err(CoreInitError::UserEntryPointMissing.into());
        }

        Ok(())
    }

    /// Saves the validated MM Init module allocation before the producer's HOBs are reclaimed.
    pub(crate) fn discover_and_store_init_region(
        &self,
        hob_hand_off_table: &PhaseHandoffInformationTable,
        state: &crate::state::InitState,
    ) -> MmSupervisorResult<()> {
        // SAFETY: `hob_hand_off_table` is the in-place PHIT of the validated HOB list handed to
        // `bsp_init`, which is terminated by `END_OF_HOB_LIST`.
        let Some(module) = (unsafe {
            find_memory_allocation_module(hob_hand_off_table, HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID)
        }) else {
            return Err(CoreInitError::InitModuleRegionMissing.into());
        };

        let (base, length) = (module.alloc_descriptor.memory_base_address, module.alloc_descriptor.memory_length);
        let range =
            MemoryRange::new(base, length).map_err(|_| CoreInitError::InitModuleRegionInvalid { base, length })?;
        state.set_init_module_region(range);
        Ok(())
    }

    /// Frees the saved Init allocation on the first runtime SMI, without accessing any HOBs.
    ///
    /// Called only on the BSP after initialization has returned.
    pub(crate) fn free_init_module(&self, state: &crate::state::InitState) {
        if state.is_init_module_freed() {
            return;
        }
        let Some(init_module) = state.init_module_region() else {
            log::warn!("MM Init module region was not discovered during initialization");
            return;
        };
        assert!(is_range_inside_mmram(&init_module), "MM Init module is outside MMRAM");

        // Freeing takes the page table lock again, so this lock is dropped first.
        {
            let page_table = security_state().lock_page_table();
            let page_table = page_table.as_ref().expect("Page table required to validate MM Init module");
            for address in init_module.step_by(PAGE_STEP) {
                let attributes = page_table
                    .query_memory_region(address, UEFI_PAGE_SIZE as u64)
                    .expect("Failed to query MM Init module page");
                validate_init_code_page(address, attributes);
            }
        }

        security_state()
            .page_allocator()
            .free_pages_checked(
                init_module.base(),
                init_module.size() as usize / UEFI_PAGE_SIZE,
                AllocationType::Supervisor,
            )
            .expect("Failed to free MM Init module");
        state.mark_init_module_freed();
        log::info!("Freed MM Init module at {init_module}");
    }

    /// Initializes the services the HOB list describes, then validates the security policy.
    ///
    /// Reads the MP Information HOB for the CPU count, which sizes everything that follows, then
    /// the `PassDown` HOB for the policy gate, the syscall interface and the memory policy. It
    /// then publishes the save-state metadata, records the optional MSEG region, patches every
    /// core's SMI-handler IDT descriptor, assembles the communication buffers, and runs an initial
    /// security check on the policy gate.
    fn init_policy_and_validate(&self, hob_hand_off_table: &PhaseHandoffInformationTable) -> MmSupervisorResult<()> {
        // The CPU count sizes the Ring 3 stack array the PassDown HOB describes, so it is read
        // before anything that depends on it.
        // SAFETY: `hob_hand_off_table` is the in-place PHIT of the validated HOB list handed to
        // `bsp_init`, which is terminated by `END_OF_HOB_LIST`.
        let mp_information = unsafe { find_guid_hob_data(hob_hand_off_table, MP_INFORMATION_HOB_GUID) }
            .ok_or(CoreInitError::MpInformationHobMissing)?;
        let number_of_cpus = self.parse_mp_information_hob(mp_information)?;

        // SAFETY: as above, `hob_hand_off_table` is the in-place PHIT of the validated HOB list.
        let pass_down_data = unsafe { find_guid_hob_data(hob_hand_off_table, MM_SUPV_PASS_DOWN_HOB_GUID) }
            .ok_or(PassDownHobError::Missing)?;
        // SAFETY: `pass_down_data` is a slice into the validated HOB list, so the buffer pointers
        // it carries reference live memory as `init_from_pass_down_hob` requires.
        let (save_state_info, mmi_entry_size) =
            unsafe { self.init_from_pass_down_hob(pass_down_data, number_of_cpus) }?;

        // `init_from_pass_down_hob` has just checked every save-state region it describes.
        self.publish_save_state_metadata(save_state_info);

        self.discover_mseg_base(hob_hand_off_table);

        // The SMI entry blocks were already copied per SMBASE, so every core is patched here,
        // not only the BSP.
        patch_smi_handler_idt(save_state_info.smbase_array(), number_of_cpus, mmi_entry_size)?;

        self.init_comm_buffers(hob_hand_off_table)?;

        let gate = security_state().policy_gate().ok_or(CoreInitError::PolicyGateUnavailable)?;
        policy::validation::security_policy_check(gate)?;

        log::info!("Security policy check passed");
        Ok(())
    }

    /// Publishes a read-only copy of the HOB list for the demoted user core and returns its
    /// address, reclaiming the producer's pages once the copy is in place.
    ///
    /// The producer's HOB list does not end on a page boundary, and mapping is page-granular, so
    /// remapping its backing pages would hand the remainder of its final page to Ring 3 along with
    /// the list. Nothing constrains what the producer put there. Copying into a dedicated
    /// allocation instead means the only bytes Ring 3 can reach are the HOB list itself and the
    /// zeroed tail of its last page.
    fn publish_hob_list_to_user(&self, hob_hand_off_table: &PhaseHandoffInformationTable) -> MmSupervisorResult<u64> {
        // `get_pi_hob_list_size` takes a raw pointer.
        let hob_list: *const c_void = core::ptr::from_ref(hob_hand_off_table).cast();
        let hob_base = hob_list as u64;
        // SAFETY: `hob_list` is the PHIT of the HOB list `bsp_init` validated, which ends in
        // `END_OF_HOB_LIST`.
        let hob_list_size = unsafe { hob::get_pi_hob_list_size(hob_list) } as u64;

        // A zero-length walk means the list has no HOBs at all, which is not a list. The producer
        // named this range, so it is confirmed to be MMRAM before it is read.
        let hob_range = MemoryRange::new(hob_base, hob_list_size)
            .ok()
            .filter(is_range_inside_mmram)
            .ok_or(HobValidationError::HobListOutsideMmram { base: hob_base, size: hob_list_size })?;

        let size =
            usize::try_from(hob_list_size).map_err(|_| CoreInitError::HobListTooLargeToCopy { size: hob_list_size })?;

        let pages = size.div_ceil(UEFI_PAGE_SIZE);
        let copy_base = security_state().page_allocator().allocate_pages_with_type(pages, AllocationType::User)?;
        let copy_size = pages * UEFI_PAGE_SIZE;

        // The destination is user-owned, so SMAP comes down for the supervisor to fill it. The
        // whole allocation is zeroed first because Ring 3 can read the tail of the last page, and
        // pool memory is not zeroed on allocation.
        // SAFETY: `copy_base` is a live allocation of `copy_size` bytes that nothing else
        // references yet, and `hob_base` was checked above to be `size` readable bytes inside
        // MMRAM. The two cannot overlap: the allocation came from the free pool, while the HOB
        // list is memory the MM IPL reserved.
        unsafe {
            with_user_access(|| {
                core::ptr::write_bytes(copy_base as *mut u8, 0, copy_size);
                core::ptr::copy_nonoverlapping(hob_base as *const u8, copy_base as *mut u8, size);
            });
        }

        // The page table guard is a temporary, so the lock is released at the end of this
        // statement, before the reclaim below takes it again. It is not reentrant.
        let attrs = MemoryAttributes::ReadOnly | MemoryAttributes::ExecuteProtect;
        security_state()
            .lock_page_table()
            .as_mut()
            .ok_or(CoreInitError::PageTableUnavailable)?
            .map_memory_region(copy_base, copy_size as u64, attrs)
            .map_err(|err| {
                log::error!("Failed to map the user HOB list copy at 0x{copy_base:016x}: {err:?}");
                CoreInitError::HobListCopyNotMapped { base: copy_base, size: copy_size as u64 }
            })?;

        let copy_end = copy_base + copy_size as u64;
        log::info!(
            "Published HOB list copy of 0x{hob_list_size:x} bytes from 0x{hob_base:016x} at \
             0x{copy_base:016x}-0x{copy_end:016x} as user read-only"
        );

        // Nothing reads the original HOB list after this point, because the user core gets the copy.
        // The original's pages are therefore returned to the page allocator, which zeroes them and
        // makes them inaccessible. This is best effort and may not free the whole list.
        // Freeing works on whole pages, and the allocator does not record where the producer's
        // allocation ends, so only the pages lying wholly inside the list are freed. A partial page
        // at either end may also hold other data, so it stays allocated. At most two pages are
        // left behind, and a list that fills no whole page frees nothing.
        if let Ok(whole_pages) = hob_range.align_inward(UEFI_PAGE_SIZE as u64) {
            let (base, pages) = (whole_pages.base(), whole_pages.pages() as usize);
            match security_state().page_allocator().free_pages_checked(base, pages, AllocationType::Supervisor) {
                Ok(()) => log::info!("Reclaimed {pages} page(s) of the producer's HOB list at {whole_pages}"),
                // Reclaiming is best effort, but a failure means the range is not what the
                // descriptors said it was, so it is logged as an error.
                Err(err) => log::error!("Failed to reclaim the producer's HOB list at {whole_pages}: {err:?}"),
            }
        }

        Ok(copy_base)
    }

    /// Maps the per-CPU Ring 3 stacks as user-accessible, writable, non-executable pages.
    ///
    /// A demoted routine faults on its first push if its stack is supervisor-owned. The range
    /// covers the `num_cpus` stacks the MM IPL provisioned, which is also the range
    /// [`SyscallInterface::get_cpl3_stack`](crate::privilege_mgmt::syscall_setup::SyscallInterface::get_cpl3_stack)
    /// hands out.
    fn map_cpl3_stacks_to_user(&self, base: u64, per_core_size: u64, num_cpus: usize) -> MmSupervisorResult<()> {
        let invalid_region = || CoreInitError::Cpl3StackRegionInvalid { base, per_core_size, num_cpus };

        if base == 0 || per_core_size == 0 {
            return Err(invalid_region().into());
        }

        let total_size = per_core_size.checked_mul(num_cpus as u64).ok_or_else(invalid_region)?;
        let stacks = MemoryRange::new(base, total_size)
            .and_then(|range| range.align_outward(UEFI_PAGE_SIZE as u64))
            .map_err(|_| invalid_region())?;

        // The region is described by the untrusted producer, so nothing outside MMRAM may be
        // handed to Ring 3.
        if !is_range_inside_mmram(&stacks) {
            return Err(CoreInitError::Cpl3StackRegionOutsideMmram { range: stacks }.into());
        }

        // The page table guard is a temporary, so the lock is released at the end of this
        // statement, before the verification below takes it again.
        security_state()
            .lock_page_table()
            .as_mut()
            .ok_or(CoreInitError::PageTableUnavailable)?
            .map_memory_region(stacks.base(), stacks.size(), MemoryAttributes::ExecuteProtect)
            .map_err(|err| {
                log::error!("Failed to map CPL3 stacks {stacks} to user level: {err:?}");
                CoreInitError::Cpl3StacksNotMapped { range: stacks }
            })?;

        validation::verify_cpl3_stacks_user_accessible(&stacks).map_err(|err| {
            log::error!("CPL3 stacks {stacks} are not usable by Ring 3 after remapping: {err}");
            CoreInitError::Cpl3StacksNotMapped { range: stacks }
        })?;

        log::info!("Mapped CPL3 stacks {stacks} as user read/write, NX");
        Ok(())
    }

    /// Programs the per-logical-processor MSRs on every core (BSP and APs) during its first
    /// entry: the MSEG base, the SMRR, and SMM Code Access Check.
    pub(crate) fn per_core_init(&'static self, cpu_id: u32) -> MmSupervisorResult<()> {
        log::trace!("CPU {cpu_id} performing per-core initialization...");

        // IA32_SMM_MONITOR_CTL is per-logical-processor, so every core programs it.
        super::mseg::program_mseg_base(cpu_id);

        // SMRR is per-logical-processor. The BSP already programmed the same range in `bsp_init`.
        let range = init_state().smrr_range().ok_or(CoreInitError::SmrrRangeUnavailable)?;
        smrr_initialize(range)?;
        configure_smm_code_access();

        log::trace!("CPU {cpu_id} per-core initialization complete.");
        Ok(())
    }

    /// Publishes the save-state metadata the read syscall depends on.
    ///
    /// `save_state_info` must have passed [`check_save_state_regions`](Self::check_save_state_regions).
    fn publish_save_state_metadata(&self, save_state_info: SaveStateInfo) {
        security_state().set_save_state_info(save_state_info);
        log_save_state_map(save_state_info);
        log::info!(
            "Save-state metadata initialized for {} CPU(s) from SMBASE array at 0x{:016x} \
             using the {} save-state layout",
            save_state_info.number_of_cpus(),
            save_state_info.smbase_array(),
            vendor_name()
        );
    }

    /// Records the MSEG region reserved for an STM, when the platform published one.
    ///
    /// Each core programs the base into `IA32_SMM_MONITOR_CTL` during per-core init. Platforms
    /// without STM or SEA integration do not publish the HOB, so its absence is not an error.
    fn discover_mseg_base(&self, hob_hand_off_table: &PhaseHandoffInformationTable) {
        // SAFETY: `hob_hand_off_table` is the in-place PHIT of the validated HOB list handed to
        // `bsp_init`, which is terminated by `END_OF_HOB_LIST`.
        match unsafe { find_guid_hob_data(hob_hand_off_table, MSEG_SMRAM_HOB_GUID) }.and_then(parse_mseg_smram_hob) {
            Some(mseg_base) => {
                init_state().set_mseg_base(mseg_base);
                log::info!("MSEG base 0x{mseg_base:x} discovered from MSEG SMRAM HOB");
            }
            None => log::warn!("No usable MSEG SMRAM HOB; IA32_SMM_MONITOR_CTL will not be programmed"),
        }
    }

    /// Builds both communication channels and stores the assembled configuration.
    fn init_comm_buffers(&self, hob_hand_off_table: &PhaseHandoffInformationTable) -> MmSupervisorResult<()> {
        // Supervisor channel, from the `MM_COMMON_REGION_HOB_GUID` HOB.
        // SAFETY: `hob_hand_off_table` is the PHIT of the validated HOB list.
        let supv_region_data = unsafe { find_guid_hob_data(hob_hand_off_table, MM_COMMON_REGION_HOB_GUID) }
            .ok_or(CommBufferError::CommRegionHobMissing)?;
        let supv = CommChannel::from_supv_hob(supv_region_data)
            .inspect_err(|err| log::error!("Failed to initialize supervisor communication buffer: {err}"))?;

        // User channel, from the `MM_COMM_BUFFER_HOB_GUID` HOB. The user core reads this HOB too, so
        // it is redirected to the internal copy.
        // SAFETY: as above.
        let user_buffer_data = unsafe { find_guid_hob_data(hob_hand_off_table, MM_COMM_BUFFER_HOB_GUID) }
            .ok_or(CommBufferError::CommunicationBufferHobMissing)?;
        let user = CommChannel::from_user_hob(user_buffer_data)
            .inspect_err(|err| log::error!("Failed to initialize user communication buffer: {err}"))?;

        // SAFETY: `user_buffer_data` is the writable HOB payload and is not used after this call.
        unsafe { user.redirect_user_hob(user_buffer_data.as_ptr().cast_mut(), user_buffer_data.len()) }
            .inspect_err(|err| log::error!("Failed to redirect the user communication buffer HOB: {err}"))?;

        let supv_to_user_buffer =
            security_state().page_allocator().allocate_pages_with_type(1, AllocationType::User).map_err(|err| {
                log::error!("Failed to allocate page for supervisor-to-user buffer: {err}");
                CommBufferError::AllocationFailed
            })?;

        security_state().set_comm_buffer_config(CommBufferConfig {
            supervisor: supv,
            user,
            supv_to_user_buffer,
            supv_to_user_buffer_size: UEFI_PAGE_SIZE as u64,
        });
        log::info!("Comm buffers: supervisor [{supv}], user [{user}], supv_to_user=0x{supv_to_user_buffer:x}");

        Ok(())
    }

    /// Returns the CPU count from the MP Information HOB.
    ///
    /// The count is validated against `MAX_CPUS` here and returned as a `usize`, so callers
    /// index and size allocations with it directly. A raw `u64` count from the HOB that does not
    /// fit a `usize` is reported in [`CoreInitError::InvalidCpuCount`] as `usize::MAX`.
    fn parse_mp_information_hob(&self, data: &[u8]) -> MmSupervisorResult<usize> {
        /// Offset of `ProcessorInfoBuffer[]` within `MP_INFORMATION_HOB_DATA`.
        const PROCESSOR_INFO_BUFFER_OFFSET: usize = 16;

        if data.len() < PROCESSOR_INFO_BUFFER_OFFSET {
            log::error!("MP Information HOB too small: {} < {}", data.len(), PROCESSOR_INFO_BUFFER_OFFSET);
            return Err(CoreInitError::MpInformationHobMalformed.into());
        }

        // The payload was checked to be at least `PROCESSOR_INFO_BUFFER_OFFSET` bytes above,
        // so reading the leading count cannot fail. The reasons are still reported in case a
        // future change loosens that guard.
        let number_of_cpus = u64::from_le_bytes(
            data.get(0..8)
                .ok_or_else(|| {
                    log::error!("MP Information HOB has no processor count field");
                    CoreInitError::MpInformationHobMalformed
                })?
                .try_into()
                .map_err(|_| {
                    log::error!("MP Information HOB processor count field is not 8 bytes");
                    CoreInitError::MpInformationHobMalformed
                })?,
        );

        // A count that does not fit a `usize` is certainly larger than MAX_CPUS, so it is named
        // as the largest count this target can express and rejected by the bound below rather
        // than needing an error of its own.
        let cpu_count = usize::try_from(number_of_cpus).unwrap_or(usize::MAX);
        if cpu_count == 0 || cpu_count > MAX_CPUS {
            log::error!("MP Information HOB CPU count {cpu_count} is outside the supported range 1..={MAX_CPUS}");
            return Err(CoreInitError::InvalidCpuCount { found: cpu_count, maximum: MAX_CPUS }.into());
        }

        // `cpu_count` is bounded by `MAX_CPUS`, so the offsets below cannot overflow today.
        let processor_info_size = cpu_count.checked_mul(PROCESSOR_INFO_ENTRY_SIZE).ok_or_else(|| {
            log::error!("MP Information HOB: {cpu_count} processor entries overflow the payload size");
            CoreInitError::MpInformationHobMalformed
        })?;

        let processor_info_end = PROCESSOR_INFO_BUFFER_OFFSET.checked_add(processor_info_size).ok_or_else(|| {
            log::error!("MP Information HOB: processor info end offset overflows");
            CoreInitError::MpInformationHobMalformed
        })?;

        data.get(PROCESSOR_INFO_BUFFER_OFFSET..processor_info_end).ok_or_else(|| {
            log::error!(
                "MP Information HOB holds {} bytes but {cpu_count} processor entries need {processor_info_end}",
                data.len()
            );
            CoreInitError::MpInformationHobMalformed
        })?;

        Ok(cpu_count)
    }

    /// Publishes the per-core "MM initialized" slots the MM IPL allocated.
    ///
    /// The buffer is required: every core marks its slot at the end of its first entry, and later
    /// entries read it to skip initialization, so a missing buffer would fail the first entry. A
    /// null buffer is rejected by the MMRAM check below, since MMRAM never covers address 0.
    ///
    /// ## Safety
    ///
    /// `buffer` must name `number_of_cpus` bytes that stay resident for the supervisor's
    /// lifetime.
    unsafe fn store_mm_initialized_slots(&self, buffer: u64, number_of_cpus: usize) -> MmSupervisorResult<()> {
        // The slice built below is `number_of_cpus` elements long, so the bound is a
        // memory-safety precondition and is checked here rather than assumed of the caller.
        if number_of_cpus == 0 || number_of_cpus > MAX_CPUS {
            return Err(CoreInitError::InvalidCpuCount { found: number_of_cpus, maximum: MAX_CPUS }.into());
        }
        if !MemoryRange::new(buffer, number_of_cpus as u64).is_ok_and(|range| is_range_inside_mmram(&range)) {
            log::error!("MM initialized buffer at 0x{buffer:016x} does not contain {number_of_cpus} slot(s) in MMRAM");
            return Err(CoreInitError::InitializedBufferInvalid.into());
        }

        let address = usize::try_from(buffer).map_err(|_| CoreInitError::InitializedBufferInvalid)?;
        let buffer_ptr = core::ptr::with_exposed_provenance::<AtomicU8>(address);
        // SAFETY: the checks above place `number_of_cpus` one-byte slots inside MMRAM at
        // `buffer`, and this function's contract keeps them resident.
        let initialized_slots = unsafe { core::slice::from_raw_parts(buffer_ptr, number_of_cpus) };
        init_state().set_mm_initialized_buffer(initialized_slots);
        log::info!("MM Initialized buffer set to 0x{buffer:016x} with {number_of_cpus} slot(s)");

        Ok(())
    }

    /// Requires every per-CPU save-state region to lie inside MMRAM and be mapped
    /// supervisor-only.
    ///
    /// The save-state syscall reads these regions on Ring 3's behalf, so they are proven once
    /// here rather than trusted at each read.
    fn check_save_state_regions(&self, save_state_info: &SaveStateInfo) -> MmSupervisorResult<()> {
        validate_save_state_regions(save_state_info, is_range_inside_mmram, |range| {
            matches!(query_address_ownership(range), Some(PageOwnership::Supervisor))
        })
        .inspect_err(|err| log::error!("PassDown HOB does not describe usable save-state regions: {err}"))?;
        log::info!(
            "Validated save-state regions for {} CPU(s) from SMBASE array at 0x{:016x}",
            save_state_info.number_of_cpus(),
            save_state_info.smbase_array()
        );

        Ok(())
    }

    /// Builds the policy gate over the firmware policy blob and installs it.
    ///
    /// Returns the page reserved for the memory policy descriptors, which the gate keeps as its
    /// snapshot buffer and [`record_unblocked_memory`](Self::record_unblocked_memory) fills.
    ///
    /// ## Safety
    ///
    /// `buffer` must name `size` readable bytes that stay resident for the supervisor's lifetime.
    unsafe fn install_policy_gate(&self, buffer: u64, size: u64) -> MmSupervisorResult<u64> {
        let memory_policy_buffer = security_state()
            .page_allocator()
            .allocate_pages(1)
            .inspect_err(|err| log::error!("Failed to allocate page for memory policy buffer: {err}"))?;

        let policy_ptr = buffer as *const u8;
        let policy_buffer_size =
            usize::try_from(size).map_err(|_| PassDownHobError::FirmwarePolicyBufferSizeUnsupported { size })?;

        // SAFETY: this function's contract requires `buffer` to be readable for `size` bytes and
        // to stay resident. The HOB's reported size bounds the blob's own internal offsets.
        let mut gate = unsafe { PolicyGate::new(policy_ptr, policy_buffer_size) }
            .inspect_err(|err| log::error!("Failed to create policy gate: {err}"))?;
        log::info!("Policy gate initialized successfully");
        // SAFETY: `policy_ptr` is the same valid, resident firmware policy buffer.
        unsafe { dump_policy(policy_ptr) };

        policy::audit_boundary_grants(&gate);

        gate.set_memory_policy_buffer(
            memory_policy_buffer as *mut MemDescriptorV1_0,
            MEMORY_POLICY_DESCRIPTORS_PER_PAGE,
        )?;
        security_state().set_policy_gate(gate);

        Ok(memory_policy_buffer)
    }

    /// Starts the syscall interface over the Ring 3 stacks the MM IPL provisioned.
    fn start_syscall_interface(
        &self,
        cpl3_stack_base: u64,
        cpl3_stack_size: u64,
        number_of_cpus: usize,
    ) -> MmSupervisorResult<()> {
        // The CPU count bounds `get_cpl3_stack`, so it must be the count the MM IPL sized the
        // stack array for, not the supervisor's `MAX_CPUS` capacity.
        let stack_size = usize::try_from(cpl3_stack_size).map_err(|_| CoreInitError::Cpl3StackRegionInvalid {
            base: cpl3_stack_base,
            per_core_size: cpl3_stack_size,
            num_cpus: number_of_cpus,
        })?;

        self.syscall_interface.init(number_of_cpus, cpl3_stack_base, stack_size)?;
        Ok(())
    }

    /// Records the memory outside MMRAM that the page table currently maps.
    ///
    /// The descriptors the walk generates seed the unblocked memory tracker, which later
    /// `UNBLOCK_MEM` requests are checked against.
    ///
    /// ## Safety
    ///
    /// `memory_policy_buffer` must name a page with room for
    /// [`MEMORY_POLICY_DESCRIPTORS_PER_PAGE`] descriptors that stays resident.
    unsafe fn record_unblocked_memory(&self, memory_policy_buffer: u64) -> MmSupervisorResult<()> {
        let descriptors = memory_policy_buffer as *mut MemDescriptorV1_0;
        let cr3 = read_cr3();

        // SAFETY: `cr3` is read from the active control register, so it points
        // to the live root page table, and this function's contract gives
        // `descriptors` room for `MEMORY_POLICY_DESCRIPTORS_PER_PAGE` entries.
        let descriptor_count =
            unsafe { walk_page_table(cr3, descriptors, MEMORY_POLICY_DESCRIPTORS_PER_PAGE, is_range_inside_mmram) }?;
        log::info!("Generated {descriptor_count} memory policy descriptor(s) from the page table walk");

        // SAFETY: `walk_page_table` succeeded, so the buffer holds `descriptor_count` initialized
        // `MemDescriptorV1_0` entries.
        unsafe { security_state().unblocked_tracker().init_from_buffer(descriptors, descriptor_count) }?;

        security_state().unblocked_tracker().dump_regions();
        Ok(())
    }

    /// Processes the MM Supervisor `PassDown` HOB.
    ///
    /// Runs the setup its payload describes, in order: the per-core initialized slots, the
    /// save-state regions, the policy gate, the syscall interface and the Ring 3 stacks, and the
    /// unblocked memory tracker. The stacks are mapped before the page-table walk so the
    /// descriptors it generates see their final attributes.
    ///
    /// Returns the checked save-state metadata and the `mmi_entry_size` carried by the HOB.
    ///
    /// ## Safety
    ///
    /// The buffer pointers carried in `data` (e.g. the firmware policy buffer)
    /// must reference valid memory for their declared sizes and remain resident
    /// for the supervisor's lifetime, as they are dereferenced during setup and
    /// runtime.
    pub(crate) unsafe fn init_from_pass_down_hob(
        &self,
        data: &[u8],
        number_of_cpus: usize,
    ) -> MmSupervisorResult<(SaveStateInfo, u64)> {
        let MmSupvPassDownHobData {
            cpl3_stack_base,
            cpl3_stack_size,
            sm_base_array,
            mm_initialized_buffer,
            firmware_policy_buffer,
            firmware_policy_buffer_size,
            mmi_entry_size,
            ..
        } = MmSupvPassDownHobData::parse(data)?;

        // SAFETY: this function's contract requires the pointers the HOB carries to name resident
        // memory of the size they declare.
        unsafe { self.store_mm_initialized_slots(mm_initialized_buffer, number_of_cpus) }?;

        // SAFETY: `check_save_state_regions` proves the SMBASE array and every map it describes
        // before anything reads through them, and the MM IPL keeps them resident for the
        // supervisor's lifetime.
        let save_state_info = unsafe { SaveStateInfo::new(number_of_cpus, sm_base_array) }
            .inspect_err(|err| log::error!("PassDown HOB does not describe a usable SMBASE array: {err}"))?;
        self.check_save_state_regions(&save_state_info)?;

        // SAFETY: as above, for the firmware policy buffer and the size reported beside it.
        let memory_policy_buffer =
            unsafe { self.install_policy_gate(firmware_policy_buffer, firmware_policy_buffer_size) }?;

        self.start_syscall_interface(cpl3_stack_base, cpl3_stack_size, number_of_cpus)?;
        self.map_cpl3_stacks_to_user(cpl3_stack_base, cpl3_stack_size, number_of_cpus)?;

        // SAFETY: `install_policy_gate` returned a freshly allocated page, which holds
        // `MEMORY_POLICY_DESCRIPTORS_PER_PAGE` descriptors and stays resident.
        unsafe { self.record_unblocked_memory(memory_policy_buffer) }?;

        Ok((save_state_info, mmi_entry_size))
    }
}
