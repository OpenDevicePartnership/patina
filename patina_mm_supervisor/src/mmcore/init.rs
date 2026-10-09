//! `MmSupervisorCore` Initialization Phase
//!
//! The one-time setup every core runs on its first entry into MM. The BSP additionally performs
//! the system-wide work in [`bsp_init`](MmSupervisorCore::bsp_init): validating the incoming HOB
//! list, programming the SMRRs, committing the allocators and page table, and publishing a
//! read-only copy of the HOB list for the demoted user core.
//!
//! Sits beside the other two phases in `mmcore`. The steps here sequence the work; what each
//! step reads or programs stays with the module that owns it. The HOB payloads and the parsing
//! behind them are in [`hob`](crate::hob), [`comm_buffer`](crate::comm_buffer) and
//! [`mmram`](crate::memory::mmram), and the hardware a core programs on its way up is in
//! [`smrr`](super::smrr), [`mseg`](super::mseg) and
//! [`smi_idt_patch`](super::smi_idt_patch).
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{ffi::c_void, sync::atomic::AtomicU8};

use patina::{
    UEFI_PAGE_SIZE, align_range,
    management_mode::{
        comm_buffer_hob::MM_COMM_BUFFER_HOB_GUID,
        supervisor::{MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, MM_SUPERVISOR_USER_GUID},
    },
    pi::{
        guid::HOB_MEMORY_ALLOC_MODULE_GUID,
        hob::{self, Hob, PhaseHandoffInformationTable},
    },
};
use patina_internal_cpu::{interrupts::Interrupts, save_state::PROCESSOR_INFO_ENTRY_SIZE};
use patina_paging::{MemoryAttributes, PageTable, PagingType, x64::X64PageTable};

use crate::{
    MmSupervisorCore, PlatformInfo,
    comm_buffer::{CommBufferConfig, CommBufferError, CommChannel, MM_COMMON_REGION_HOB_GUID},
    error::MmSupervisorResult,
    hob::validation::{self, HobValidationError},
    intrinsics::read_cr3,
    memory::{
        self, AllocationType, PageAllocator, SharedPagingAllocator,
        mmram::MmramRegion,
        mmram::{classify_mmram_in_regions, is_buffer_inside_mmram},
        page_ownership::{PageOwnership, query_address_ownership},
    },
    policy::{self, MemDescriptorV1_0, dump_policy, gate::PolicyGate, walk_page_table},
    state::{
        init_state,
        save_state::{SaveStateInfo, validate_save_state_regions},
        security_state,
    },
    user_access_guard::with_user_access,
};

