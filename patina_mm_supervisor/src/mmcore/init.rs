//! `MmSupervisorCore` Initialization Phase
//!
//! The setup each core runs on its first entry into MM. The BSP also runs
//! [`bsp_init`](MmSupervisorCore::bsp_init): it validates the incoming HOB list, programs the
//! SMRR, sets up the allocators, adopts the page table MM Init built, and publishes a read-only
//! copy of the HOB list for the user core.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    ffi::c_void,
    sync::atomic::{AtomicU8, Ordering},
};

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
    uefi_pages_to_size, uefi_size_to_pages,
};
use patina_internal_cpu::{
    interrupts::Interrupts,
    save_state::{PROCESSOR_INFO_ENTRY_SIZE, vendor_name},
};
use patina_paging::{MemoryAttributes, PageTable, x64::X64PageTable};

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
        validation::validate_page_table_post_activation,
    },
    policy::{self, MemDescriptorV1_0, dump_policy, gate::PolicyGate, walk_page_table},
    state::{init_state, security_state},
};

use super::CoreInitError;
use super::mseg::{MSEG_SMRAM_HOB_GUID, parse_mseg_smram_hob};
use super::per_core::allocate_per_core_slice;
use super::smi_idt_patch::patch_smi_handler_idt;
use super::smrr::{configure_smm_code_access, smrr_initialize};
use crate::hob::pass_down::{MM_SUPV_PASS_DOWN_HOB_GUID, MmSupvPassDownHobData, PassDownHobError};
use crate::memory::mmram::{establish_mmram_bound, supervisor_image_anchor};
use crate::privilege_mgmt::save_state::{SaveStateInfo, log_save_state_map, validate_save_state_regions};

/// GUID of the MP Information HOB (`gMpInformationHobGuid`), which carries the processor count
/// and the `EFI_PROCESSOR_INFORMATION` array used by the save-state read path.
pub(crate) const MP_INFORMATION_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("ba33f15d-4000-45c1-8e88-f91692d457e3");

/// Number of memory policy descriptors one page holds.
///
/// The page-table walk and the policy gate both take this as an entry count, not a byte count,
/// for the single page they share.
const MEMORY_POLICY_DESCRIPTORS_PER_PAGE: usize = UEFI_PAGE_SIZE / core::mem::size_of::<MemDescriptorV1_0>();

const _: () = {
    assert!(
        MEMORY_POLICY_DESCRIPTORS_PER_PAGE * core::mem::size_of::<MemDescriptorV1_0>() <= UEFI_PAGE_SIZE,
        "the memory policy descriptor count must fit the page the page-table walk writes into"
    );
};

/// Verifies that the allocators' own MMRAM, `bookkeeping` and `paging_pool`, is mapped as
/// supervisor data.
///
/// Checks:
/// - Every page of the allocator bookkeeping is present, supervisor-only and non-executable.
/// - Every page of the paging pool is present, supervisor-only and non-executable.
fn verify_allocator_protections(
    page_table: &impl PageTable,
    bookkeeping: MemoryRange,
    paging_pool: MemoryRange,
) -> MmSupervisorResult<()> {
    let supervisor_data = MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect;
    for (kind, range) in [("allocator bookkeeping", bookkeeping), ("paging pool", paging_pool)] {
        validation::verify_page_attributes(page_table, &range, supervisor_data, MemoryAttributes::ReadProtect)
            .inspect_err(|err| log::error!("The {kind} at {range} is not mapped as supervisor data: {err}"))?;
    }
    Ok(())
}

