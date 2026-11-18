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
        self, AllocationType, PageAllocator, SharedPagingAllocator,
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::error::MmSupervisorError;
    use crate::hob::validation::HobValidationError;
    use crate::{memory, test_support};
    use crate::{
        memory::{PAGE_STEP, PageAllocator, PagingPoolAllocator},
        state::InitState,
    };
    use patina::management_mode::supervisor::{MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_USER_GUID};
    use patina::{management_mode::supervisor::MM_SUPERVISOR_CORE_GUID, pi::hob::PhaseHandoffInformationTable};
    use patina_internal_cpu::save_state::PROCESSOR_INFO_ENTRY_SIZE;
    use patina_paging::PageTable;
    use serial_test::serial;

    use crate::test_support::*;

    #[test]
    fn test_scan_hob_list_does_not_write_the_memory_it_describes() {
        // Scanning must not commit to the producer's descriptors. Nothing in the memory they name
        // may be touched until they have been anchored and validated, or a forged descriptor
        // steers a write before anything has had the chance to reject it.
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        let hob_list = smram_hob_list(&memory);
        let size = memory.size() as usize;
        // SAFETY: `memory` is a live, exclusively owned allocation of `size` bytes and no
        // references into it are held here.
        unsafe { core::ptr::write_bytes(memory.base() as *mut u8, 0xa5, size) };

        let scanned = scan_regions(&hob_list);

        assert_eq!(scanned, [MmramRegion::new(memory.range(), false)]);
        // SAFETY: the same live allocation, read through a shared view while nothing else
        // references it.
        let after_scan = unsafe { core::slice::from_raw_parts(memory.base() as *const u8, size) };
        assert!(after_scan.iter().all(|byte| *byte == 0xa5), "scanning wrote into the memory the HOB list describes");

        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let page_allocator = PageAllocator::new();
        let paging_allocator = PagingPoolAllocator::new();
        // SAFETY: the scanned descriptor references the live, exclusively owned `memory`.
        unsafe { supervisor.init_page_allocators(&scanned, &page_allocator, &paging_allocator) }
            .expect("the scanned region supports both allocators");

        // SAFETY: the same live allocation, read after the allocator has finished with it.
        let after_commit = unsafe { core::slice::from_raw_parts(memory.base() as *const u8, size) };
        assert!(
            after_commit.iter().any(|byte| *byte != 0xa5),
            "committing left the bookkeeping region untouched, so the test proves nothing"
        );
    }

    #[test]
    fn test_init_page_allocators_commits_scanned_regions() {
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        let hob_list = smram_hob_list(&memory);
        let scanned = scan_regions(&hob_list);
        let page_allocator = PageAllocator::new();
        let paging_allocator = PagingPoolAllocator::new();

        assert_eq!(scanned, [MmramRegion::new(memory.range(), false)]);

        // SAFETY: the scanned descriptor references the live, exclusively owned page-aligned
        // `memory` allocation.
        let (bookkeeping, paging_pool) =
            unsafe { supervisor.init_page_allocators(&scanned, &page_allocator, &paging_allocator) }
                .expect("the scanned region supports both allocators");

        assert!(page_allocator.is_initialized());
        assert!(paging_allocator.is_initialized());
        assert_eq!(paging_allocator.free_page_count(), memory::DEFAULT_PAGING_POOL_PAGES);

        // The bookkeeping starts the free region, and both regions are reported apart, inside it.
        assert_eq!(bookkeeping.base(), memory.base());
        assert_eq!(paging_pool.pages(), memory::DEFAULT_PAGING_POOL_PAGES as u64);
        assert!(memory.range().contains_range(&bookkeeping));
        assert!(memory.range().contains_range(&paging_pool));
        assert!(!bookkeeping.overlaps(&paging_pool));
    }

    #[test]
    fn test_verify_allocator_protections_requires_supervisor_data() {
        use patina_paging::{MockPageTable, PtError};

        let bookkeeping = MemoryRange::new(0x1000, 0x1000).unwrap();
        let paging_pool = MemoryRange::new(0x4000, 0x2000).unwrap();
        let verify = |page_table: &MockPageTable| verify_allocator_protections(page_table, bookkeeping, paging_pool);
        let page_table = |bookkeeping: MemoryAttributes, pool: MemoryAttributes| {
            let mut page_table = MockPageTable::new();
            page_table
                .expect_query_memory_region()
                .withf(|&addr, _| addr == 0x1000)
                .returning(move |_, _| Ok(bookkeeping));
            page_table
                .expect_query_memory_region()
                .withf(|&addr, _| addr == 0x4000 || addr == 0x5000)
                .returning(move |_, _| Ok(pool));
            page_table
        };
        let data = MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect;
        assert_eq!(verify(&page_table(data, data)), Ok(()));

        // User-accessible bookkeeping.
        let user = MemoryAttributes::ExecuteProtect;
        assert_eq!(
            verify(&page_table(user, data)),
            Err(HobValidationError::PageMissingAttribute { addr: 0x1000, desired: data.bits(), found: user.bits() }
                .into())
        );

        // An executable page in the paging pool.
        let code = MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly;
        assert_eq!(
            verify(&page_table(data, code)),
            Err(HobValidationError::PageMissingAttribute { addr: 0x4000, desired: data.bits(), found: code.bits() }
                .into())
        );

        // A page that is not mapped at all.
        let mut unmapped = MockPageTable::new();
        unmapped.expect_query_memory_region().returning(|_, _| Err(PtError::NoMapping));
        assert_eq!(verify(&unmapped), Err(HobValidationError::PageAttributeQueryFailed { addr: 0x1000 }.into()));
    }

    #[test]
    fn test_init_page_allocators_reports_paging_pool_allocation_failure() {
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let memory = PageAlignedMemory::new(2);
        let hob_list = smram_hob_list(&memory);
        let scanned = scan_regions(&hob_list);
        let page_allocator = PageAllocator::new();
        let paging_allocator = PagingPoolAllocator::new();

        // SAFETY: the scanned descriptor references the live `memory` allocation.
        let result = unsafe { supervisor.init_page_allocators(&scanned, &page_allocator, &paging_allocator) };

        assert!(result.is_err(), "a region too small for the paging pool must be reported");
        assert!(page_allocator.is_initialized());
        assert!(!paging_allocator.is_initialized());
    }

    #[test]
    fn test_init_page_allocators_reports_paging_allocator_failure() {
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let existing_pool = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES);
        let paging_allocator = PagingPoolAllocator::new();
        // SAFETY: `existing_pool` is page-aligned, exclusively owned, and remains alive.
        unsafe {
            paging_allocator
                .init(existing_pool.base(), memory::DEFAULT_PAGING_POOL_PAGES)
                .expect("pre-initialize paging allocator");
        }

        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        let hob_list = smram_hob_list(&memory);
        let scanned = scan_regions(&hob_list);
        let page_allocator = PageAllocator::new();

        // SAFETY: the scanned descriptor references the live `memory` allocation.
        let result = unsafe { supervisor.init_page_allocators(&scanned, &page_allocator, &paging_allocator) };

        assert!(result.is_err(), "an already-initialized paging allocator must be reported");
    }

    #[test]
    fn test_discover_and_store_user_entry_walks_real_hob_list() {
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let state = InitState::new();
        let mut hob_list = RawHobList::new();
        hob_list.push_struct(allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_USER_GUID,
            0x1234_5678,
        ));
        let hob_list = hob_list.finish();
        supervisor
            .discover_and_store_user_entry(hob_list.handoff(), &state)
            .expect("the synthetic HOB list is well formed");

        assert_eq!(state.user_entry_point(), Some(0x1234_5678));
    }

    #[test]
    fn test_discover_and_store_user_entry_reports_a_missing_module() {
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let state = InitState::new();
        let hob_list = RawHobList::new().finish();

        let result = supervisor.discover_and_store_user_entry(hob_list.handoff(), &state);

        assert_eq!(result, Err(CoreInitError::UserEntryPointMissing.into()));
        assert_eq!(state.user_entry_point(), None);
    }

    #[test]
    #[serial]
    fn test_publish_hob_list_to_user_copies_the_list_and_reclaims_whole_pages() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 32);
        init_global_state_over(&memory);

        // The producer's list starts mid-page, so only the whole pages inside it can be reclaimed.
        let staging = security_state().page_allocator().allocate_pages(6).expect("staging pages");
        let producer = staging + UEFI_PAGE_SIZE as u64 / 2;
        let source = padded_hob_list(3 * UEFI_PAGE_SIZE);
        // SAFETY: `source` is a valid HOB list and `producer` is inside a live six-page
        // allocation that comfortably holds it.
        let size = unsafe { hob::get_pi_hob_list_size(source.as_ptr()) };
        // SAFETY: source and destination are separate live allocations of at least `size` bytes.
        unsafe { core::ptr::copy_nonoverlapping(source.as_ptr().cast::<u8>(), producer as *mut u8, size) };

        let reclaimed = MemoryRange::new(producer, size as u64)
            .and_then(|list| list.align_inward(UEFI_PAGE_SIZE as u64))
            .expect("the producer's list spans whole pages")
            .pages() as usize;
        assert_eq!(reclaimed, 2, "the producer's list must span whole pages for the reclaim to run");

        let free_before = security_state().page_allocator().free_page_count();
        // SAFETY: `producer` now holds a valid HOB list inside the committed MMRAM region, so it
        // begins with a Phase Handoff Information Table that stays live for the borrow.
        let handoff = unsafe { &*(producer as *const PhaseHandoffInformationTable) };
        let copy = supervisor.publish_hob_list_to_user(handoff).expect("publishing the HOB list copy should succeed");

        assert_ne!(copy, 0);
        assert_eq!(security_state().page_allocator().get_allocation_type(copy), Some(AllocationType::Supervisor));
        assert_eq!(
            security_state().page_allocator().free_pages_checked(copy + UEFI_PAGE_SIZE as u64, 1, AllocationType::User),
            Err(memory::AllocError::TypeMismatch.into()),
            "Ring 3 must not free even an interior page of its read-only HOB copy"
        );

        // SAFETY: `copy` is a live allocation of at least `size` bytes the supervisor just filled.
        let published = unsafe { core::slice::from_raw_parts(copy as *const u8, size) };
        // SAFETY: `source` is still live and holds the bytes the copy was taken from.
        let original = unsafe { core::slice::from_raw_parts(source.as_ptr().cast::<u8>(), size) };
        assert_eq!(published, original, "the published copy does not match the producer's list");

        let copy_pages = uefi_size_to_pages!(size);
        let tail = uefi_pages_to_size!(copy_pages) - size;
        // SAFETY: the allocation is page-granular, so the slack after the list is live memory.
        let slack = unsafe { core::slice::from_raw_parts((copy + size as u64) as *const u8, tail) };
        assert!(slack.iter().all(|byte| *byte == 0), "the tail Ring 3 can read was not zeroed");

        // The whole pages of the producer's list went back to the pool, offsetting the copy.
        assert_eq!(security_state().page_allocator().free_page_count(), free_before - copy_pages + reclaimed);
    }

    #[test]
    #[serial]
    fn test_publish_hob_list_to_user_reports_a_failed_reclaim() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 32);
        init_global_state_over(&memory);

        // Keep the producer mapped supervisor-only, but give it the wrong allocator type so
        // reclaim fails after the user copy has been published.
        let staging =
            security_state().page_allocator().allocate_pages_with_type(4, AllocationType::User).expect("staging pages");
        security_state()
            .lock_page_table()
            .as_mut()
            .unwrap()
            .map_memory_region(
                staging,
                4 * UEFI_PAGE_SIZE as u64,
                MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect,
            )
            .expect("map the producer's HOB list supervisor-only");
        let source = padded_hob_list(2 * UEFI_PAGE_SIZE);
        // SAFETY: `source` is a valid HOB list.
        let size = unsafe { hob::get_pi_hob_list_size(source.as_ptr()) };
        // SAFETY: source and destination are separate live allocations of at least `size` bytes.
        unsafe { core::ptr::copy_nonoverlapping(source.as_ptr().cast::<u8>(), staging as *mut u8, size) };

        // SAFETY: `staging` now holds a valid HOB list inside the committed MMRAM region, so it
        // begins with a Phase Handoff Information Table that stays live for the borrow.
        let handoff = unsafe { &*(staging as *const PhaseHandoffInformationTable) };
        let copy = supervisor.publish_hob_list_to_user(handoff).expect("publishing the HOB list copy should succeed");

        assert_ne!(copy, 0);
        // The reclaim was refused, so the producer's pages are still marked user-allocated.
        assert_eq!(security_state().page_allocator().get_allocation_type(staging), Some(AllocationType::User));
    }

    #[test]
    #[serial]
    fn test_publish_hob_list_to_user_rejects_a_list_outside_mmram() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        init_global_state_over(&memory);

        // A list placed outside the regions the producer described is refused before it is read,
        // and reported as an error so the caller can see why.
        let outside = padded_hob_list(0);

        let result = supervisor.publish_hob_list_to_user(outside.handoff());

        assert!(
            matches!(result, Err(MmSupervisorError::HobValidation(HobValidationError::HobListOutsideMmram { .. }))),
            "a HOB list outside MMRAM was published to Ring 3, got {result:?}"
        );
    }

    #[test]
    #[serial]
    fn test_publish_hob_list_to_user_refuses_to_publish_without_a_page_table() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        let hob_list = smram_hob_list(&memory);
        let scanned = scan_regions(&hob_list);
        // SAFETY: the scanned descriptor references the live, exclusively owned `memory`.
        unsafe {
            supervisor
                .init_page_allocators(&scanned, security_state().page_allocator(), security_state().paging_allocator())
                .expect("the scanned region supports both allocators");
        }

        // Without a page table the copy cannot be made read-only, so it must not be handed to
        // Ring 3 writable instead.
        let staging = security_state().page_allocator().allocate_pages(2).expect("staging pages");
        let source = padded_hob_list(0);
        // SAFETY: `source` is a valid HOB list.
        let size = unsafe { hob::get_pi_hob_list_size(source.as_ptr()) };
        // SAFETY: source and destination are separate live allocations of at least `size` bytes.
        unsafe { core::ptr::copy_nonoverlapping(source.as_ptr().cast::<u8>(), staging as *mut u8, size) };

        // SAFETY: `staging` holds a valid HOB list inside the committed MMRAM region, so it
        // begins with a Phase Handoff Information Table that stays live for the borrow.
        let handoff = unsafe { &*(staging as *const PhaseHandoffInformationTable) };
        let result = supervisor.publish_hob_list_to_user(handoff);

        // The refusal is reported rather than panicked, so the caller can see which step failed.
        assert_eq!(
            result,
            Err(CoreInitError::PageTableUnavailable.into()),
            "the HOB list copy was published without being mapped read-only"
        );
    }

    #[test]
    fn test_free_init_module_does_not_mark_missing_image_as_freed() {
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let state = InitState::new();
        let hob_list = RawHobList::new().finish();

        // Discovery reports the missing module rather than leaving the region unset.
        let result = supervisor.discover_and_store_init_region(hob_list.handoff(), &state);
        assert_eq!(result, Err(CoreInitError::InitModuleRegionMissing.into()));

        assert_eq!(state.init_module_region(), None);
        supervisor.free_init_module(&state);

        assert!(!state.is_init_module_freed());
    }

    #[test]
    fn test_discover_init_region_rejects_an_empty_allocation() {
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let state = InitState::new();
        let mut init_module = allocation_module(HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, 0x10_0000);
        init_module.alloc_descriptor.memory_length = 0;
        let mut hobs = RawHobList::new();
        hobs.push_struct(init_module);
        let hob_list = hobs.finish();

        assert_eq!(
            supervisor.discover_and_store_init_region(hob_list.handoff(), &state),
            Err(CoreInitError::InitModuleRegionInvalid { base: 0x10_0000, length: 0 }.into())
        );
        assert_eq!(state.init_module_region(), None);
    }

    #[test]
    fn test_free_init_module_skips_already_freed_region() {
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let state = InitState::new();
        state.mark_init_module_freed();

        supervisor.free_init_module(&state);
    }

    #[test]
    fn test_free_init_module_releases_mixed_code_and_data_exactly_once() {
        let mapped = MappedInitModule::new();
        let base = mapped.init_module.alloc_descriptor.memory_base_address;
        let size = mapped.init_module.alloc_descriptor.memory_length as usize;
        let image = MemoryRange::new(base, size as u64).unwrap();
        let allocator = security_state().page_allocator();
        let free_pages = allocator.free_page_count();
        // SAFETY: the inactive test page table does not change the host allocation's writable mapping.
        let bytes = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, size) };
        bytes.fill(0xA5);

        mapped.free();

        assert!(mapped.state.is_init_module_freed());
        assert_eq!(allocator.free_page_count(), free_pages + 3);
        assert!(bytes.iter().all(|&byte| byte == 0), "the page allocator must scrub the freed image");
        {
            let page_table = security_state().lock_page_table();
            let page_table = page_table.as_ref().unwrap();
            for address in image.step_by(PAGE_STEP) {
                assert_eq!(allocator.get_allocation_type(address), None);
                assert_eq!(
                    page_table.query_memory_region(address, UEFI_PAGE_SIZE as u64),
                    Err(patina_paging::PtError::NoMapping)
                );
            }
            let core_base = mapped.core_module.alloc_descriptor.memory_base_address;
            assert_eq!(allocator.get_allocation_type(core_base), Some(AllocationType::Supervisor));
            assert_eq!(
                page_table.query_memory_region(core_base, UEFI_PAGE_SIZE as u64).unwrap(),
                MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly
            );
        }

        assert_eq!(allocator.allocate_pages(3).unwrap(), base);
        mapped.supervisor.free_init_module(&mapped.state);
        assert_eq!(allocator.free_page_count(), free_pages);
        assert_eq!(allocator.get_allocation_type(base), Some(AllocationType::Supervisor));
    }

    #[test]
    fn test_free_init_module_uses_saved_region_after_hobs_are_reclaimed_and_reused() {
        let mapped = MappedInitModule::new();
        let allocator = security_state().page_allocator();
        let producer = allocator.allocate_pages(4).unwrap();
        let mut source = RawHobList::new();
        source.push_struct(mapped.core_module);
        source.push_struct(mapped.init_module);
        source.push_guid_hob(MM_SUPERVISOR_CORE_GUID, &vec![0xcd_u8; 2 * UEFI_PAGE_SIZE]);
        let source = source.finish();
        // SAFETY: the separate producer allocation has room for this valid HOB list.
        let size = unsafe {
            let size = hob::get_pi_hob_list_size(source.as_ptr());
            core::ptr::copy_nonoverlapping(source.as_ptr().cast::<u8>(), producer as *mut u8, size);
            size
        };
        let free_before = allocator.free_page_count();
        // SAFETY: the producer's HOB list and its synthetic image allocations remain live, so the
        // list begins with a Phase Handoff Information Table that outlives the borrow.
        let handoff = unsafe { &*(producer as *const PhaseHandoffInformationTable) };
        mapped
            .supervisor
            .discover_and_store_init_region(handoff, &mapped.state)
            .expect("the producer's HOB list describes an Init module");
        let user_copy =
            mapped.supervisor.publish_hob_list_to_user(handoff).expect("publishing the HOB list copy should succeed");

        let init_base = mapped.init_module.alloc_descriptor.memory_base_address;
        let init_size = mapped.init_module.alloc_descriptor.memory_length;
        assert_eq!(mapped.state.init_module_region(), MemoryRange::new(init_base, init_size).ok());
        assert!(!mapped.state.is_init_module_freed());
        assert_eq!(allocator.get_allocation_type(init_base), Some(AllocationType::Supervisor));
        let reclaimed_pages = size / UEFI_PAGE_SIZE;
        let copy_pages = uefi_size_to_pages!(size);
        assert_eq!(reclaimed_pages, 2);
        assert_eq!(allocator.free_page_count(), free_before - copy_pages + reclaimed_pages);
        for address in (producer..producer + uefi_pages_to_size!(reclaimed_pages) as u64).step_by(UEFI_PAGE_SIZE) {
            assert_eq!(allocator.get_allocation_type(address), None);
            assert_eq!(
                security_state()
                    .lock_page_table()
                    .as_ref()
                    .unwrap()
                    .query_memory_region(address, UEFI_PAGE_SIZE as u64),
                Err(patina_paging::PtError::NoMapping)
            );
        }

        assert_eq!(allocator.allocate_pages(reclaimed_pages).unwrap(), producer);
        // SAFETY: the old HOB pages are now a live, writable allocation used for unrelated data.
        let reused =
            unsafe { core::slice::from_raw_parts_mut(producer as *mut u8, uefi_pages_to_size!(reclaimed_pages)) };
        reused.fill(0xA5);
        mapped.supervisor.free_init_module(&mapped.state);

        assert!(mapped.state.is_init_module_freed());
        assert_eq!(allocator.get_allocation_type(init_base), None);
        assert_eq!(allocator.free_page_count(), free_before - copy_pages + 3);
        assert!(reused.iter().all(|&byte| byte == 0xA5), "runtime must not read or free the old HOB allocation");
        assert_eq!(allocator.get_allocation_type(producer), Some(AllocationType::Supervisor));
        assert_eq!(
            allocator.get_allocation_type(mapped.core_module.alloc_descriptor.memory_base_address),
            Some(AllocationType::Supervisor)
        );
        // SAFETY: the published user copy and `source` are both still readable.
        assert_eq!(unsafe { core::slice::from_raw_parts(user_copy as *const u8, size) }, unsafe {
            core::slice::from_raw_parts(source.as_ptr().cast::<u8>(), size)
        });
        assert_eq!(allocator.get_allocation_type(user_copy), Some(AllocationType::Supervisor));
        assert_eq!(
            security_state()
                .lock_page_table()
                .as_ref()
                .unwrap()
                .query_memory_region(user_copy, uefi_pages_to_size!(copy_pages) as u64),
            Ok(MemoryAttributes::ReadOnly | MemoryAttributes::ExecuteProtect)
        );
    }

    #[test]
    fn test_init_policy_and_validate_reports_a_missing_pass_down_hob() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();

        // The PassDown lookup is the first step, so initialization stops there. Lists that get
        // past this point reach steps that touch real MMRAM, so the later required-HOB lookups are
        // not reachable from a host test.
        let without_pass_down = RawHobList::new().finish();

        let result = supervisor.init_policy_and_validate(without_pass_down.handoff(), 2);
        assert_eq!(result, Err(PassDownHobError::Missing.into()));
        // The core count is recorded before the PassDown lookup.
        assert_eq!(supervisor.number_of_cores(), 2);
    }

    #[test]
    fn test_mp_information_cpu_count_reports_a_missing_hob() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let hob_list = RawHobList::new().finish();

        let result = supervisor.mp_information_cpu_count(hob_list.handoff(), whole_address_space());

        assert_eq!(result, Err(CoreInitError::MpInformationHobMissing.into()));
    }

    #[test]
    fn test_mp_information_cpu_count_requires_the_payload_inside_mmram() {
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let hob_list = policy_hob_list_with_core_count(2, false);

        assert_eq!(supervisor.mp_information_cpu_count(hob_list.handoff(), whole_address_space()), Ok(2));

        // The list is on the host heap, so a one-page span at 0x1000 cannot hold its payload. It is
        // rejected before a byte of it is read.
        let mmram = MemoryRange::new(0x1000, UEFI_PAGE_SIZE as u64).unwrap();
        assert_eq!(
            supervisor.mp_information_cpu_count(hob_list.handoff(), mmram),
            Err(CoreInitError::MpInformationHobOutsideMmram.into())
        );
    }

    /// A span holding any host test buffer, standing in for MMRAM.
    fn whole_address_space() -> MemoryRange {
        MemoryRange::new(0, u64::MAX).unwrap()
    }

    #[test]
    fn test_mp_information_cpu_count_takes_any_nonzero_count_the_payload_holds() {
        let supervisor = MmSupervisorCore::<TestPlatform>::new();

        let empty = policy_hob_list_with_core_count(0, true);
        assert_eq!(
            supervisor.mp_information_cpu_count(empty.handoff(), whole_address_space()),
            Err(CoreInitError::ZeroCoreCount.into())
        );

        // No compile-time capacity caps the count; the payload holds all five processor entries.
        let five = policy_hob_list_with_core_count(5, true);
        assert_eq!(supervisor.mp_information_cpu_count(five.handoff(), whole_address_space()), Ok(5));
    }

    #[test]
    fn test_parse_mp_information_hob() {
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let processor_ids = [0x00_u64, 0x10, 0x20];
        let mut data = [0_u8; 16 + 3 * PROCESSOR_INFO_ENTRY_SIZE];
        data[..8].copy_from_slice(&(processor_ids.len() as u64).to_le_bytes());
        for (cpu_index, processor_id) in processor_ids.iter().enumerate() {
            let offset = 16 + cpu_index * PROCESSOR_INFO_ENTRY_SIZE;
            data[offset..offset + 8].copy_from_slice(&processor_id.to_le_bytes());
        }

        assert_eq!(supervisor.parse_mp_information_hob(&data), Ok(3));
    }

    #[test]
    fn test_parse_mp_information_hob_rejects_invalid_size() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let mut data = [0_u8; 16 + PROCESSOR_INFO_ENTRY_SIZE];

        assert_eq!(
            supervisor.parse_mp_information_hob(&data[..15]),
            Err(CoreInitError::MpInformationHobMalformed.into())
        );

        data[..8].copy_from_slice(&2_u64.to_le_bytes());
        assert_eq!(supervisor.parse_mp_information_hob(&data), Err(CoreInitError::MpInformationHobMalformed.into()));
    }

    #[test]
    fn test_parse_mp_information_hob_rejects_invalid_core_count() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let mut data = [0_u8; 16];

        data[..8].copy_from_slice(&0_u64.to_le_bytes());
        assert_eq!(supervisor.parse_mp_information_hob(&data), Err(CoreInitError::ZeroCoreCount.into()));

        // A count far beyond what the payload holds, including one that overflows the entry size,
        // fails the payload bound.
        for core_count in [5_u64, u64::MAX] {
            data[..8].copy_from_slice(&core_count.to_le_bytes());
            assert_eq!(
                supervisor.parse_mp_information_hob(&data),
                Err(CoreInitError::MpInformationHobMalformed.into())
            );
        }
    }

    #[test]
    fn test_store_mm_initialized_slots_rejects_a_null_buffer() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();

        // SAFETY: a null buffer fails the MMRAM check before anything is read.
        let result = unsafe { supervisor.store_mm_initialized_slots(0, 2) };
        assert_eq!(result, Err(CoreInitError::InitializedBufferInvalid.into()));
        assert!(init_state().mm_initialized_buffer().is_none());
    }

    #[test]
    fn test_store_mm_initialized_slots_rejects_zero_cores() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let memory = PageAlignedMemory::new(1);

        // The count is the length of the slot slice, so it is refused before the buffer is read.
        // SAFETY: the count is rejected before `memory` is dereferenced.
        let result = unsafe { supervisor.store_mm_initialized_slots(memory.base(), 0) };
        assert_eq!(result, Err(CoreInitError::ZeroCoreCount.into()));
        assert!(init_state().mm_initialized_buffer().is_none());
    }

    #[test]
    #[serial]
    fn test_store_mm_initialized_slots_rejects_a_buffer_outside_mmram() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        test_support::init_global_state_over(&memory);

        // Every core writes its own slot, so a buffer the MM IPL placed outside MMRAM would let
        // anything outside MM report a core as initialized.
        let outside = PageAlignedMemory::new(1);
        // SAFETY: `outside` is a live page, and the call refuses it before building a slice.
        let result = unsafe { supervisor.store_mm_initialized_slots(outside.base(), 2) };
        assert_eq!(result, Err(CoreInitError::InitializedBufferInvalid.into()));
        assert!(init_state().mm_initialized_buffer().is_none());
    }

    #[test]
    #[serial]
    fn test_store_mm_initialized_slots_rejects_a_user_mapped_buffer() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        test_support::init_global_state_over(&memory);
        security_state()
            .lock_page_table()
            .as_mut()
            .expect("a page table is installed")
            .map_memory_region(memory.base(), UEFI_PAGE_SIZE as u64, MemoryAttributes::ExecuteProtect)
            .expect("map the buffer for Ring 3");

        // Ring 3 could write a slot it can reach and make a core skip its first entry.
        // SAFETY: `memory` is a live allocation inside MMRAM, and the call refuses it before
        // building a slice.
        let result = unsafe { supervisor.store_mm_initialized_slots(memory.base(), 2) };
        assert_eq!(result, Err(CoreInitError::InitializedBufferInvalid.into()));
        assert!(init_state().mm_initialized_buffer().is_none());
    }

    #[test]
    #[serial]
    fn test_store_mm_initialized_slots_publishes_one_slot_per_cpu() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        test_support::init_global_state_over(&memory);
        test_support::map_supervisor_only(&memory);

        // SAFETY: `memory` is a live allocation inside the MMRAM the allocators were built over,
        // and it outlives the published slice for the rest of this test.
        assert_eq!(unsafe { supervisor.store_mm_initialized_slots(memory.base(), 3) }, Ok(()));

        let slots = init_state().mm_initialized_buffer().expect("the slots were published");
        assert_eq!(slots.len(), 3);
    }

    #[test]
    fn test_free_init_module_rejects_missing_page_table() {
        let mapped = MappedInitModule::new();
        *security_state().lock_page_table() = None;

        mapped.assert_rejected("Page table required to free MM Init module");
    }

    #[test]
    fn test_free_init_module_does_not_mark_failed_free_as_complete() {
        let mapped = MappedInitModule::new();
        let base = mapped.init_module.alloc_descriptor.memory_base_address;
        let size = mapped.init_module.alloc_descriptor.memory_length;
        let allocator = security_state().page_allocator();
        allocator.free_pages(base, 3).unwrap();
        assert_eq!(allocator.allocate_pages_with_type(3, AllocationType::User).unwrap(), base);
        security_state()
            .lock_page_table()
            .as_mut()
            .unwrap()
            .map_memory_region(base, size, MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly)
            .unwrap();

        mapped.assert_rejected("Failed to free MM Init module");
    }

    #[test]
    fn test_parse_mp_information_hob_rejects_a_count_whose_end_offset_overflows() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        // The largest count whose entries still fit a `usize`. Adding the 16-byte header to their
        // size overflows, which must be reported rather than wrap to a small end offset.
        let core_count = usize::MAX / PROCESSOR_INFO_ENTRY_SIZE;
        assert!(core_count * PROCESSOR_INFO_ENTRY_SIZE > usize::MAX - 16, "the end offset must overflow");
        let mut data = [0_u8; 16 + PROCESSOR_INFO_ENTRY_SIZE];
        data[..8].copy_from_slice(&(core_count as u64).to_le_bytes());

        assert_eq!(supervisor.parse_mp_information_hob(&data), Err(CoreInitError::MpInformationHobMalformed.into()));
    }

    #[test]
    #[serial]
    fn test_publish_hob_list_to_user_frees_nothing_for_a_list_inside_one_page() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        init_global_state_over(&memory);

        // A list shorter than a page covers no whole page, so the producer's page stays allocated
        // and only the copy is taken from the pool.
        let staging = security_state().page_allocator().allocate_pages(1).expect("staging page");
        let source = padded_hob_list(0);
        // SAFETY: `source` is a valid HOB list.
        let size = unsafe { hob::get_pi_hob_list_size(source.as_ptr()) };
        assert!(size < UEFI_PAGE_SIZE);
        // SAFETY: source and destination are separate live allocations of at least `size` bytes.
        unsafe { core::ptr::copy_nonoverlapping(source.as_ptr().cast::<u8>(), staging as *mut u8, size) };
        let free_before = security_state().page_allocator().free_page_count();

        // SAFETY: `staging` holds a valid HOB list inside the committed MMRAM region, so it
        // begins with a Phase Handoff Information Table that stays live for the borrow.
        let handoff = unsafe { &*(staging as *const PhaseHandoffInformationTable) };
        let copy = supervisor.publish_hob_list_to_user(handoff).expect("publishing the HOB list copy should succeed");

        assert_eq!(security_state().page_allocator().get_allocation_type(copy), Some(AllocationType::Supervisor));
        assert_eq!(security_state().page_allocator().get_allocation_type(staging), Some(AllocationType::Supervisor));
        assert_eq!(security_state().page_allocator().free_page_count(), free_before - 1);
    }

    #[test]
    fn test_per_core_init_requires_the_smrr_range_from_bsp_init() {
        test_support::init_test_logger();
        let supervisor: &'static MmSupervisorCore<TestPlatform> = Box::leak(Box::new(MmSupervisorCore::new()));

        // Without an MSEG HOB no MSR is written, and without the range `bsp_init` records the SMRR
        // cannot be programmed, so the core stops before touching it.
        assert_eq!(supervisor.per_core_init(1), Err(CoreInitError::SmrrRangeUnavailable.into()));
    }

    #[test]
    fn test_publish_save_state_metadata_stores_the_checked_smbase_array() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let smbases: &'static [u64; 2] = Box::leak(Box::new([0x10_0000, 0x10_2000]));
        // SAFETY: `smbases` is a leaked, readable array of two SMBASE values. Only the logged map
        // addresses are computed from them; no save-state map is read.
        let info = unsafe { SaveStateInfo::new(2, smbases.as_ptr() as u64, 0x100) }.expect("a usable SMBASE array");

        supervisor.publish_save_state_metadata(info);

        let published = security_state().save_state_info().expect("the metadata was published");
        assert_eq!(published.smbase_array(), smbases.as_ptr() as u64);
        assert_eq!(published.number_of_cores(), 2);
        assert_eq!(published.mmi_entry_size(), 0x100);
        assert_eq!(published.smbase(1), Ok(0x10_2000));
    }

    #[test]
    fn test_discover_mseg_base_records_the_hob_base() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let hob_list = policy_hob_list_with_core_count(1, true);

        supervisor.discover_mseg_base(hob_list.handoff());

        assert_eq!(init_state().mseg_base(), Some(0x0040_0000));
    }

    #[test]
    fn test_discover_mseg_base_leaves_the_base_unset_without_a_usable_hob() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();

        // Platforms without an STM publish no HOB at all.
        supervisor.discover_mseg_base(policy_hob_list_with_core_count(1, false).handoff());
        assert_eq!(init_state().mseg_base(), None);

        // A HOB describing an empty region is not recorded either.
        let mut hobs = RawHobList::new();
        hobs.push_guid_hob(MSEG_SMRAM_HOB_GUID, &mseg_smram_hob_data(0x0040_0000, 0x0040_0000, 0));
        supervisor.discover_mseg_base(hobs.finish().handoff());
        assert_eq!(init_state().mseg_base(), None);
    }

    /// A supervisor-mapped buffer outside MMRAM, as the MM IPL provides for a communication
    /// channel: two buffer pages followed by a status page.
    fn external_comm_buffer() -> PageAlignedMemory {
        let external = PageAlignedMemory::new(3);
        map_supervisor_only(&external);
        external
    }

    /// Builds a HOB list carrying the supervisor and user communication buffer HOBs for the
    /// buffers given, either of which may be left out.
    fn comm_buffer_hob_list(supv: Option<&PageAlignedMemory>, user: Option<&PageAlignedMemory>) -> RawHobList {
        let status = |buffer: &PageAlignedMemory| buffer.base() + 2 * UEFI_PAGE_SIZE as u64;
        let mut hobs = RawHobList::new();
        if let Some(supv) = supv {
            hobs.push_guid_hob(MM_COMMON_REGION_HOB_GUID, &supv_comm_buffer_hob_data(supv.base(), 2, status(supv)));
        }
        if let Some(user) = user {
            hobs.push_guid_hob(MM_COMM_BUFFER_HOB_GUID, &user_comm_buffer_hob_data(user.base(), 2, status(user)));
        }
        hobs.finish()
    }

    #[test]
    #[serial]
    fn test_init_comm_buffers_stores_both_channels_and_redirects_the_user_hob() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let mmram = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 16);
        init_global_state_over(&mmram);
        let (supv, user) = (external_comm_buffer(), external_comm_buffer());
        let hob_list = comm_buffer_hob_list(Some(&supv), Some(&user));

        supervisor.init_comm_buffers(hob_list.handoff()).expect("both channels are usable");

        let config = security_state().comm_buffer_config().copied().expect("the configuration was stored");
        assert_eq!(config.supervisor.external_address, supv.base());
        assert_eq!(config.user.external_address, user.base());
        assert_eq!(config.user.status_address, user.base() + 2 * UEFI_PAGE_SIZE as u64);
        assert_eq!(config.supv_to_user_buffer_size, UEFI_PAGE_SIZE as u64);
        // The user core reads the hand-off page, so it must be user-owned.
        assert_eq!(
            security_state().page_allocator().get_allocation_type(config.supv_to_user_buffer),
            Some(AllocationType::User)
        );
        // The user core finds its buffer through the HOB, which now names the internal copy.
        // SAFETY: `hob_list` is a valid HOB list ending in `END_OF_HOB_LIST`.
        let user_hob = unsafe { find_guid_hob_data(hob_list.handoff(), MM_COMM_BUFFER_HOB_GUID) }.expect("user HOB");
        assert_eq!(u64::from_ne_bytes(user_hob[0..8].try_into().expect("eight bytes")), config.user.internal_address);
    }

    #[test]
    fn test_init_comm_buffers_requires_the_supervisor_region_hob() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();

        let result = supervisor.init_comm_buffers(comm_buffer_hob_list(None, None).handoff());

        assert_eq!(result, Err(CommBufferError::CommRegionHobMissing.into()));
        assert!(security_state().comm_buffer_config().is_none());
    }

    #[test]
    #[serial]
    fn test_init_comm_buffers_requires_the_user_buffer_hob() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let mmram = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 16);
        init_global_state_over(&mmram);
        let supv = external_comm_buffer();

        let result = supervisor.init_comm_buffers(comm_buffer_hob_list(Some(&supv), None).handoff());

        assert_eq!(result, Err(CommBufferError::CommunicationBufferHobMissing.into()));
        assert!(security_state().comm_buffer_config().is_none());
    }

    #[test]
    #[serial]
    fn test_init_comm_buffers_reports_a_failed_hand_off_page_allocation() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let mmram = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 16);
        init_global_state_over(&mmram);
        let (supv, user) = (external_comm_buffer(), external_comm_buffer());
        // Leave room for the two internal copies of two pages each, and none for the hand-off page.
        let allocator = security_state().page_allocator();
        allocator.allocate_pages(allocator.free_page_count() - 4).expect("reserve the rest of MMRAM");

        let result = supervisor.init_comm_buffers(comm_buffer_hob_list(Some(&supv), Some(&user)).handoff());

        assert_eq!(result, Err(CommBufferError::AllocationFailed.into()));
        assert_eq!(allocator.free_page_count(), 0, "both internal copies were allocated first");
        assert!(security_state().comm_buffer_config().is_none());
    }

    /// MMRAM holding the buffers the MM IPL passes down for two cores, and the `PassDown` HOB
    /// payload that describes them. The fields keep the memory alive for the test.
    struct PassDownFixture {
        _mmram: PageAlignedMemory,
        _policy: crate::policy::test_support::PolicyBuffer,
        smbase_array: u64,
        pass_down: MmSupvPassDownHobData,
    }

    impl PassDownFixture {
        const CORES: usize = 2;
        const SMBASE_STRIDE: u64 = 0x2000;

        /// Installs MMRAM and its allocators, then carves the per-core initialized slots, the
        /// SMBASE array, and every core's save-state map out of supervisor-owned pages.
        fn new(policy: crate::policy::test_support::PolicyBuffer) -> Self {
            test_support::init_test_logger();
            let mmram = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 64);
            init_global_state_over(&mmram);
            let allocator = security_state().page_allocator();
            let slots = allocator.allocate_pages(1).expect("initialized slot page");
            let smbase_array = allocator.allocate_pages(1).expect("SMBASE array page");
            // Each save-state map ends 0x10000 bytes past its SMBASE.
            let save_state_pages = (0x10000 + Self::SMBASE_STRIDE as usize) / UEFI_PAGE_SIZE;
            let smbase = allocator.allocate_pages(save_state_pages).expect("save-state pages");
            let fixture = Self {
                pass_down: MmSupvPassDownHobData {
                    sm_base_array: smbase_array,
                    mm_initialized_buffer: slots,
                    firmware_policy_buffer: policy.as_ptr() as u64,
                    firmware_policy_buffer_size: policy.len() as u64,
                    ..valid_pass_down_hob()
                },
                _mmram: mmram,
                _policy: policy,
                smbase_array,
            };
            fixture.write_smbases(&[smbase, smbase + Self::SMBASE_STRIDE]);
            fixture
        }

        fn write_smbases(&self, smbases: &[u64; Self::CORES]) {
            // SAFETY: `smbase_array` is a live, page-aligned MMRAM page with room for the array.
            unsafe { (self.smbase_array as *mut [u64; Self::CORES]).write(*smbases) };
        }

        fn smbase(&self, cpu_index: usize) -> u64 {
            // SAFETY: `write_smbases` initialized the array in the live page at `smbase_array`.
            unsafe { (self.smbase_array as *const u64).add(cpu_index).read() }
        }

        fn init_policy_and_validate(&self, supervisor: &MmSupervisorCore<TestPlatform>) -> MmSupervisorResult<()> {
            let mut hobs = RawHobList::new();
            hobs.push_guid_hob(MM_SUPV_PASS_DOWN_HOB_GUID, &pass_down_hob_data(&self.pass_down));
            supervisor.init_policy_and_validate(hobs.finish().handoff(), Self::CORES)
        }
    }

    #[test]
    #[serial]
    fn test_init_policy_and_validate_sets_up_the_pass_down_services_in_order() {
        use crate::policy::validation::PageTableWalkError;

        let fixture = PassDownFixture::new(crate::policy::test_support::full_policy().build());
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let free_before = security_state().page_allocator().free_page_count();

        let result = fixture.init_policy_and_validate(&supervisor);

        // Host tests have no live page table at CR3, so the walk that seeds the unblocked memory
        // tracker is refused. Every step before it must have completed.
        assert_eq!(result, Err(PageTableWalkError::InvalidCr3.into()));
        assert_eq!(init_state().mm_initialized_buffer().map(<[AtomicU8]>::len), Some(PassDownFixture::CORES));
        assert!(security_state().policy_gate().is_some());
        // The policy gate keeps one page for the memory policy snapshot.
        assert_eq!(security_state().page_allocator().free_page_count(), free_before - 1);
        let stack_top = fixture.pass_down.cpl3_stack_base + 2 * fixture.pass_down.cpl3_stack_size - 8;
        assert_eq!(supervisor.syscall_interface.get_cpl3_stack(1), Ok(stack_top));
    }

    #[test]
    #[serial]
    fn test_init_policy_and_validate_rejects_a_truncated_pass_down_hob() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();
        let data = pass_down_hob_data(&valid_pass_down_hob());
        let mut hobs = RawHobList::new();
        hobs.push_guid_hob(MM_SUPV_PASS_DOWN_HOB_GUID, &data[..data.len() - 8]);

        let result = supervisor.init_policy_and_validate(hobs.finish().handoff(), 2);

        // None of the addresses in a malformed payload is used.
        assert_eq!(result, Err(PassDownHobError::TooSmall.into()));
        assert!(init_state().mm_initialized_buffer().is_none());
    }

    #[test]
    #[serial]
    fn test_init_policy_and_validate_rejects_a_misaligned_smbase_array() {
        use crate::privilege_mgmt::save_state::SaveStateValidationError;

        let mut fixture = PassDownFixture::new(crate::policy::test_support::full_policy().build());
        fixture.pass_down.sm_base_array += 4;
        let supervisor = MmSupervisorCore::<TestPlatform>::new();

        let result = fixture.init_policy_and_validate(&supervisor);

        assert_eq!(
            result,
            Err(SaveStateValidationError::UnusableSmBaseArray {
                base: fixture.pass_down.sm_base_array,
                count: PassDownFixture::CORES
            }
            .into())
        );
        assert!(security_state().policy_gate().is_none());
    }

    #[test]
    #[serial]
    fn test_init_policy_and_validate_rejects_a_save_state_map_outside_mmram() {
        use crate::privilege_mgmt::save_state::SaveStateValidationError;

        let fixture = PassDownFixture::new(crate::policy::test_support::full_policy().build());
        // The second SMBASE puts its save-state map far below the MMRAM the host allocated.
        fixture.write_smbases(&[fixture.smbase(0), 0x1000]);
        let supervisor = MmSupervisorCore::<TestPlatform>::new();

        let result = fixture.init_policy_and_validate(&supervisor);

        assert_eq!(
            result,
            Err(SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 1, smbase: 0x1000 }.into())
        );
        assert!(security_state().policy_gate().is_none());
    }

    #[test]
    #[serial]
    fn test_init_policy_and_validate_rejects_an_unsupported_policy_version() {
        let policy = crate::policy::test_support::full_policy().version(2, 0).build();
        let fixture = PassDownFixture::new(policy);
        let supervisor = MmSupervisorCore::<TestPlatform>::new();

        let result = fixture.init_policy_and_validate(&supervisor);

        assert_eq!(result, Err(crate::policy::PolicyGateError::InvalidVersion.into()));
        assert!(security_state().policy_gate().is_none());
        assert!(!supervisor.syscall_interface.is_initialized());
    }

    #[test]
    fn test_install_policy_gate_needs_the_page_allocator_before_reading_the_policy() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform>::new();

        // The snapshot page is allocated first, so a null policy is never read.
        // SAFETY: the allocation fails before `buffer` is dereferenced.
        let result = unsafe { supervisor.install_policy_gate(0, 0x100) };

        assert_eq!(result, Err(memory::AllocError::NotInitialized.into()));
        assert!(security_state().policy_gate().is_none());
    }

    #[test]
    fn test_start_syscall_interface_keeps_the_first_stack_layout() {
        use crate::privilege_mgmt::syscall_setup::SyscallSetupError;

        let supervisor = MmSupervisorCore::<TestPlatform>::new();

        assert_eq!(supervisor.start_syscall_interface(0x10_0000, 0x1000, 2), Ok(()));
        assert_eq!(
            supervisor.start_syscall_interface(0x20_0000, 0x2000, 4),
            Err(SyscallSetupError::AlreadyInitialized.into())
        );
        // Only two cores were configured, and the stacks keep the first base and size.
        assert_eq!(supervisor.syscall_interface.get_cpl3_stack(1), Ok(0x10_2000 - 8));
        assert_eq!(supervisor.syscall_interface.get_cpl3_stack(2), Err(SyscallSetupError::InvalidCpuIndex));
    }
}