use super::CoreInitError;
use super::mseg::{MSEG_SMRAM_HOB_GUID, parse_mseg_smram_hob};
use super::smi_idt_patch::patch_smi_handler_idt;
use super::smrr::{coalesced_smrr_range, configure_smm_code_access, smrr_initialize};
use crate::hob::lookup::{find_guid_hob, find_module};
use crate::hob::pass_down::{MM_SUPV_PASS_DOWN_HOB_GUID, MmSupvPassDownHobData, PassDownHobError};
use crate::memory::mmram::{establish_mmram_bound, supervisor_image_anchor};

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
///
/// # Panics
///
/// Panics when an executable page is not supervisor-only and read-only, or is read-protected.
/// Freeing a page the supervisor cannot account for would return memory of unknown provenance to
/// the allocator, so this fails closed.
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
    /// BSP-specific initialization.
    ///
    /// This is called only on the BSP after basic setup is complete. It parses and validates the
    /// incoming HOB list, programs the SMRR, initializes the page and paging allocators and the
    /// global page table, discovers the user module entry point, initializes the security policy,
    /// and remaps the HOB list so the demoted user core can read it.
    ///
    /// Returns the address of the read-only HOB list copy to hand the user core.
    ///
    /// # Errors
    ///
    /// Reports the first stage that fails, so the caller stops before anything further is
    /// programmed. The HOB list is rejected through [`HobValidationError`], the SMRRs through
    /// [`SmrrError`](super::smrr::SmrrError), the allocators through
    /// [`AllocError`](crate::memory::AllocError), and the module discovery and policy
    /// setup through [`CoreInitError`] and [`PassDownHobError`].
    ///
    /// The MM IPL describes MMRAM and sits outside the supervisor's trust boundary, and the
    /// platform leaves the SMRRs unprogrammed at entry, so no hardware bound is available to check
    /// its descriptors against. Ordering carries the weight instead: the descriptors are parsed
    /// into stack metadata, anchored to the supervisor's own image, validated, and used to program
    /// the SMRR, and only then is anything written into the memory they name. The extent
    /// of MMRAM still originates with the MM IPL, which remains a platform requirement rather than
    /// something the supervisor can verify.
    pub(crate) fn bsp_init(
        &'static self,
        hob_hand_off_table: &PhaseHandoffInformationTable,
    ) -> MmSupervisorResult<u64> {
        log::info!("BSP performing one-time initialization...");

        Interrupts::new().initialize().map_err(CoreInitError::InterruptManagerInit)?;

        // Parse the producer's SMRAM descriptors into stack metadata. Nothing in MMRAM is written
        // until they have been anchored and validated below.
        // SAFETY: `hob_list` is provided by the MM IPL and is guaranteed to be a
        // valid HOB list (the caller asserts it is non-null before dispatching).
        let (scanned_regions, region_count) = unsafe { PageAllocator::scan_hob_list(hob_hand_off_table)? };

        let scanned_regions = scanned_regions.get(..region_count).unwrap_or(&scanned_regions);

        let smrr_range = establish_mmram_bound(scanned_regions, supervisor_image_anchor(), coalesced_smrr_range)?;

        validation::validate_incoming_hobs_pre_paging_init(hob_hand_off_table, scanned_regions, |base, size| {
            classify_mmram_in_regions(scanned_regions, base, size).is_inside(base, size)
        })?;

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

        self.init_page_table();

        // Validate the incoming HOBs that require an active page table, now
        // that it is available (the remaining checks that only need the page
        // allocator ran above).
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
    ///
    /// # Errors
    ///
    /// Returns an [`AllocError`](crate::memory::AllocError) when the regions cannot back the page
    /// allocator, when the paging
    /// pool cannot be reserved out of them, or when the paging allocator is already initialized.
    /// The page allocator keeps whatever state it reached, so a failed call leaves the paging
    /// allocator uninitialized rather than half configured.
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
    fn init_page_table(&self) {
        let cr3 = read_cr3();
        let paging_alloc = SharedPagingAllocator::new(security_state().paging_allocator());
        // SAFETY: `cr3` is read from the active control register, so it points to
        // the valid page table hierarchy currently in use by this core, and
        // `paging_alloc` owns the pool from which new paging structures are drawn.
        let page_table = unsafe { X64PageTable::from_existing(cr3, paging_alloc, PagingType::Paging4Level) }
            .expect("Failed to create page table from active CR3");
        *security_state().lock_page_table() = Some(page_table);
        log::info!("Page table initialized from CR3=0x{cr3:016x}");
    }

    /// Discovers the MM Supervisor User module entry point from the HOB list and
    /// stores it for use during request processing.
    ///
    /// We look for `EFI_HOB_TYPE_MEMORY_ALLOCATION` HOBs whose
    /// `MemoryAllocationHeader.Name` is `gMmSupervisorHobMemoryAllocModuleGuid`
    /// and whose `ModuleName` is `gMmSupervisorUserGuid`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreInitError::UserEntryPointMissing`] when the HOB list describes no such
    /// module. The stored entry point is left unset, so no later stage can demote to Ring 3.
    fn discover_and_store_user_entry(
        &self,
        hob_hand_off_table: &PhaseHandoffInformationTable,
        state: &crate::state::InitState,
    ) -> MmSupervisorResult<()> {
        let module = find_module(
            &Hob::Handoff(hob_hand_off_table),
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_USER_GUID,
        );

        if let Some(module) = module {
            let entry = module.entry_point;
            log::info!("Discovered MM User module entry point: 0x{entry:016x}");
            state.set_user_entry_point(entry);
        } else {
            return Err(CoreInitError::UserEntryPointMissing.into());
        }

        Ok(())
    }

    /// Saves the validated Init image allocation before the producer's HOBs are reclaimed.
    ///
    /// We look for `EFI_HOB_TYPE_MEMORY_ALLOCATION` HOBs whose
    /// `MemoryAllocationHeader.Name` is `gEfiHobMemoryAllocModuleGuid`
    /// and whose `ModuleName` is `gMmSupervisorInitGuid`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreInitError::InitModuleRegionMissing`] when the HOB list describes no Init
    /// module. Validation already rejects such a list, so reaching this means discovery ran
    /// against a list that was never accepted.
    pub(crate) fn discover_and_store_init_region(
        &self,
        hob_hand_off_table: &PhaseHandoffInformationTable,
        state: &crate::state::InitState,
    ) -> MmSupervisorResult<()> {
        let Some(module) =
            find_module(&Hob::Handoff(hob_hand_off_table), HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID)
        else {
            return Err(CoreInitError::InitModuleRegionMissing.into());
        };

        state
            .set_init_module_region(module.alloc_descriptor.memory_base_address, module.alloc_descriptor.memory_length);
        Ok(())
    }

    /// Frees the saved Init allocation on the first runtime SMI, without accessing any HOBs.
    ///
    /// Called only on the BSP after initialization has returned.
    pub(crate) fn free_init_module(&self, state: &crate::state::InitState) {
        if state.is_init_module_freed() {
            return;
        }
        let Some((base, size)) = state.init_module_region() else {
            log::warn!("MM Init module region was not discovered during initialization");
            return;
        };
        assert!(is_buffer_inside_mmram(base, size), "MM Init module is outside MMRAM");

        // Wrap in a block to drop the lock automatically
        {
            let page_table = security_state().lock_page_table();
            let page_table = page_table.as_ref().expect("Page table required to validate MM Init module");
            let end = base.checked_add(size).expect("MM Init module allocation overflows");
            for address in (base..end).step_by(UEFI_PAGE_SIZE) {
                let attributes = page_table
                    .query_memory_region(address, UEFI_PAGE_SIZE as u64)
                    .expect("Failed to query MM Init module page");
                validate_init_code_page(address, attributes);
            }
        }

        security_state()
            .page_allocator()
            .free_pages_checked(base, size as usize / UEFI_PAGE_SIZE, AllocationType::Supervisor)
            .expect("Failed to free MM Init module");
        state.mark_init_module_freed();
        log::info!("Freed MM Init module at 0x{base:016x} (0x{size:x} bytes)");
    }

    /// Initializes the policy gate from the `PassDown` HOB and runs an initial
    /// security validation.
    ///
    /// # Errors
    ///
    /// Reports a [`PassDownHobError`] when the `PassDown` HOB cannot be parsed, a
    /// [`PolicyGateError`](crate::policy::gate::PolicyGateError) when the policy blob cannot
    /// be read, and a [`PolicyValidationError`](policy::validation::PolicyValidationError) when
    /// the blob is rejected. All were fatal before and still stop initialization; the caller now
    /// decides how to fail instead of this function panicking.
    fn init_policy_and_validate(&self, hob_hand_off_table: &PhaseHandoffInformationTable) -> MmSupervisorResult<()> {
        self.init_policy_from_hob_list(hob_hand_off_table)?;

        let gate =
            security_state().policy_gate().expect("Policy gate must be initialized before policy validation runs");
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
    ///
    /// # Errors
    ///
    /// Returns [`HobValidationError::HobListOutsideMmram`] when the walked list is empty or does
    /// not lie entirely inside MMRAM, before any of its bytes are read.
    ///
    /// Returns a [`CoreInitError`] when the copy cannot be made: the list is too long to address
    /// ([`HobListTooLargeToCopy`](CoreInitError::HobListTooLargeToCopy)), no page table is
    /// installed ([`PageTableUnavailable`](CoreInitError::PageTableUnavailable)), or the copy
    /// could not be mapped read-only
    /// ([`HobListCopyNotMapped`](CoreInitError::HobListCopyNotMapped)). An allocation failure is
    /// reported as the allocator's own [`AllocError`](crate::memory::AllocError).
    ///
    /// Every one of these stops the BSP before it demotes, so Ring 3 never receives an address
    /// for a copy that was not mapped read-only.
    fn publish_hob_list_to_user(&self, hob_hand_off_table: &PhaseHandoffInformationTable) -> MmSupervisorResult<u64> {
        // We need to convert back to *const c_void because
        // `get_pi_hob_list_size()` do not accept a reference directly.
        let hob_list: *const c_void = core::ptr::from_ref(hob_hand_off_table).cast();
        let hob_base = hob_list as u64;
        // SAFETY: `hob_list` is a valid HOB list per this function's contract.
        let hob_list_size = unsafe { hob::get_pi_hob_list_size(hob_list) } as u64;

        // A zero-length walk means the list has no HOBs at all, which is not a list. The producer
        // named this range, so it is confirmed to be MMRAM before it is read.
        if hob_list_size == 0 || !is_buffer_inside_mmram(hob_base, hob_list_size) {
            return Err(HobValidationError::HobListOutsideMmram { base: hob_base, size: hob_list_size }.into());
        }

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

        let attrs = MemoryAttributes::ReadOnly | MemoryAttributes::ExecuteProtect;
        {
            let mut pt_guard = security_state().lock_page_table();
            let pt = pt_guard.as_mut().ok_or(CoreInitError::PageTableUnavailable)?;
            pt.map_memory_region(copy_base, copy_size as u64, attrs).map_err(|e| {
                log::error!("Failed to map the user HOB list copy at 0x{copy_base:016x}: {e:?}");
                CoreInitError::HobListCopyNotMapped { base: copy_base, size: copy_size as u64 }
            })?;
        }

        let copy_end = copy_base + copy_size as u64;
        log::info!(
            "Published HOB list copy of 0x{hob_list_size:x} bytes from 0x{hob_base:016x} at \
             0x{copy_base:016x}-0x{copy_end:016x} as user read-only"
        );

        // The copy is the only HOB list anything uses from here, so the original's pages go back
        // to the pool, scrubbed and unmapped. Only pages lying wholly inside the list are
        // released: rounding outward would hand the pool the trailing slack this copy exists to
        // keep out of Ring 3's reach. The page table lock is released above because freeing takes
        // it again, and it is not reentrant.
        let page = UEFI_PAGE_SIZE as u64;
        let first_page = hob_base.div_ceil(page) * page;
        let last_page = (hob_base + hob_list_size) / page * page;
        if let Some(pages) = last_page.checked_sub(first_page).map(|bytes| (bytes / page) as usize).filter(|p| *p != 0)
        {
            match security_state().page_allocator().free_pages_checked(first_page, pages, AllocationType::Supervisor) {
                Ok(()) => log::info!(
                    "Reclaimed {pages} page(s) of the producer's HOB list at 0x{first_page:016x}-0x{last_page:016x}"
                ),
                // Reclaiming is best-effort, but a failure means the range is not what the
                // descriptors said it was, which is worth saying out loud.
                Err(e) => log::error!("Failed to reclaim the producer's HOB list at 0x{first_page:016x}: {e:?}"),
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
    ///
    /// # Errors
    ///
    /// Returns [`Cpl3StackRegionInvalid`](CoreInitError::Cpl3StackRegionInvalid) when the HOB
    /// describes no usable region, and
    /// [`Cpl3StackRegionOutsideMmram`](CoreInitError::Cpl3StackRegionOutsideMmram) when the range
    /// is not MMRAM, before anything is mapped. A failure to map, or stacks that do not verify as
    /// user-accessible afterwards, report
    /// [`Cpl3StacksNotMapped`](CoreInitError::Cpl3StacksNotMapped).
    fn map_cpl3_stacks_to_user(&self, base: u64, per_core_size: u64, num_cpus: usize) -> MmSupervisorResult<()> {
        let invalid_region = || CoreInitError::Cpl3StackRegionInvalid { base, per_core_size, num_cpus };

        if base == 0 || per_core_size == 0 {
            return Err(invalid_region().into());
        }

        let total_size = per_core_size.checked_mul(num_cpus as u64).ok_or_else(invalid_region)?;
        let (aligned_base, aligned_size) =
            align_range(base, total_size, UEFI_PAGE_SIZE as u64).map_err(|_| invalid_region())?;
        let aligned_end = aligned_base + aligned_size;

        // The region is described by the untrusted producer, so nothing outside MMRAM may be
        // handed to Ring 3.
        if !is_buffer_inside_mmram(aligned_base, aligned_size) {
            return Err(CoreInitError::Cpl3StackRegionOutsideMmram { base: aligned_base, size: aligned_size }.into());
        }

        // Scoped so the page table lock is released before the verification below retakes it.
        {
            let attrs = MemoryAttributes::ExecuteProtect;
            let mut pt_guard = security_state().lock_page_table();
            let pt = pt_guard.as_mut().ok_or(CoreInitError::PageTableUnavailable)?;

            pt.map_memory_region(aligned_base, aligned_size, attrs).map_err(|e| {
                log::error!("Failed to map CPL3 stacks to user level at 0x{aligned_base:016x}: {e:?}");
                CoreInitError::Cpl3StacksNotMapped { base: aligned_base, size: aligned_size }
            })?;
        }

        validation::verify_cpl3_stacks_user_accessible(aligned_base, aligned_size).map_err(|e| {
            log::error!("CPL3 stacks at 0x{aligned_base:016x} are not usable by Ring 3 after remapping: {e}");
            CoreInitError::Cpl3StacksNotMapped { base: aligned_base, size: aligned_size }
        })?;

        log::info!("Mapped CPL3 stacks 0x{aligned_base:016x}-0x{aligned_end:016x} as user read/write, NX");
        Ok(())
    }

    /// Per-core initialization.
    ///
    /// This is called on every core (BSP and APs) during the first entry.
    /// Use this for setting up per-CPU state like syscall MSRs, GS base, etc.
    ///
    /// # Errors
    ///
    /// Returns an [`SmrrError`](super::smrr::SmrrError) when this processor's SMRRs cannot be
    /// programmed. The range is
    /// per-logical-processor, so one core failing does not undo the cores that already succeeded.
    ///
    /// # Panics
    ///
    /// Panics if the SMRR range was not determined during BSP initialization, or if the CPU does
    /// not report SMM Code Access Check support.
    pub(crate) fn per_core_init(&'static self, cpu_id: u32, is_bsp: bool) -> MmSupervisorResult<()> {
        let core_type = if is_bsp { "BSP" } else { "AP" };
        log::trace!("{core_type} (CPU {cpu_id}) performing per-core initialization...");

        // IA32_SMM_MONITOR_CTL is per-logical-processor, so every core programs it.
        super::mseg::program_mseg_base(cpu_id);

        // SMRR is per-logical-processor. The APs program theirs here; the BSP's was done in `bsp_init`.
        let range =
            init_state().smrr_range().expect("SMRR range must be determined during BSP init before per-core init");
        smrr_initialize(range)?;
        configure_smm_code_access();

        log::trace!("{core_type} (CPU {cpu_id}) per-core initialization complete.");
        Ok(())
    }

    /// Publishes the save-state metadata the read syscall depends on.
    ///
    /// ## Safety
    ///
    /// `sm_base` must reference `number_of_cpus` resident SMBASE entries in MMRAM.
    unsafe fn publish_save_state_metadata(&self, number_of_cpus: usize, sm_base: u64) {
        let save_state_info = SaveStateInfo { number_of_cpus, sm_base };
        security_state().set_save_state_info(save_state_info);
        // SAFETY: this function's contract gives `sm_base` `number_of_cpus` readable entries.
        unsafe { crate::state::save_state::log_save_state_map(save_state_info) };
        log::info!("Save-state metadata initialized for {number_of_cpus} CPU(s) from SMBASE array at 0x{sm_base:016x}");
    }

    /// Records the MSEG region reserved for an STM, when the platform published one.
    ///
    /// Each core programs the base into `IA32_SMM_MONITOR_CTL` during per-core init. Platforms
    /// without STM or SEA integration do not publish the HOB, so its absence is not an error.
    fn discover_mseg_base(&self, hob_hand_off_table: &PhaseHandoffInformationTable) {
        match find_guid_hob(hob_hand_off_table, MSEG_SMRAM_HOB_GUID).and_then(parse_mseg_smram_hob) {
            Some(mseg_base) => {
                init_state().set_mseg_base(mseg_base);
                log::info!("MSEG base 0x{mseg_base:x} discovered from MSEG SMRAM HOB");
            }
            None => log::warn!("No usable MSEG SMRAM HOB; IA32_SMM_MONITOR_CTL will not be programmed"),
        }
    }

    /// Builds both communication channels and stores the assembled configuration.
    ///
    /// # Errors
    ///
    /// Returns a [`CommBufferError`] when either HOB is missing, when a region one of them
    /// describes cannot be used, or when the supervisor-to-user page cannot be allocated.
    fn init_comm_buffers(&self, hob_hand_off_table: &PhaseHandoffInformationTable) -> MmSupervisorResult<()> {
        // Only one MM_COMM_REGION_HOB is published, the supervisor one; the user channel flows
        // through MM_COMM_BUFFER_HOB_GUID below.
        let supv_region_data = find_guid_hob(hob_hand_off_table, MM_COMMON_REGION_HOB_GUID)
            .ok_or(CommBufferError::CommRegionHobMissing)?;
        let supv = CommChannel::from_supv_hob(supv_region_data)
            .inspect_err(|e| log::error!("Failed to initialize supervisor communication buffer: {e}"))?;

        // The user channel still uses the legacy `MM_COMM_BUFFER_HOB_GUID` so the user core's own
        // HOB walk keeps working (see the HACKHACK at the tail of `CommChannel::from_user_hob`).
        //
        // The lookup is scoped so the shared slice it returns is dead before the call below
        // rewrites the same bytes. Taking the address is the only way to hand over write access:
        // the HOB list is walked through shared references, and `&[u8]` cannot become `&mut [u8]`.
        let (user_buffer_data, user_buffer_data_len) = {
            let data = find_guid_hob(hob_hand_off_table, MM_COMM_BUFFER_HOB_GUID)
                .ok_or(CommBufferError::CommunicationBufferHobMissing)?;
            (data.as_ptr().cast_mut(), data.len())
        };
        // SAFETY: the pointer and length identify the original HOB payload in the writable live
        // HOB list. The shared slice used to locate it is no longer used while it is rewritten.
        let user = unsafe {
            CommChannel::from_user_hob(user_buffer_data, user_buffer_data_len)
                .inspect_err(|e| log::error!("Failed to initialize user communication buffer: {e}"))?
        };

        let supv_to_user_buffer =
            security_state().page_allocator().allocate_pages_with_type(1, AllocationType::User).map_err(|e| {
                log::error!("Failed to allocate page for supervisor-to-user buffer: {e}");
                CommBufferError::AllocationFailed
            })?;

        if supv.external == 0 || user.external == 0 || user.status == 0 || supv.status == 0 || supv_to_user_buffer == 0
        {
            log::error!("One or more communication buffers are not properly initialized");
            return Err(CommBufferError::Missing.into());
        }

        security_state().set_comm_buffer_config(CommBufferConfig {
            supervisor: supv,
            user,
            supv_to_user_buffer,
            supv_to_user_buffer_size: UEFI_PAGE_SIZE as u64,
        });
        log::info!(
            "Comm buffers: supv=0x{:x}/0x{:x} size=0x{:x} status=0x{:x}, user=0x{:x}/0x{:x} size=0x{:x} status=0x{:x}",
            supv.external,
            supv.internal,
            supv.size,
            supv.status,
            user.external,
            user.internal,
            user.size,
            user.status
        );

        Ok(())
    }

    /// Initializes the services the HOB list describes.
    ///
    /// Reads the MP Information HOB for the CPU count, which sizes everything that follows, then
    /// the `PassDown` HOB for the policy gate, the syscall interface and the memory policy. It
    /// then publishes the save-state metadata, records the optional MSEG region, patches every
    /// core's SMI-handler IDT descriptor, and assembles the communication buffers.
    ///
    /// # Errors
    ///
    /// Reports the first stage that fails. A missing MP Information HOB and an unusable CPU count
    /// are reported through [`CoreInitError`], a missing or malformed `PassDown` HOB through
    /// [`PassDownHobError`], and the communication buffers through [`CommBufferError`]. A missing
    /// MSEG HOB is not an error, because platforms without STM integration do not publish one.
    fn init_policy_from_hob_list(&self, hob_hand_off_table: &PhaseHandoffInformationTable) -> MmSupervisorResult<()> {
        // The CPU count sizes the Ring 3 stack array the PassDown HOB describes, so it is read
        // before anything that depends on it.
        let mp_information =
            find_guid_hob(hob_hand_off_table, MP_INFORMATION_HOB_GUID).ok_or(CoreInitError::MpInformationHobMissing)?;
        let number_of_cpus = self.parse_mp_information_hob(mp_information)?;

        let pass_down_data =
            find_guid_hob(hob_hand_off_table, MM_SUPV_PASS_DOWN_HOB_GUID).ok_or(PassDownHobError::Missing)?;
        // SAFETY: `pass_down_data` is a slice into the validated HOB list, so the buffer pointers
        // it carries reference live memory as `init_from_pass_down_hob` requires.
        let (sm_base, mmi_entry_size) = unsafe { self.init_from_pass_down_hob(pass_down_data, number_of_cpus) }?;

        // SAFETY: `sm_base` came from the PassDown HOB the MM IPL published, so it references
        // `number_of_cpus` resident SMBASE entries in MMRAM.
        unsafe { self.publish_save_state_metadata(number_of_cpus, sm_base) };

        self.discover_mseg_base(hob_hand_off_table);

        // The SMI entry blocks were already copied per SMBASE, so every core is patched here,
        // not just the BSP.
        patch_smi_handler_idt(sm_base, number_of_cpus, mmi_entry_size);

        self.init_comm_buffers(hob_hand_off_table)
    }

    /// Returns the CPU count from the MP Information HOB.
    ///
    /// The count is validated against `MAX_CPUS` here and returned as a `usize`, so callers
    /// index and size allocations with it directly. [`CoreInitError::InvalidCpuCount`] keeps the
    /// raw `u64` from the HOB, because the value it reports may be one that does not fit a
    /// `usize`.
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
    /// A null buffer is not an error. The MM IPL may omit it, and the supervisor then runs
    /// without per-core initialized tracking.
    ///
    /// # Errors
    ///
    /// Returns [`CoreInitError::InvalidCpuCount`] when the count cannot size the slot array, and
    /// [`CoreInitError::InitializedBufferInvalid`] when the buffer does not hold one slot per CPU
    /// inside MMRAM.
    ///
    /// ## Safety
    ///
    /// `buffer` must name `number_of_cpus` bytes that stay resident for the supervisor's
    /// lifetime.
    unsafe fn store_mm_initialized_slots(&self, buffer: u64, number_of_cpus: usize) -> MmSupervisorResult<()> {
        if buffer == 0 {
            log::warn!("MM Initialized buffer is null in PassDown HOB");
            return Ok(());
        }

        // The slice built below is `number_of_cpus` elements long, so the bound is a
        // memory-safety precondition and is checked here rather than assumed of the caller.
        if number_of_cpus == 0 || number_of_cpus > MAX_CPUS {
            return Err(CoreInitError::InvalidCpuCount { found: number_of_cpus, maximum: MAX_CPUS }.into());
        }
        if !is_buffer_inside_mmram(buffer, number_of_cpus as u64) {
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
    ///
    /// # Errors
    ///
    /// Returns a [`SaveStateValidationError`](crate::state::save_state::SaveStateValidationError)
    /// naming the first region the `PassDown` HOB describes that cannot be used.
    fn check_save_state_regions(&self, sm_base: u64, number_of_cpus: usize) -> MmSupervisorResult<()> {
        validate_save_state_regions(sm_base, number_of_cpus, is_buffer_inside_mmram, |base, size| {
            matches!(query_address_ownership(base, size), Some(PageOwnership::Supervisor))
        })
        .inspect_err(|e| log::error!("PassDown HOB does not describe usable save-state regions: {e}"))?;
        log::info!("Validated save-state regions for {number_of_cpus} CPU(s) from SMBASE array at 0x{sm_base:016x}");

        Ok(())
    }

    /// Builds the policy gate over the firmware policy blob and installs it.
    ///
    /// Returns the page reserved for the memory policy descriptors, which the gate keeps as its
    /// snapshot buffer and [`record_unblocked_memory`](Self::record_unblocked_memory) fills.
    ///
    /// # Errors
    ///
    /// Returns an [`AllocError`](crate::memory::AllocError) when the descriptor page cannot be
    /// allocated, [`PassDownHobError::FirmwarePolicyBufferSizeUnsupported`] when the reported
    /// size does not fit the target architecture, and a
    /// [`PolicyGateError`](crate::policy::gate::PolicyGateError) when the blob's layout
    /// is rejected.
    ///
    /// ## Safety
    ///
    /// `buffer` must name `size` readable bytes that stay resident for the supervisor's lifetime.
    unsafe fn install_policy_gate(&self, buffer: u64, size: u64) -> MmSupervisorResult<u64> {
        let memory_policy_buffer = security_state()
            .page_allocator()
            .allocate_pages(1)
            .inspect_err(|e| log::error!("Failed to allocate page for memory policy buffer: {e}"))?;

        let policy_ptr = buffer as *const u8;
        let policy_buffer_size =
            usize::try_from(size).map_err(|_| PassDownHobError::FirmwarePolicyBufferSizeUnsupported { size })?;

        // SAFETY: this function's contract requires `buffer` to be readable for `size` bytes and
        // to stay resident. The HOB's reported size bounds the blob's own internal offsets.
        let mut gate = unsafe { PolicyGate::new(policy_ptr, policy_buffer_size) }
            .inspect_err(|e| log::error!("Failed to create policy gate: {e}"))?;
        log::info!("Policy gate initialized successfully");
        // SAFETY: `policy_ptr` is the same valid, resident firmware policy buffer.
        unsafe { dump_policy(policy_ptr) };

        policy::audit_boundary_grants(&gate);

        gate.set_memory_policy_buffer(
            memory_policy_buffer as *mut MemDescriptorV1_0,
            MEMORY_POLICY_DESCRIPTORS_PER_PAGE,
        );
        security_state().set_policy_gate(gate);

        Ok(memory_policy_buffer)
    }

    /// Starts the syscall interface over the Ring 3 stacks the MM IPL provisioned.
    ///
    /// # Errors
    ///
    /// Returns [`Cpl3StackRegionInvalid`](CoreInitError::Cpl3StackRegionInvalid) when the stack
    /// size does not fit the target architecture, and a
    /// [`SyscallSetupError`](crate::privilege_mgmt::syscall_setup::SyscallSetupError) when the
    /// interface rejects the region. Either means the supervisor has nowhere to demote to.
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
    /// # Errors
    ///
    /// Returns a [`PageTableWalkError`](crate::policy::validation::PageTableWalkError)
    /// when the walk cannot describe the mapped memory, and an
    /// [`UnblockError`](crate::handlers::supv_request::unblock_memory::UnblockError)
    /// when the tracker rejects the descriptors. Both leave the tracker without the regions every
    /// later `UNBLOCK_MEM` request is checked against, so neither is survivable.
    ///
    /// ## Safety
    ///
    /// `memory_policy_buffer` must name a page with room for
    /// [`MEMORY_POLICY_DESCRIPTORS_PER_PAGE`] descriptors that stays resident.
    unsafe fn record_unblocked_memory(&self, memory_policy_buffer: u64) -> MmSupervisorResult<()> {
        let descriptors = memory_policy_buffer as *mut MemDescriptorV1_0;
        let cr3 = read_cr3();

        // SAFETY: `cr3` is read from the active control register, so it points to the live PML4
        // table, and this function's contract gives `descriptors` room for
        // `MEMORY_POLICY_DESCRIPTORS_PER_PAGE` entries.
        let descriptor_count =
            unsafe { walk_page_table(cr3, descriptors, MEMORY_POLICY_DESCRIPTORS_PER_PAGE, is_buffer_inside_mmram) }?;
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
    /// Returns `(sm_base_array, mmi_entry_size)` carried by the HOB.
    ///
    /// # Errors
    ///
    /// Reports the first stage that fails, so the caller stops before anything further is
    /// programmed. The payload itself is rejected through [`PassDownHobError`], the slot array,
    /// the CPU count and the Ring 3 stack region through [`CoreInitError`], the save-state
    /// regions through [`SaveStateValidationError`](crate::state::save_state::SaveStateValidationError),
    /// the policy blob through [`PolicyGateError`](crate::policy::gate::PolicyGateError),
    /// the syscall interface through
    /// [`SyscallSetupError`](crate::privilege_mgmt::syscall_setup::SyscallSetupError), and the
    /// page-table walk that seeds the unblocked memory tracker through
    /// [`PageTableWalkError`](crate::policy::validation::PageTableWalkError).
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
    ) -> MmSupervisorResult<(u64, u64)> {
        let MmSupvPassDownHobData {
            cpl3_stack_base,
            cpl3_stack_size,
            sm_base,
            mm_initialized_buffer,
            firmware_policy_buffer,
            firmware_policy_buffer_size,
            mmi_entry_size,
            ..
        } = MmSupvPassDownHobData::parse(data)?;

        // SAFETY: this function's contract requires the pointers the HOB carries to name resident
        // memory of the size they declare.
        unsafe { self.store_mm_initialized_slots(mm_initialized_buffer, number_of_cpus) }?;

        self.check_save_state_regions(sm_base, number_of_cpus)?;

        // SAFETY: as above, for the firmware policy buffer and the size reported beside it.
        let memory_policy_buffer =
            unsafe { self.install_policy_gate(firmware_policy_buffer, firmware_policy_buffer_size) }?;

        self.start_syscall_interface(cpl3_stack_base, cpl3_stack_size, number_of_cpus)?;
        self.map_cpl3_stacks_to_user(cpl3_stack_base, cpl3_stack_size, number_of_cpus)?;

        // SAFETY: `install_policy_gate` returned a freshly allocated page, which holds
        // `MEMORY_POLICY_DESCRIPTORS_PER_PAGE` descriptors and stays resident.
        unsafe { self.record_unblocked_memory(memory_policy_buffer) }?;

        Ok((sm_base, mmi_entry_size))
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
        memory::{PageAllocator, PagingPoolAllocator},
        state::InitState,
    };
    use patina::management_mode::supervisor::{MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_USER_GUID};
    use patina::{management_mode::supervisor::MM_SUPERVISOR_CORE_GUID, pi::hob::PhaseHandoffInformationTable};
    use patina_internal_cpu::save_state::PROCESSOR_INFO_ENTRY_SIZE;
    use patina_paging::PageTable;
    use serial_test::serial;
    use std::panic::catch_unwind;

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

        assert_eq!(scanned, [MmramRegion::new(memory.base(), memory.size(), false)]);
        // SAFETY: the same live allocation, read through a shared view while nothing else
        // references it.
        let after_scan = unsafe { core::slice::from_raw_parts(memory.base() as *const u8, size) };
        assert!(after_scan.iter().all(|byte| *byte == 0xa5), "scanning wrote into the memory the HOB list describes");

        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
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
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        let hob_list = smram_hob_list(&memory);
        let scanned = scan_regions(&hob_list);
        let page_allocator = PageAllocator::new();
        let paging_allocator = PagingPoolAllocator::new();

        assert_eq!(scanned, [MmramRegion::new(memory.base(), memory.size(), false)]);

        // SAFETY: the scanned descriptor references the live, exclusively owned page-aligned
        // `memory` allocation.
        unsafe { supervisor.init_page_allocators(&scanned, &page_allocator, &paging_allocator) }
            .expect("the scanned region supports both allocators");

        assert!(page_allocator.is_initialized());
        assert!(paging_allocator.is_initialized());
        assert_eq!(paging_allocator.free_page_count(), memory::DEFAULT_PAGING_POOL_PAGES);
    }

    #[test]
    fn test_init_page_allocators_reports_paging_pool_allocation_failure() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
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
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
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
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
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
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
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
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
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

        let page = UEFI_PAGE_SIZE as u64;
        let reclaimed = ((producer + size as u64) / page - producer.div_ceil(page)) as usize;
        assert_eq!(reclaimed, 2, "the producer's list must span whole pages for the reclaim to run");

        let free_before = security_state().page_allocator().free_page_count();
        // SAFETY: `producer` now holds a valid HOB list inside the committed MMRAM region, so it
        // begins with a Phase Handoff Information Table that stays live for the borrow.
        let handoff = unsafe { &*(producer as *const PhaseHandoffInformationTable) };
        let copy = supervisor.publish_hob_list_to_user(handoff).expect("publishing the HOB list copy should succeed");

        assert_ne!(copy, 0);
        assert_eq!(security_state().page_allocator().get_allocation_type(copy), Some(AllocationType::User));

        // SAFETY: `copy` is a live allocation of at least `size` bytes the supervisor just filled.
        let published = unsafe { core::slice::from_raw_parts(copy as *const u8, size) };
        // SAFETY: `source` is still live and holds the bytes the copy was taken from.
        let original = unsafe { core::slice::from_raw_parts(source.as_ptr().cast::<u8>(), size) };
        assert_eq!(published, original, "the published copy does not match the producer's list");

        let copy_pages = size.div_ceil(UEFI_PAGE_SIZE);
        let tail = copy_pages * UEFI_PAGE_SIZE - size;
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
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 32);
        init_global_state_over(&memory);

        // A user-owned staging range makes the supervisor-typed reclaim fail, which is reported
        // rather than fatal: the copy the user core needs has already been published.
        let staging =
            security_state().page_allocator().allocate_pages_with_type(4, AllocationType::User).expect("staging pages");
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
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        init_global_state_over(&memory);

        // A list the producer placed outside the regions it described is refused before it is read.
        let outside = padded_hob_list(0);

        // The refusal is now reported rather than panicked, so the caller can see why.
        let result = supervisor.publish_hob_list_to_user(outside.handoff());

        assert!(
            matches!(result, Err(MmSupervisorError::HobValidation(HobValidationError::HobListOutsideMmram { .. }))),
            "a HOB list outside MMRAM was published to Ring 3, got {result:?}"
        );
    }

    #[test]
    fn test_map_cpl3_stacks_to_user_rejects_a_region_the_hob_does_not_describe() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();

        // A zero base or per-core size, and a total that overflows, all mean the producer gave
        // the supervisor nowhere to demote to. Each is reported rather than halting here, and
        // each is refused before the region is looked up in the page table.
        for (base, per_core_size, num_cpus) in [(0, 0x2000, 4), (0x1000, 0, 4), (0x1000, u64::MAX, 4)] {
            assert_eq!(
                supervisor.map_cpl3_stacks_to_user(base, per_core_size, num_cpus),
                Err(CoreInitError::Cpl3StackRegionInvalid { base, per_core_size, num_cpus }.into()),
                "base 0x{base:x} size 0x{per_core_size:x} for {num_cpus} CPUs should be refused"
            );
        }
    }

    #[test]
    #[serial]
    fn test_map_cpl3_stacks_to_user_rejects_a_region_outside_mmram() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        init_global_state_over(&memory);

        // The producer names this range, so mapping it user-accessible without the MMRAM check
        // would hand Ring 3 write access to whatever sits outside MMRAM at that address.
        let outside = 0x1000;
        let per_core_size = UEFI_PAGE_SIZE as u64;

        let result = supervisor.map_cpl3_stacks_to_user(outside, per_core_size, 2);

        assert_eq!(
            result,
            Err(CoreInitError::Cpl3StackRegionOutsideMmram { base: outside, size: 2 * per_core_size }.into())
        );
    }

    #[test]
    #[serial]
    fn test_publish_hob_list_to_user_refuses_to_publish_without_a_page_table() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
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
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let state = InitState::new();
        let hob_list = RawHobList::new().finish();

        // The discovery now reports the missing module rather than leaving the region unset.
        let result = supervisor.discover_and_store_init_region(hob_list.handoff(), &state);
        assert_eq!(result, Err(CoreInitError::InitModuleRegionMissing.into()));

        assert_eq!(state.init_module_region(), None);
        supervisor.free_init_module(&state);

        assert!(!state.is_init_module_freed());
    }

    #[test]
    fn test_free_init_module_skips_already_freed_region() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let state = InitState::new();
        state.mark_init_module_freed();

        supervisor.free_init_module(&state);
    }

    #[test]
    fn test_free_init_module_releases_mixed_code_and_data_exactly_once() {
        let mapped = MappedInitModule::new();
        let base = mapped.init_module.alloc_descriptor.memory_base_address;
        let size = mapped.init_module.alloc_descriptor.memory_length as usize;
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
            for address in (base..base + size as u64).step_by(UEFI_PAGE_SIZE) {
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
        assert_eq!(mapped.state.init_module_region(), Some((init_base, init_size)));
        assert!(!mapped.state.is_init_module_freed());
        assert_eq!(allocator.get_allocation_type(init_base), Some(AllocationType::Supervisor));
        let reclaimed_pages = size / UEFI_PAGE_SIZE;
        let copy_pages = size.div_ceil(UEFI_PAGE_SIZE);
        assert_eq!(reclaimed_pages, 2);
        assert_eq!(allocator.free_page_count(), free_before - copy_pages + reclaimed_pages);
        for address in (producer..producer + (reclaimed_pages * UEFI_PAGE_SIZE) as u64).step_by(UEFI_PAGE_SIZE) {
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
        let reused = unsafe { core::slice::from_raw_parts_mut(producer as *mut u8, reclaimed_pages * UEFI_PAGE_SIZE) };
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
        // SAFETY: the published user copy and source mapped are both still readable.
        assert_eq!(unsafe { core::slice::from_raw_parts(user_copy as *const u8, size) }, unsafe {
            core::slice::from_raw_parts(source.as_ptr().cast::<u8>(), size)
        });
        assert_eq!(allocator.get_allocation_type(user_copy), Some(AllocationType::User));
        assert_eq!(
            security_state()
                .lock_page_table()
                .as_ref()
                .unwrap()
                .query_memory_region(user_copy, (copy_pages * UEFI_PAGE_SIZE) as u64),
            Ok(MemoryAttributes::ReadOnly | MemoryAttributes::ExecuteProtect)
        );
    }

    #[test]
    fn test_init_policy_from_hob_list_reports_missing_hobs() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();

        // A null list is no longer representable: the handoff table is taken by reference.
        let hob_list = RawHobList::new().finish();
        let result = supervisor.init_policy_from_hob_list(hob_list.handoff());
        assert_eq!(result, Err(CoreInitError::MpInformationHobMissing.into()));
    }

    #[test]
    fn test_init_policy_from_hob_list_reports_a_missing_pass_down_hob() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();

        // Carries the MP Information HOB so initialization reaches the PassDown lookup and
        // stops there. Lists that get past this point reach steps that touch real MMRAM, so
        // the later required-HOB lookups are not reachable from a host test.
        let mut without_pass_down = RawHobList::new();
        without_pass_down.push_guid_hob(MP_INFORMATION_HOB_GUID, &mp_information_hob_data(2));
        let without_pass_down = without_pass_down.finish();

        let result = supervisor.init_policy_from_hob_list(without_pass_down.handoff());
        assert_eq!(result, Err(PassDownHobError::Missing.into()));
    }

    #[test]
    fn test_init_policy_and_validate_reports_initialization_failure() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let hob_list = RawHobList::new().finish();

        let result = supervisor.init_policy_and_validate(hob_list.handoff());

        assert_eq!(result, Err(CoreInitError::MpInformationHobMissing.into()));
    }

    #[test]
    fn test_init_policy_and_validate_reports_an_invalid_mp_cpu_count() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();

        for cpu_count in [0, 5] {
            let hob_list = policy_hob_list_with_cpu_count(cpu_count, true);

            let result = supervisor.init_policy_and_validate(hob_list.handoff());

            assert!(result.is_err(), "CPU count {cpu_count} should fail-stop initialization");
        }
    }

    #[test]
    fn test_parse_mp_information_hob() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
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
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let mut data = [0_u8; 16 + PROCESSOR_INFO_ENTRY_SIZE];

        assert_eq!(
            supervisor.parse_mp_information_hob(&data[..15]),
            Err(CoreInitError::MpInformationHobMalformed.into())
        );

        data[..8].copy_from_slice(&2_u64.to_le_bytes());
        assert_eq!(supervisor.parse_mp_information_hob(&data), Err(CoreInitError::MpInformationHobMalformed.into()));
    }

    #[test]
    fn test_parse_mp_information_hob_rejects_invalid_cpu_count() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let mut data = [0_u8; 16];

        for cpu_count in [0_usize, 5] {
            data[..8].copy_from_slice(&(cpu_count as u64).to_le_bytes());
            assert_eq!(
                supervisor.parse_mp_information_hob(&data),
                Err(CoreInitError::InvalidCpuCount { found: cpu_count, maximum: 4 }.into())
            );
        }
    }

    #[test]
    fn test_store_mm_initialized_slots_accepts_a_null_buffer() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();

        // The MM IPL may omit the buffer. The supervisor then runs without per-core initialized
        // tracking rather than refusing to start.
        // SAFETY: a null buffer returns before anything is read.
        assert_eq!(unsafe { supervisor.store_mm_initialized_slots(0, 2) }, Ok(()));
        assert!(init_state().mm_initialized_buffer().is_none());
    }

    #[test]
    fn test_store_mm_initialized_slots_rejects_an_unusable_cpu_count() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let memory = PageAlignedMemory::new(1);

        // The count is the length of the slot slice, so it is refused before the buffer is read.
        for cpu_count in [0, 5] {
            // SAFETY: the count is rejected before `memory` is dereferenced.
            let result = unsafe { supervisor.store_mm_initialized_slots(memory.base(), cpu_count) };
            assert_eq!(result, Err(CoreInitError::InvalidCpuCount { found: cpu_count, maximum: 4 }.into()));
        }
        assert!(init_state().mm_initialized_buffer().is_none());
    }

    #[test]
    #[serial]
    fn test_store_mm_initialized_slots_rejects_a_buffer_outside_mmram() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
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
    fn test_store_mm_initialized_slots_publishes_one_slot_per_cpu() {
        test_support::init_test_logger();
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        test_support::init_global_state_over(&memory);

        // SAFETY: `memory` is a live allocation inside the MMRAM the allocators were built over,
        // and it outlives the published slice for the rest of this test.
        assert_eq!(unsafe { supervisor.store_mm_initialized_slots(memory.base(), 3) }, Ok(()));

        let slots = init_state().mm_initialized_buffer().expect("the slots were published");
        assert_eq!(slots.len(), 3);
    }

    #[test]
    fn test_validate_init_code_page_requires_supervisor_readonly_executable() {
        let code = MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly;
        validate_init_code_page(0x1000, code);
        validate_init_code_page(0x2000, MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect);
        for attributes in
            [MemoryAttributes::Supervisor, MemoryAttributes::ReadOnly, code | MemoryAttributes::ReadProtect]
        {
            assert!(catch_unwind(|| validate_init_code_page(0x1000, attributes)).is_err());
        }
    }

    #[test]
    fn test_free_init_module_accepts_an_entirely_non_executable_image() {
        let mapped = MappedInitModule::new();
        let base = mapped.init_module.alloc_descriptor.memory_base_address;
        let size = mapped.init_module.alloc_descriptor.memory_length;
        security_state()
            .lock_page_table()
            .as_mut()
            .unwrap()
            .map_memory_region(base, size, MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect)
            .unwrap();

        mapped.free();

        assert!(mapped.state.is_init_module_freed());
        assert_eq!(security_state().page_allocator().get_allocation_type(base), None);
    }

    #[test]
    fn test_free_init_module_rejects_missing_page_table() {
        let mapped = MappedInitModule::new();
        *security_state().lock_page_table() = None;

        mapped.assert_rejected("Page table required to validate MM Init module");
    }

    #[test]
    fn test_free_init_module_rejects_an_unmapped_later_page() {
        let mapped = MappedInitModule::new();
        let last_page = mapped.init_module.alloc_descriptor.memory_base_address + 2 * UEFI_PAGE_SIZE as u64;
        security_state()
            .lock_page_table()
            .as_mut()
            .unwrap()
            .unmap_memory_region(last_page, UEFI_PAGE_SIZE as u64)
            .unwrap();

        mapped.assert_rejected("Failed to query MM Init module page");
    }

    #[test]
    fn test_free_init_module_rejects_unprotected_later_code_pages() {
        let mapped = MappedInitModule::new();
        let last_page = mapped.init_module.alloc_descriptor.memory_base_address + 2 * UEFI_PAGE_SIZE as u64;
        for attributes in [MemoryAttributes::Supervisor, MemoryAttributes::ReadOnly] {
            security_state()
                .lock_page_table()
                .as_mut()
                .unwrap()
                .map_memory_region(last_page, UEFI_PAGE_SIZE as u64, attributes)
                .unwrap();
            mapped.assert_rejected("must be supervisor-only, read-only and executable");
        }
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
}