impl<P: PlatformInfo> MmSupervisorCore<P> {
    /// BSP-only, one-time initialization.
    ///
    /// Loads the IDT, validates the incoming HOB list, programs the SMRR, sets up the page and
    /// paging allocators, the page table and the per-core storage, finds the user and Init
    /// modules, initializes the security policy, and publishes a copy of the HOB list for the
    /// user core.
    ///
    /// Returns the address of the read-only HOB list copy to hand the user core.
    // Orchestrates hardware setup (IDT, SMRR, CR3) that host tests cannot run.
    #[cfg_attr(coverage, coverage(off))]
    pub(crate) fn bsp_init(
        &'static self,
        hob_hand_off_table: &PhaseHandoffInformationTable,
    ) -> MmSupervisorResult<u64> {
        log::info!("BSP performing one-time initialization...");

        // The GDT and TSS loaded on MM entry stay in place, so only the IDT is loaded. Its gates
        // use that GDT's 64-bit code segment at 0x38.
        Interrupts::new().initialize_idt(&[]).map_err(CoreInitError::InterruptManagerInit)?;

        // Check the HOB headers before anything walks the list.
        // SAFETY: `hob_hand_off_table` is the MM IPL's HOB list (the caller asserts it is
        // non-null). The walk reads only the headers it reaches and stops at the first bad one.
        let hob_list_len = unsafe { validation::validate_hob_list_structure(hob_hand_off_table)? };

        // Nothing in MMRAM is written until these descriptors are anchored and validated below.
        // SAFETY: the HOB list passed `validate_hob_list_structure` above.
        let scanned = unsafe { PageAllocator::scan_hob_list_for_smram_regions(hob_hand_off_table)? };
        let scanned_regions = scanned.as_slice();

        let smrr_range = establish_mmram_bound(scanned_regions, supervisor_image_anchor())?;

        validation::validate_hob_list_inside_mmram(hob_hand_off_table, hob_list_len, smrr_range)?;

        let number_of_cpus = self.mp_information_cpu_count(hob_hand_off_table, smrr_range)?;
        let pass_down = validation::validate_incoming_hobs_pre_paging_init(
            hob_hand_off_table,
            hob_list_len,
            scanned_regions,
            smrr_range,
            number_of_cpus,
        )?;

        // Program the range before the allocator first writes into MMRAM, but leave enabling it to
        // `smrr_enable` on the next SMI: enabling it now keeps it enforcing after `RSM`, which
        // faults the non-MM code that still reaches into the range.
        smrr_initialize(smrr_range)?;
        init_state().set_smrr_range(smrr_range);

        // SAFETY: the descriptors were anchored and validated above, so the free regions they
        // describe are MMRAM the supervisor owns exclusively.
        let (bookkeeping, paging_pool) = unsafe {
            self.init_page_allocators(
                scanned_regions,
                security_state().page_allocator(),
                security_state().paging_allocator(),
            )?
        };

        self.init_page_table()?;
        validation::with_active_page_table(|page_table| {
            verify_allocator_protections(page_table, bookkeeping, paging_pool)
        })?;

        // The page table exists now, so these pages are mapped supervisor-only and non-executable.
        let page_allocator = security_state().page_allocator();
        self.cpu_manager.init(allocate_per_core_slice(page_allocator, number_of_cpus)?)?;
        self.mailbox_manager.init(allocate_per_core_slice(page_allocator, number_of_cpus)?)?;
        log::info!("Allocated per-core storage for {number_of_cpus} cores");

        // The checks that need the page table.
        validation::validate_incoming_hobs_post_paging_init(hob_hand_off_table, &pass_down, number_of_cpus)?;
        validate_page_table_post_activation(hob_hand_off_table, &pass_down, number_of_cpus, smrr_range)?;

        self.discover_and_store_user_entry(hob_hand_off_table, init_state())?;
        self.discover_and_store_init_region(hob_hand_off_table, init_state())?;
        self.init_policy_and_validate(hob_hand_off_table, number_of_cpus)?;
        let user_hob_list = self.publish_hob_list_to_user(hob_hand_off_table)?;

        log::info!("BSP one-time initialization complete.");
        Ok(user_hob_list)
    }

    /// Commits the validated SMRAM regions to the page allocator and initializes the paging
    /// allocator from a pool reserved out of it. Returns the MMRAM the two allocators keep for
    /// themselves, as `(bookkeeping, paging_pool)`.
    ///
    /// ## Safety
    ///
    /// Every non-pre-allocated region in `scanned` must be valid, exclusively owned MMRAM.
    pub(crate) unsafe fn init_page_allocators(
        &self,
        scanned: &[MmramRegion],
        page_allocator: &memory::PageAllocator,
        paging_allocator: &memory::PagingPoolAllocator,
    ) -> MmSupervisorResult<(MemoryRange, MemoryRange)> {
        // SAFETY: the caller guarantees that the free regions in `scanned` are valid, exclusively
        // owned MMRAM.
        let bookkeeping = unsafe { page_allocator.init_from_regions(scanned)? };

        // Reserved before paging is initialized, which draws its structures from this pool.
        let paging_pool_base = page_allocator.allocate_pages(memory::DEFAULT_PAGING_POOL_PAGES)?;

        log::info!(
            "Reserved {} pages at 0x{:016x} for paging structures",
            memory::DEFAULT_PAGING_POOL_PAGES,
            paging_pool_base
        );

        // SAFETY: `paging_pool_base` was just reserved from the page allocator, so it is a
        // page-aligned region of `DEFAULT_PAGING_POOL_PAGES` pages in SMRAM owned exclusively by
        // the paging allocator.
        unsafe {
            paging_allocator.init(paging_pool_base, memory::DEFAULT_PAGING_POOL_PAGES)?;
        }

        // `paging_allocator.init` accepted these pages, so the range is valid.
        let paging_pool =
            MemoryRange::from_pages(paging_pool_base, memory::DEFAULT_PAGING_POOL_PAGES as u64).map_err(|_| {
                HobValidationError::InvalidAddressRange {
                    kind: "paging pool",
                    base: paging_pool_base,
                    length: uefi_pages_to_size!(memory::DEFAULT_PAGING_POOL_PAGES) as u64,
                }
            })?;

        Ok((bookkeeping, paging_pool))
    }

    /// Adopts the active CR3 hierarchy, 4-level or 5-level as CR4.LA57 selects, as the global
    /// page table.
    ///
    /// Must run after [`init_page_allocators`](Self::init_page_allocators), because the page
    /// table draws its backing memory from the paging allocator.
    ///
    /// ## Errors
    ///
    /// Returns `PageTableFromCr3Failed` if the hierarchy at CR3 cannot be adopted.
    // Adopts the live CR3 hierarchy, which host tests cannot provide.
    #[cfg_attr(coverage, coverage(off))]
    fn init_page_table(&self) -> MmSupervisorResult<()> {
        let cr3 = read_cr3();
        let paging_alloc = SharedPagingAllocator::new(security_state().paging_allocator());
        // SAFETY: the active hierarchy is valid, being in use by this core, and this becomes the
        // only manager that modifies it: it is kept behind the security-state lock. `paging_alloc`
        // owns the pool new paging structures come from.
        let page_table = unsafe { X64PageTable::open_active(paging_alloc) }.map_err(|err| {
            log::error!("Failed to create page table from active CR3=0x{cr3:016x}: {err:?}");
            CoreInitError::PageTableFromCr3Failed { cr3 }
        })?;
        *security_state().lock_page_table() = Some(page_table);
        log::info!("Page table initialized from CR3=0x{cr3:016x}");
        Ok(())
    }

    /// Finds the MM Supervisor User module in the HOB list and stores its entry point.
    ///
    /// The module is the memory allocation module HOB named
    /// `MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID` with module name `MM_SUPERVISOR_USER_GUID`.
    /// Returns [`CoreInitError::UserEntryPointMissing`] if there is none.
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

    /// Saves the MM Init module allocation so it can be freed after the HOB list is reclaimed.
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

    /// Frees the saved MM Init allocation on the BSP's first runtime entry. Does nothing once the
    /// allocation is freed.
    ///
    /// ## Panics
    ///
    /// Panics if the allocation is outside MMRAM, no page table is installed, or the pages cannot
    /// be freed.
    pub(crate) fn free_init_module(&self, state: &crate::state::InitState) {
        if state.is_init_module_freed() {
            return;
        }
        let Some(init_module) = state.init_module_region() else {
            log::warn!("MM Init module region was not discovered during initialization");
            return;
        };
        assert!(is_range_inside_mmram(&init_module), "MM Init module is outside MMRAM");
        // The free unmaps the image. Without a page table it would only log, and leave the freed
        // pages mapped.
        assert!(security_state().lock_page_table().is_some(), "Page table required to free MM Init module");

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

    /// Records the core count and sets up everything the `PassDown` HOB describes, then the save
    /// state metadata, MSEG base, SMI handler IDT descriptors and communication buffers, and
    /// finally runs the initial security policy check.
    fn init_policy_and_validate(
        &self,
        hob_hand_off_table: &PhaseHandoffInformationTable,
        number_of_cores: usize,
    ) -> MmSupervisorResult<()> {
        self.number_of_cores.store(number_of_cores, Ordering::Release);

        // SAFETY: `hob_hand_off_table` is the in-place PHIT of the validated HOB list handed to
        // `bsp_init`, which is terminated by `END_OF_HOB_LIST`.
        let pass_down_data = unsafe { find_guid_hob_data(hob_hand_off_table, MM_SUPV_PASS_DOWN_HOB_GUID) }
            .ok_or(PassDownHobError::Missing)?;
        // SAFETY: `pass_down_data` is a slice into the validated HOB list, so the buffer pointers
        // it carries reference live memory as `init_from_pass_down_hob` requires.
        let save_state_info = unsafe { self.init_from_pass_down_hob(pass_down_data, number_of_cores) }?;

        // `init_from_pass_down_hob` has just checked every save-state region it describes.
        self.publish_save_state_metadata(save_state_info);

        self.discover_mseg_base(hob_hand_off_table);

        // The SMI entry code is copied per SMBASE, so every core's copy is patched here.
        patch_smi_handler_idt(save_state_info.smbase_array(), number_of_cores, save_state_info.mmi_entry_size())?;

        self.init_comm_buffers(hob_hand_off_table)?;

        let gate = security_state().policy_gate().ok_or(CoreInitError::PolicyGateUnavailable)?;
        policy::validation::security_policy_check(gate)?;

        log::info!("Security policy check passed");
        Ok(())
    }

    /// Publishes a read-only copy of the HOB list for the user core and returns its address, then
    /// reclaims the whole pages of the original.
    ///
    /// The list is copied rather than remapped because mapping is page-granular: remapping would
    /// also expose whatever the producer left in the rest of its last page. The copy's tail is
    /// zeroed instead.
    fn publish_hob_list_to_user(&self, hob_hand_off_table: &PhaseHandoffInformationTable) -> MmSupervisorResult<u64> {
        let hob_list: *const c_void = core::ptr::from_ref(hob_hand_off_table).cast();
        let hob_base = hob_list as u64;
        // SAFETY: `hob_list` is the PHIT of the HOB list `bsp_init` validated, which ends in
        // `END_OF_HOB_LIST`.
        let hob_list_size = unsafe { hob::get_pi_hob_list_size(hob_list) } as u64;

        // A zero-length list is rejected, and the range is confirmed to be MMRAM before it is read.
        let hob_range = MemoryRange::new(hob_base, hob_list_size)
            .ok()
            .filter(is_range_inside_mmram)
            .ok_or(HobValidationError::HobListOutsideMmram { base: hob_base, size: hob_list_size })?;

        let size =
            usize::try_from(hob_list_size).map_err(|_| CoreInitError::HobListTooLargeToCopy { size: hob_list_size })?;

        let pages = uefi_size_to_pages!(size);
        // Ring 3 borrows this allocation read-only; it must not be able to free it.
        let copy_base = security_state().page_allocator().allocate_pages(pages)?;
        let copy_size = uefi_pages_to_size!(pages);

        // Zero the whole allocation because Ring 3 can read the tail of the last page.
        // SAFETY: the validated producer list is supervisor-readable. The fresh supervisor
        // allocation is disjoint from it, holds `copy_size` bytes, and is not shared until below.
        unsafe {
            memory::write_zeros_volatile(copy_base as *mut u8, copy_size);
            memory::copy_volatile(hob_base as *const u8, copy_base as *mut u8, size);
        }

        // The page table guard is a temporary, so the lock is released at the end of this
        // statement, before the reclaim below takes it again.
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

        // The user core reads only the copy, so the original's pages go back to the allocator,
        // which zeroes them and makes them inaccessible. Only pages lying wholly inside the list
        // are freed, since a partial page at either end may hold other data.
        if let Ok(whole_pages) = hob_range.align_inward(UEFI_PAGE_SIZE as u64) {
            let (base, pages) = (whole_pages.base(), whole_pages.pages() as usize);
            match security_state().page_allocator().free_pages_checked(base, pages, AllocationType::Supervisor) {
                Ok(()) => log::info!("Reclaimed {pages} page(s) of the producer's HOB list at {whole_pages}"),
                // Best effort, but a failure means the descriptors got the range wrong.
                Err(err) => log::error!("Failed to reclaim the producer's HOB list at {whole_pages}: {err:?}"),
            }
        }

        Ok(copy_base)
    }

    /// Programs the per-logical-processor MSRs on each core's first entry: the MSEG base, the
    /// SMRR, and SMM Code Access Check.
    pub(crate) fn per_core_init(&'static self, cpu_id: u32) -> MmSupervisorResult<()> {
        log::trace!("CPU {cpu_id} performing per-core initialization...");

        super::mseg::program_mseg_base(cpu_id);

        // The BSP already programmed this range in `bsp_init`, so it writes the same values again.
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
            save_state_info.number_of_cores(),
            save_state_info.smbase_array(),
            vendor_name()
        );
    }

    /// Records the MSEG base for an STM, when the platform published the MSEG SMRAM HOB. Its
    /// absence is not an error.
    fn discover_mseg_base(&self, hob_hand_off_table: &PhaseHandoffInformationTable) {
        // SAFETY: `hob_hand_off_table` is the in-place PHIT of the validated HOB list handed to
        // `bsp_init`, which is terminated by `END_OF_HOB_LIST`.
        match unsafe { find_guid_hob_data(hob_hand_off_table, MSEG_SMRAM_HOB_GUID) }.and_then(parse_mseg_smram_hob) {
            Some((mseg_base, _)) => {
                init_state().set_mseg_base(mseg_base);
                log::info!("MSEG base 0x{mseg_base:x} discovered from MSEG SMRAM HOB");
            }
            None => log::warn!("No usable MSEG SMRAM HOB; IA32_SMM_MONITOR_CTL will not be programmed"),
        }
    }

    /// Builds the supervisor and user communication channels and stores the configuration,
    /// along with a newly allocated supervisor-to-user page.
    fn init_comm_buffers(&self, hob_hand_off_table: &PhaseHandoffInformationTable) -> MmSupervisorResult<()> {
        // SAFETY: `hob_hand_off_table` is the PHIT of the validated HOB list.
        let supv_region_data = unsafe { find_guid_hob_data(hob_hand_off_table, MM_COMMON_REGION_HOB_GUID) }
            .ok_or(CommBufferError::CommRegionHobMissing)?;
        let supv = CommChannel::from_supv_hob(supv_region_data)
            .inspect_err(|err| log::error!("Failed to initialize supervisor communication buffer: {err}"))?;

        // The user core may read this HOB too, so it is redirected to the internal copy.
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

    /// Returns the CPU count from the MP Information HOB, whose payload must lie inside `mmram`
    /// because it is read in place.
    fn mp_information_cpu_count(
        &self,
        hob_hand_off_table: &PhaseHandoffInformationTable,
        mmram: MemoryRange,
    ) -> MmSupervisorResult<usize> {
        // SAFETY: `hob_hand_off_table` is the in-place PHIT of the HOB list handed to `bsp_init`,
        // which passed `validate_hob_list_structure`, so the walk ends at its `END_OF_HOB_LIST`.
        let mp_information = unsafe { find_guid_hob_data(hob_hand_off_table, MP_INFORMATION_HOB_GUID) }
            .ok_or(CoreInitError::MpInformationHobMissing)?;

        // An empty payload is left for the parse below to report as malformed.
        let inside_mmram = MemoryRange::new(mp_information.as_ptr().addr() as u64, mp_information.len() as u64)
            .map_or(mp_information.is_empty(), |payload| mmram.contains_range(&payload));
        if !inside_mmram {
            return Err(CoreInitError::MpInformationHobOutsideMmram.into());
        }

        self.parse_mp_information_hob(mp_information)
    }

    /// Returns the core count from the MP Information HOB payload.
    ///
    /// The count must be non-zero and every processor entry it counts must fit the payload.
    fn parse_mp_information_hob(&self, data: &[u8]) -> MmSupervisorResult<usize> {
        /// Offset of `ProcessorInfoBuffer[]` within `MP_INFORMATION_HOB_DATA`.
        const PROCESSOR_INFO_BUFFER_OFFSET: usize = 16;

        if data.len() < PROCESSOR_INFO_BUFFER_OFFSET {
            log::error!("MP Information HOB too small: {} < {}", data.len(), PROCESSOR_INFO_BUFFER_OFFSET);
            return Err(CoreInitError::MpInformationHobMalformed.into());
        }

        // The length check above makes these failures unreachable, but they are still reported.
        let number_of_cores = u64::from_le_bytes(
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

        // A count too large for `usize` becomes `usize::MAX`, which the size check below rejects.
        let core_count = usize::try_from(number_of_cores).unwrap_or(usize::MAX);
        if core_count == 0 {
            log::error!("MP Information HOB reports zero cores");
            return Err(CoreInitError::ZeroCoreCount.into());
        }

        let processor_info_size = core_count.checked_mul(PROCESSOR_INFO_ENTRY_SIZE).ok_or_else(|| {
            log::error!("MP Information HOB: {core_count} processor entries overflow the payload size");
            CoreInitError::MpInformationHobMalformed
        })?;

        let processor_info_end = PROCESSOR_INFO_BUFFER_OFFSET.checked_add(processor_info_size).ok_or_else(|| {
            log::error!("MP Information HOB: processor info end offset overflows");
            CoreInitError::MpInformationHobMalformed
        })?;

        data.get(PROCESSOR_INFO_BUFFER_OFFSET..processor_info_end).ok_or_else(|| {
            log::error!(
                "MP Information HOB holds {} bytes but {core_count} processor entries need {processor_info_end}",
                data.len()
            );
            CoreInitError::MpInformationHobMalformed
        })?;

        Ok(core_count)
    }

    /// Publishes the per-core "MM initialized" slots the MM IPL allocated.
    ///
    /// The buffer must be `number_of_cores` bytes inside MMRAM, which also rules out a null
    /// address, and mapped supervisor-only so Ring 3 cannot mark a core as initialized.
    ///
    /// ## Safety
    ///
    /// `buffer` must name `number_of_cores` bytes that stay resident for the supervisor's
    /// lifetime.
    unsafe fn store_mm_initialized_slots(&self, buffer: u64, number_of_cores: usize) -> MmSupervisorResult<()> {
        // The slice built below relies on this range, so it is checked rather than assumed.
        if number_of_cores == 0 {
            return Err(CoreInitError::ZeroCoreCount.into());
        }
        let range =
            MemoryRange::new(buffer, number_of_cores as u64).ok().filter(is_range_inside_mmram).ok_or_else(|| {
                log::error!(
                    "MM initialized buffer at 0x{buffer:016x} does not contain {number_of_cores} slot(s) in MMRAM"
                );
                CoreInitError::InitializedBufferInvalid
            })?;
        if !matches!(query_address_ownership(&range), Some(PageOwnership::Supervisor)) {
            log::error!("MM initialized buffer {range} is not mapped supervisor-only");
            return Err(CoreInitError::InitializedBufferInvalid.into());
        }

        let address = usize::try_from(buffer).map_err(|_| CoreInitError::InitializedBufferInvalid)?;
        let buffer_ptr = core::ptr::with_exposed_provenance::<AtomicU8>(address);
        // SAFETY: the checks above place `number_of_cores` one-byte slots inside MMRAM at
        // `buffer`, and this function's contract keeps them resident.
        let initialized_slots = unsafe { core::slice::from_raw_parts(buffer_ptr, number_of_cores) };
        init_state().set_mm_initialized_buffer(initialized_slots);
        log::info!("MM Initialized buffer set to 0x{buffer:016x} with {number_of_cores} slot(s)");

        Ok(())
    }

    /// Validates the per-CPU save-state regions the `PassDown` HOB describes, so the save-state
    /// syscall can read them without checking again.
    ///
    /// Checks:
    /// - The SMBASE array and every save-state map pass [`validate_save_state_regions`], against
    ///   MMRAM and supervisor-only page ownership.
    fn check_save_state_regions(&self, save_state_info: &SaveStateInfo) -> MmSupervisorResult<()> {
        validate_save_state_regions(save_state_info, is_range_inside_mmram, |range| {
            matches!(query_address_ownership(range), Some(PageOwnership::Supervisor))
        })
        .inspect_err(|err| log::error!("PassDown HOB does not describe usable save-state regions: {err}"))?;
        log::info!(
            "Validated save-state regions for {} CPU(s) from SMBASE array at 0x{:016x}",
            save_state_info.number_of_cores(),
            save_state_info.smbase_array()
        );

        Ok(())
    }

    /// Builds the policy gate over the firmware policy blob and installs it.
    ///
    /// Returns a newly allocated page for the memory policy descriptors, which the gate keeps as
    /// its snapshot buffer and [`record_unblocked_memory`](Self::record_unblocked_memory) fills.
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

    /// Starts the syscall interface over the Ring 3 stacks the MM IPL provisioned, one per core.
    fn start_syscall_interface(
        &self,
        cpl3_stack_base: u64,
        cpl3_stack_size: u64,
        number_of_cores: usize,
    ) -> MmSupervisorResult<()> {
        let stack_size = usize::try_from(cpl3_stack_size).map_err(|_| CoreInitError::Cpl3StackRegionInvalid {
            base: cpl3_stack_base,
            per_core_size: cpl3_stack_size,
            number_of_cores: number_of_cores,
        })?;

        self.syscall_interface.init(number_of_cores, cpl3_stack_base, stack_size)?;
        Ok(())
    }

    /// Seeds the unblocked memory tracker, which later `UNBLOCK_MEM` requests are checked
    /// against, with the memory outside MMRAM that the page table currently maps.
    ///
    /// ## Safety
    ///
    /// `memory_policy_buffer` must name a page with room for
    /// [`MEMORY_POLICY_DESCRIPTORS_PER_PAGE`] descriptors that stays resident.
    unsafe fn record_unblocked_memory(&self, memory_policy_buffer: u64) -> MmSupervisorResult<()> {
        let descriptors = memory_policy_buffer as *mut MemDescriptorV1_0;
        let cr3 = read_cr3();

        // SAFETY: `cr3` is the live root page table, and this function's contract gives
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
    /// Sets up, in order: the per-core initialized slots, the save-state metadata, the policy
    /// gate, the syscall interface and the unblocked memory tracker. Returns the checked
    /// save-state metadata.
    ///
    /// ## Safety
    ///
    /// The buffer pointers carried in `data` must reference valid memory for their declared sizes
    /// and stay resident for the supervisor's lifetime.
    pub(crate) unsafe fn init_from_pass_down_hob(
        &self,
        data: &[u8],
        number_of_cores: usize,
    ) -> MmSupervisorResult<SaveStateInfo> {
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

        // SAFETY: this function's contract requires the HOB's pointers to name resident memory of
        // the declared size.
        unsafe { self.store_mm_initialized_slots(mm_initialized_buffer, number_of_cores) }?;

        // SAFETY: `check_save_state_regions` validates the SMBASE array and every map before
        // anything reads through them, and the MM IPL keeps them resident.
        let save_state_info = unsafe { SaveStateInfo::new(number_of_cores, sm_base_array, mmi_entry_size) }
            .inspect_err(|err| log::error!("PassDown HOB does not describe a usable SMBASE array: {err}"))?;
        self.check_save_state_regions(&save_state_info)?;

        // SAFETY: as above, for the firmware policy buffer and the size reported beside it.
        let memory_policy_buffer =
            unsafe { self.install_policy_gate(firmware_policy_buffer, firmware_policy_buffer_size) }?;

        self.start_syscall_interface(cpl3_stack_base, cpl3_stack_size, number_of_cores)?;

        // SAFETY: `install_policy_gate` returned a freshly allocated page, which holds
        // `MEMORY_POLICY_DESCRIPTORS_PER_PAGE` descriptors and stays resident.
        unsafe { self.record_unblocked_memory(memory_policy_buffer) }?;

        Ok(save_state_info)
    }
}
