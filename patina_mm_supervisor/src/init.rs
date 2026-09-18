//! MM Supervisor Core Initialization
//!
//! This module contains all one-time initialization logic for the MM Supervisor Core,
//! including BSP initialization, per-core setup, HOB discovery, policy gate initialization,
//! and SMI handler IDT patching.
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
        MmCommBufferStatus,
        comm_buffer_hob::{MM_COMM_BUFFER_HOB_GUID, MmCommonBufferHobData},
        supervisor::{MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_USER_GUID},
    },
    pi::hob::{self, Hob, PhaseHandoffInformationTable},
};
use patina_paging::{
    MemoryAttributes, PageTable, PagingType,
    x64::{X64PageTable, disable_write_protection, enable_write_protection},
};

use crate::{
    AllocationType, CommBufferConfig, MmSupervisorCore, PageOwnership, PlatformInfo, SharedPagingAllocator,
    hob_validation,
    intrinsics::{get_current_cpu_id, read_cr3, write_msr},
    is_buffer_inside_mmram,
    mem::page_allocator::SmramDescriptor,
    mem::{
        self,
        page_allocator::{MAX_TEMP_REGIONS, coalesced_smrr_range},
    },
    mm_policy::{self, MemDescriptorV1_0, dump_policy, gate::PolicyGate, walk_page_table},
    query_address_ownership,
    save_state::SaveStateInfo,
    smrr::{SmramRegion, configure_smm_code_access, smrr_initialize},
    state::{init_state, security_state},
};

use patina_internal_cpu::{interrupts::Interrupts, save_state::PROCESSOR_INFO_ENTRY_SIZE};
use zerocopy::FromBytes;
use zerocopy_derive::Immutable;

/// Errors that can occur during policy initialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyInitError {
    /// The HOB list pointer is null.
    NullHobList,
    /// Some HOB not found.
    HobNotFound,
    /// Invalid `PassDown` HOB revision.
    InvalidRevision {
        /// The revision value found in the `PassDown` HOB.
        found: u32,
        /// The revision value the supervisor expected.
        expected: u32,
    },
    /// Firmware policy buffer is null or empty.
    NullFirmwarePolicyBuffer,
    /// Invalid policy data.
    InvalidPolicyData,
    /// The MP Information HOB reports an unsupported CPU count.
    InvalidCpuCount {
        /// CPU count reported by the HOB.
        found: u64,
        /// Maximum CPU count supported by this supervisor instance.
        maximum: usize,
    },
    /// A communication buffer page count is zero, cannot fit the target
    /// architecture, or produces an overflowing address range.
    InvalidCommunicationBufferSize {
        /// The invalid page count.
        pages: u64,
    },
    /// Memory allocation failed for policy buffers.
    MemoryAllocationFailed,
    /// One or more communication buffers are not properly initialized.
    MissingCommunicationBuffer,
}

/// Offset from SMBASE where the SMI handler code is located.
const SMM_HANDLER_OFFSET: u64 = 0x8000;

/// MSR index for `IA32_SMM_MONITOR_CTL`, which holds the MSEG base used to
/// activate the dual-monitor treatment (Intel SDM Vol. 4).
const IA32_SMM_MONITOR_CTL_MSR: u32 = 0x9b;

/// `IA32_SMM_MONITOR_CTL.Valid` (bit 0). An STM may only be invoked when set.
const SMM_MONITOR_CTL_VALID: u64 = 1;

/// `IA32_SMM_MONITOR_CTL.MsegBase` (bits 31:12).
const SMM_MONITOR_CTL_MSEG_BASE_MASK: u64 = 0xffff_f000;

/// Index into the Fixup64 array for the SMI handler IDTR pointer.
const FIXUP64_SMI_HANDLER_IDTR: usize = 5;

/// MM Common Region HOB Data Structure
///
/// Describes the supervisor MM communication region published by the C MM
/// IPL under `gMmCommonRegionHobGuid`. Carries the buffer location/size and
/// a dedicated `MmCommBufferStatus` mailbox in `status_addr`. The layout
/// matches the C `MM_COMM_REGION_HOB` from `MmCommonRegion.h`; the
/// `region_type` discriminator exists for C ABI parity but is always
/// `MM_SUPERVISOR_BUFFER_T` (0) in practice — the user channel uses the
/// separate `gMmCommBufferHobGuid` HOB.
#[repr(C)]
#[derive(Debug, Clone, Copy, FromBytes, Immutable)]
pub struct MmCommonRegionHobData {
    /// Region type discriminator. Always `MM_SUPERVISOR_BUFFER_T` (0) for
    /// the HOB the supervisor consumes.
    pub region_type: u64,
    /// Base address of the communication buffer region.
    pub addr: u64,
    /// Number of pages in the communication buffer region.
    pub number_of_pages: u64,
    /// Address of the `MmCommBufferStatus` structure that pairs with this region.
    pub status_addr: u64,
}

/// MM Supervisor `PassDown` HOB Data Structure
///
/// This structure contains various buffer pointers and sizes passed from
/// the PEI phase to the MM Supervisor.
///
/// All fields are naturally aligned (`u32`, `u32`, then `u64`s), so `repr(C)`
/// has the same byte layout the C producer emits while still allowing safe,
/// reference-based field access once parsed via `zerocopy`.
#[repr(C)]
#[derive(Debug, Clone, Copy, FromBytes, Immutable)]
pub struct MmSupvPassDownHobData {
    /// Revision of this HOB structure
    pub revision: u32,
    /// Reserved for future use
    pub reserved: u32,
    /// Base address of CPL3 stack for MM Supervisor
    pub mm_supervisor_cpl3_stack_base: u64,
    /// Per-CPU stack size for CPL3
    pub mm_supervisor_cpl3_per_core_stack_size: u64,
    /// Pointer to the per-CPU SMBASE array (`u64[number_of_cpus]`), indexed by the
    /// UEFI processor index (the same `cpu_index` the supervisor registers).
    ///
    /// The save-state region base for a CPU is `sm_base[cpu_index] +
    /// SMRAM_SAVE_STATE_MAP_OFFSET`. The BSP's own entry also serves as the
    /// IDT-patch fallback when `IA32_MSR_SMBASE` reads 0 (e.g. on QEMU).
    pub sm_base: u64,
    /// MM Initialized buffer base address
    pub mm_initialized_buffer: u64,
    /// MM Supervisor firmware policy buffer base address
    pub mm_supv_firmware_policy_buffer: u64,
    /// Size of MM Supervisor firmware policy buffer
    pub mm_supv_firmware_policy_buffer_size: u64,
    /// Size of the MMI entry point structure (for validating against expected size in supervisor)
    pub mmi_entrypoint_size: u64,
}

/// Per-core MMI entry structure header.
///
/// This packed structure is embedded at the end of the SMI handler binary template.
/// It contains offsets (relative to the header start) to fixup arrays that the
/// relocation code uses to patch per-CPU values into the binary.
///
/// Layout matches the C `PER_CORE_MMI_ENTRY_STRUCT_HDR` from SeaResponder.h.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, FromBytes, Immutable)]
struct PerCoreMmiEntryStructHdr {
    /// Header version (4 for version 4).
    header_version: u32,
    /// Offset from header start to `FixUpStruct` array.
    fixup_struct_offset: u8,
    /// Number of `FixUpStruct` array entries.
    fixup_struct_num: u8,
    /// Offset from header start to Fixup64 array.
    fixup64_offset: u8,
    /// Number of Fixup64 array entries.
    fixup64_num: u8,
    /// Offset from header start to Fixup32 array.
    fixup32_offset: u8,
    /// Number of Fixup32 array entries.
    fixup32_num: u8,
    /// Offset from header start to Fixup8 array.
    fixup8_offset: u8,
    /// Number of Fixup8 array entries.
    fixup8_num: u8,
    /// SMI entry binary version.
    binary_version: u16,
    /// SPL value for SMI entry binary.
    spl_value: u32,
    /// Reserved for future use.
    reserved: u32,
}

/// Pointer structure used by the `SIDT` / `LIDT` (and `SGDT` / `LGDT`) instructions.
///
/// Layout matches the Intel SDM: a 16-bit limit followed by a 64-bit base.
/// `packed(2)` produces the expected 10-byte on-the-wire representation with no
/// internal padding between `limit` and `base`.
#[repr(C, packed(2))]
#[derive(Debug, Clone, Copy)]
struct DescriptorTablePointer {
    /// Size of the descriptor table in bytes, minus 1.
    limit: u16,
    /// Linear address of the descriptor table.
    base: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmiHandlerIdtPatchError {
    EntryTooSmall,
    FixupStructureOutOfBounds,
    FixupHeaderTooSmall,
    Fixup64ArrayTooSmall { found: u8 },
    Fixup64EntryOutOfBounds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmiHandlerIdtPatchInputError {
    ZeroEntrySize,
    MissingSmBaseArray,
    CpuCountTooLarge,
    SmBaseArraySizeOverflow,
    SmBaseArrayOutsideMmram,
    EntrySizeTooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SmiHandlerIdtPatchInputs {
    sm_base_array_size: usize,
    mmi_entry_size: usize,
    mmi_entry_size_u64: u64,
}

fn validate_smi_handler_idt_patch_inputs(
    sm_base_array: u64,
    number_of_cpus: u64,
    mmi_entry_size: u64,
    is_inside_mmram: impl Fn(u64, u64) -> bool,
) -> Result<SmiHandlerIdtPatchInputs, SmiHandlerIdtPatchInputError> {
    if mmi_entry_size == 0 {
        return Err(SmiHandlerIdtPatchInputError::ZeroEntrySize);
    }
    if sm_base_array == 0 || number_of_cpus == 0 {
        return Err(SmiHandlerIdtPatchInputError::MissingSmBaseArray);
    }

    let cpu_count = usize::try_from(number_of_cpus).map_err(|_| SmiHandlerIdtPatchInputError::CpuCountTooLarge)?;
    let sm_base_array_size = cpu_count
        .checked_mul(core::mem::size_of::<u64>())
        .filter(|size| isize::try_from(*size).is_ok())
        .ok_or(SmiHandlerIdtPatchInputError::SmBaseArraySizeOverflow)?;
    let sm_base_array_size_u64 =
        u64::try_from(sm_base_array_size).map_err(|_| SmiHandlerIdtPatchInputError::SmBaseArraySizeOverflow)?;
    if sm_base_array.checked_add(sm_base_array_size_u64).is_none()
        || !is_inside_mmram(sm_base_array, sm_base_array_size_u64)
    {
        return Err(SmiHandlerIdtPatchInputError::SmBaseArrayOutsideMmram);
    }

    let mmi_entry_size_usize = usize::try_from(mmi_entry_size)
        .ok()
        .filter(|size| isize::try_from(*size).is_ok())
        .ok_or(SmiHandlerIdtPatchInputError::EntrySizeTooLarge)?;

    Ok(SmiHandlerIdtPatchInputs {
        sm_base_array_size,
        mmi_entry_size: mmi_entry_size_usize,
        mmi_entry_size_u64: mmi_entry_size,
    })
}

fn parse_smi_handler_idt_descriptor(mmi_entry: &[u8]) -> Result<u64, SmiHandlerIdtPatchError> {
    const TRAILING_SIZE_FIELD_SIZE: usize = core::mem::size_of::<u32>();

    let trailer_start =
        mmi_entry.len().checked_sub(TRAILING_SIZE_FIELD_SIZE).ok_or(SmiHandlerIdtPatchError::EntryTooSmall)?;
    let trailer = mmi_entry.get(trailer_start..).ok_or(SmiHandlerIdtPatchError::EntryTooSmall)?;
    let whole_struct_size =
        u32::from_ne_bytes(trailer.try_into().map_err(|_| SmiHandlerIdtPatchError::EntryTooSmall)?) as usize;
    let header_start =
        trailer_start.checked_sub(whole_struct_size).ok_or(SmiHandlerIdtPatchError::FixupStructureOutOfBounds)?;
    let fixup_structure =
        mmi_entry.get(header_start..trailer_start).ok_or(SmiHandlerIdtPatchError::FixupStructureOutOfBounds)?;
    let (header, _) = PerCoreMmiEntryStructHdr::read_from_prefix(fixup_structure)
        .map_err(|_| SmiHandlerIdtPatchError::FixupHeaderTooSmall)?;

    if FIXUP64_SMI_HANDLER_IDTR >= usize::from(header.fixup64_num) {
        return Err(SmiHandlerIdtPatchError::Fixup64ArrayTooSmall { found: header.fixup64_num });
    }

    let fixup64_entry_start = usize::from(header.fixup64_offset)
        .checked_add(
            FIXUP64_SMI_HANDLER_IDTR
                .checked_mul(core::mem::size_of::<u64>())
                .ok_or(SmiHandlerIdtPatchError::Fixup64EntryOutOfBounds)?,
        )
        .ok_or(SmiHandlerIdtPatchError::Fixup64EntryOutOfBounds)?;
    let fixup64_entry_end = fixup64_entry_start
        .checked_add(core::mem::size_of::<u64>())
        .ok_or(SmiHandlerIdtPatchError::Fixup64EntryOutOfBounds)?;
    let fixup64_entry = fixup_structure
        .get(fixup64_entry_start..fixup64_entry_end)
        .ok_or(SmiHandlerIdtPatchError::Fixup64EntryOutOfBounds)?;

    Ok(u64::from_ne_bytes(fixup64_entry.try_into().map_err(|_| SmiHandlerIdtPatchError::Fixup64EntryOutOfBounds)?))
}

/// Read the current IDT Register (IDTR) via the `SIDT` instruction.
///
/// Returns a [`DescriptorTablePointer`] containing the IDT base and limit.
fn read_idtr() -> DescriptorTablePointer {
    let rt_descriptor = DescriptorTablePointer { limit: 0, base: 0 };

    // On the real firmware target, populate it via `SIDT`. The asm-free builds
    // (tests / non-x86_64) keep the zero-initialized value, so no mutable binding
    // is introduced where it would go unused.
    #[cfg(not(test))]
    let rt_descriptor = {
        let mut descriptor = rt_descriptor;
        // SAFETY: SIDT stores the 10-byte IDTR pseudo-descriptor to the specified
        // memory location. This is a read-only operation on CPU state.
        unsafe {
            core::arch::asm!(
                "sidt [{}]",
                in(reg) &raw mut descriptor,
                options(nostack, preserves_flags)
            );
        }
        descriptor
    };

    rt_descriptor
}

trait SmiHandlerIdtPatchServices {
    fn is_inside_mmram(&self, address: u64, size: u64) -> bool;
    fn read_idtr(&self) -> DescriptorTablePointer;
    unsafe fn write_idtr(&mut self, address: u64, idtr: DescriptorTablePointer);
}

struct RuntimeSmiHandlerIdtPatchServices;

impl SmiHandlerIdtPatchServices for RuntimeSmiHandlerIdtPatchServices {
    fn is_inside_mmram(&self, address: u64, size: u64) -> bool {
        is_buffer_inside_mmram(address, size)
    }

    fn read_idtr(&self) -> DescriptorTablePointer {
        read_idtr()
    }

    unsafe fn write_idtr(&mut self, address: u64, idtr: DescriptorTablePointer) {
        // SAFETY: the caller validated that `address` covers a writable descriptor in MMRAM.
        unsafe { core::ptr::write_unaligned(address as *mut DescriptorTablePointer, idtr) };
    }
}

type CommBufferInitResult = (u64, u64, u64, u64);

trait PolicyInitServices {
    type PolicyCheckError: core::fmt::Debug;

    unsafe fn init_from_pass_down_hob(
        &mut self,
        data: &[u8],
        number_of_cpus: u64,
    ) -> Result<(u64, u64), PolicyInitError>;
    fn set_save_state_info(&mut self, info: SaveStateInfo);
    fn set_mseg_base(&mut self, base: u64);
    fn patch_smi_handler_idt(&mut self, sm_base: u64, number_of_cpus: u64, mmi_entry_size: u64);
    fn init_supv_comm_buffer(&mut self, data: &[u8]) -> Result<CommBufferInitResult, PolicyInitError>;
    unsafe fn init_user_comm_buffer(
        &mut self,
        data: *mut u8,
        data_len: usize,
    ) -> Result<CommBufferInitResult, PolicyInitError>;
    fn allocate_supv_to_user_buffer(&mut self) -> Result<u64, PolicyInitError>;
    fn set_comm_buffer_config(&mut self, config: CommBufferConfig);
    fn validate_policy(&mut self) -> Option<Result<(), Self::PolicyCheckError>>;
}

struct RuntimePolicyInitServices<'a, P: PlatformInfo, const MAX_CPUS: usize> {
    supervisor: &'a MmSupervisorCore<P, MAX_CPUS>,
}

impl<P: PlatformInfo, const MAX_CPUS: usize> PolicyInitServices for RuntimePolicyInitServices<'_, P, MAX_CPUS> {
    type PolicyCheckError = mm_policy::helpers::PolicyCheckError;

    unsafe fn init_from_pass_down_hob(
        &mut self,
        data: &[u8],
        number_of_cpus: u64,
    ) -> Result<(u64, u64), PolicyInitError> {
        // SAFETY: the caller forwards a validated PassDown HOB payload.
        unsafe { self.supervisor.init_from_pass_down_hob(data, number_of_cpus) }
    }

    fn set_save_state_info(&mut self, info: SaveStateInfo) {
        security_state().set_save_state_info(info);
    }

    fn set_mseg_base(&mut self, base: u64) {
        init_state().set_mseg_base(base);
    }

    fn patch_smi_handler_idt(&mut self, sm_base: u64, number_of_cpus: u64, mmi_entry_size: u64) {
        MmSupervisorCore::<P, MAX_CPUS>::patch_smi_handler_idt(
            sm_base,
            number_of_cpus,
            mmi_entry_size,
            &mut RuntimeSmiHandlerIdtPatchServices,
        );
    }

    fn init_supv_comm_buffer(&mut self, data: &[u8]) -> Result<CommBufferInitResult, PolicyInitError> {
        init_supv_comm_buffer(data)
    }

    unsafe fn init_user_comm_buffer(
        &mut self,
        data: *mut u8,
        data_len: usize,
    ) -> Result<CommBufferInitResult, PolicyInitError> {
        // SAFETY: the caller forwards the original writable user communication HOB payload.
        unsafe { init_user_comm_buffer(data, data_len) }
    }

    fn allocate_supv_to_user_buffer(&mut self) -> Result<u64, PolicyInitError> {
        security_state().page_allocator().allocate_pages_with_type(1, AllocationType::User).map_err(|e| {
            log::error!("Failed to allocate page for supervisor-to-user buffer: {e:?}");
            PolicyInitError::MemoryAllocationFailed
        })
    }

    fn set_comm_buffer_config(&mut self, config: CommBufferConfig) {
        security_state().set_comm_buffer_config(config);
    }

    fn validate_policy(&mut self) -> Option<Result<(), Self::PolicyCheckError>> {
        security_state().policy_gate().map(|gate| {
            // SAFETY: `gate.as_ptr()` returns the resident firmware policy buffer pointer
            // validated while constructing the policy gate.
            unsafe { mm_policy::helpers::security_policy_check(gate.as_ptr()) }
        })
    }
}

impl<P: PlatformInfo, const MAX_CPUS: usize> MmSupervisorCore<P, MAX_CPUS> {
    /// BSP-specific initialization.
    ///
    /// This is called only on the BSP after basic setup is complete. It
    /// initializes interrupts, the page and paging allocators, the global page
    /// table, discovers the user module entry point, initializes the security
    /// policy, and remaps the HOB list so the demoted user core can read it.
    pub(crate) fn bsp_init(&'static self, hob_list: *const c_void) {
        log::info!("BSP performing one-time initialization...");

        let mut interrupt_manager = Interrupts::new();
        interrupt_manager.initialize().unwrap_or_else(|err| {
            panic!("Failed to initialize Interrupt Manager: {err:?}");
        });

        // SAFETY: `hob_list` is provided by the MM IPL and is guaranteed to be a
        // valid HOB list (the caller asserts it is non-null before dispatching).
        let (scanned_regions, region_count) = unsafe {
            self.init_page_allocators(
                hob_list,
                security_state().page_allocator(),
                security_state().paging_allocator(),
                init_state(),
                coalesced_smrr_range,
            )
        };

        // Validate the critical incoming HOBs against the untrusted producer's
        // data before any of their contents are consumed below.
        let scanned_regions = scanned_regions.get(..region_count).unwrap_or(&scanned_regions);
        // SAFETY: `hob_list` was checked non-null by `entry_point` and points to
        // a valid HOB list for the duration of BSP initialization.
        let handoff = unsafe { (hob_list as *const PhaseHandoffInformationTable).as_ref() }
            .expect("BSP initialization requires a non-null HOB list");
        if let Err(e) = hob_validation::validate_incoming_hobs_pre_paging_init(handoff, scanned_regions) {
            panic!("Incoming HOB validation failed: {e}");
        }

        self.init_page_table();

        // Validate the incoming HOBs that require an active page table, now
        // that it is available (the remaining checks that only need the page
        // allocator ran above).
        if let Err(e) = hob_validation::validate_incoming_hobs_post_paging_init(handoff) {
            panic!("Post-paging HOB validation failed: {e}");
        }

        let mut policy_services = RuntimePolicyInitServices { supervisor: self };
        // SAFETY: `hob_list` is provided by the MM IPL and is guaranteed to be a
        // valid HOB list (the caller asserts it is non-null before dispatching).
        unsafe {
            self.discover_and_store_user_entry(hob_list, init_state());
            self.init_policy_and_validate(hob_list, &mut policy_services);
            self.remap_hob_list_to_user(hob_list);
        }

        log::info!("BSP one-time initialization complete.");
    }

    /// Initializes the page and paging allocators from the HOB list.
    ///
    /// Sets up SMRAM memory tracking from the HOB list, reserves a pool of
    /// pages for paging structures (done before paging is initialized to avoid
    /// a circular dependency), and initializes the paging allocator with that
    /// pool.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `hob_list` points to a valid HOB list.
    unsafe fn init_page_allocators(
        &self,
        hob_list: *const c_void,
        page_allocator: &mem::PageAllocator,
        paging_allocator: &mem::PagingPoolAllocator,
        state: &crate::state::InitState,
        derive_smrr_range: impl FnOnce(&[SmramRegion]) -> Option<SmramRegion>,
    ) -> ([SmramRegion; MAX_TEMP_REGIONS], usize) {
        // Initialize the page allocator from the HOB list. This finds all SMRAM
        // regions and sets up memory tracking.
        // SAFETY: `hob_list` is a valid HOB list per this function's contract.
        let (smram_regions, region_count) = match unsafe { page_allocator.init_from_hob_list(hob_list) } {
            Ok(scanned_regions) => scanned_regions,
            Err(e) => panic!("Failed to initialize page allocator: {e:?}"),
        };

        // Derive the SMRR range from the scanned SMRAM regions, coalescing
        // physically adjacent regions, and store it for later SMRR programming.
        match derive_smrr_range(smram_regions.get(..region_count).unwrap_or(&smram_regions)) {
            Some(range) => {
                log::info!("Discovered SMRR range: base=0x{:08x}, size=0x{:08x}", range.base, range.size);
                state.set_smrr_range(range);
            }
            None => panic!("Failed to determine SMRR range from scanned SMRAM regions"),
        }

        // Reserve pages from the page allocator for paging structures. This is
        // done before paging is initialized to avoid a circular dependency.
        let paging_pool_base = match page_allocator.allocate_pages(mem::DEFAULT_PAGING_POOL_PAGES) {
            Ok(base) => base,
            Err(e) => {
                panic!("Failed to reserve pages for paging structures: {e:?}");
            }
        };
        log::info!(
            "Reserved {} pages at 0x{:016x} for paging structures",
            mem::DEFAULT_PAGING_POOL_PAGES,
            paging_pool_base
        );

        // Initialize the paging allocator with the reserved pool.
        // SAFETY: `paging_pool_base` was just reserved from the page allocator, so it is a
        // page-aligned region of `DEFAULT_PAGING_POOL_PAGES` pages in SMRAM owned exclusively by
        // the paging allocator.
        let init_result = unsafe { paging_allocator.init(paging_pool_base, mem::DEFAULT_PAGING_POOL_PAGES) };
        if let Err(e) = init_result {
            panic!("Failed to initialize paging allocator: {e:?}");
        }

        (smram_regions, region_count)
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
    /// ## Safety
    ///
    /// The caller must ensure that `hob_list` points to a valid HOB list.
    unsafe fn discover_and_store_user_entry(&self, hob_list: *const c_void, state: &crate::state::InitState) {
        // SAFETY: `hob_list` is a valid HOB list per this function's contract, so it
        // points to a readable handoff table for the duration of initialization.
        let entry = unsafe { (hob_list as *const PhaseHandoffInformationTable).as_ref() }
            .and_then(|handoff| find_user_module_entry(&Hob::Handoff(handoff)));

        match entry {
            Some(entry) => {
                log::info!("Discovered MM User module entry point: 0x{entry:016x}");
                state.set_user_entry_point(entry);
            }
            None => log::warn!("MM User module entry point not found in HOB list"),
        }
    }

    /// Initializes the policy gate from the `PassDown` HOB and runs an initial
    /// security validation.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `hob_list` points to a valid HOB list.
    ///
    /// # Panics
    ///
    /// Panics if policy initialization or the initial security-policy validation fails.
    unsafe fn init_policy_and_validate<S: PolicyInitServices>(&self, hob_list: *const c_void, services: &mut S) {
        // SAFETY: `hob_list` is a valid HOB list per this function's contract.
        if let Err(e) = unsafe { self.init_policy_from_hob_list(hob_list, services) } {
            panic!("Failed to initialize policy gate: {e:?}");
        }

        if let Some(result) = services.validate_policy() {
            if let Err(e) = result {
                panic!("Security policy check failed during init: {e:?}");
            }
            log::info!("Security policy check passed");
        }
    }

    /// Remaps the HOB list as user-accessible so the demoted user core can walk
    /// it during `StartUserCore`.
    ///
    /// Once all HOB content has been consumed, the page-aligned HOB range is
    /// remapped as read-only + non-executable for the user level.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `hob_list` points to a valid HOB list.
    unsafe fn remap_hob_list_to_user(&self, hob_list: *const c_void) {
        let hob_base = hob_list as u64;
        // SAFETY: `hob_list` is a valid HOB list per this function's contract.
        let hob_list_size = unsafe { hob::get_pi_hob_list_size(hob_list) } as u64;

        let (aligned_base, hob_region_size) = align_range(hob_base, hob_list_size, UEFI_PAGE_SIZE as u64)
            .unwrap_or_else(|e| panic!("Failed to page-align HOB list region: {e:?}"));
        let aligned_end = aligned_base + hob_region_size;
        log::info!(
            "HOB list at 0x{hob_base:016x} size 0x{hob_list_size:x}, aligned region 0x{aligned_base:016x}-0x{aligned_end:016x} (0x{hob_region_size:x} bytes)"
        );

        if hob_region_size == 0 {
            return;
        }

        let attrs = MemoryAttributes::ReadOnly | MemoryAttributes::ExecuteProtect;
        let mut pt_guard = security_state().lock_page_table();
        let Some(pt) = pt_guard.as_mut() else {
            panic!("Page table not initialized, cannot remap HOB list to user level");
        };

        if let Err(e) = pt.map_memory_region(aligned_base, hob_region_size, attrs) {
            panic!(
                "Failed to remap HOB list to user level at 0x{aligned_base:016x} (0x{hob_region_size:x} bytes): {e:?}"
            );
        }
        log::info!("Remapped HOB list 0x{aligned_base:016x}-0x{aligned_end:016x} as user read-only");
    }

    /// Maps the per-CPU Ring 3 stacks as user-accessible, writable, non-executable pages.
    ///
    /// A demoted routine faults on its first push if its stack is supervisor-owned. The range
    /// covers the `num_cpus` stacks the MM IPL provisioned, which is also the range
    /// [`SyscallInterface::get_cpl3_stack`](crate::privilege_mgmt::syscall_setup::SyscallInterface::get_cpl3_stack)
    /// hands out.
    fn map_cpl3_stacks_to_user(&self, base: u64, per_core_size: u64, num_cpus: u64) {
        assert!(
            !(base == 0 || per_core_size == 0),
            "PassDown HOB does not describe a CPL3 stack region (base 0x{base:016x}, per-core size 0x{per_core_size:x})"
        );

        let total_size = per_core_size
            .checked_mul(num_cpus)
            .unwrap_or_else(|| panic!("CPL3 stack size 0x{per_core_size:x} for {num_cpus} CPUs overflows"));

        let (aligned_base, aligned_size) = align_range(base, total_size, UEFI_PAGE_SIZE as u64)
            .unwrap_or_else(|e| panic!("Failed to page-align CPL3 stack region: {e:?}"));
        let aligned_end = aligned_base + aligned_size;

        // The region is described by the untrusted producer, so nothing outside MMRAM may be
        // handed to Ring 3.
        assert!(
            is_buffer_inside_mmram(aligned_base, aligned_size),
            "CPL3 stack region 0x{aligned_base:016x}-0x{aligned_end:016x} is not inside MMRAM"
        );

        // Scoped so the page table lock is released before the verification below retakes it.
        {
            let attrs = MemoryAttributes::ExecuteProtect;
            let mut pt_guard = security_state().lock_page_table();
            let Some(pt) = pt_guard.as_mut() else {
                panic!("Page table not initialized, cannot map CPL3 stacks to user level");
            };

            if let Err(e) = pt.map_memory_region(aligned_base, aligned_size, attrs) {
                panic!(
                    "Failed to map CPL3 stacks to user level at 0x{aligned_base:016x} (0x{aligned_size:x} bytes): {e:?}"
                );
            }
        }

        if let Err(e) = hob_validation::verify_cpl3_stacks_user_accessible(aligned_base, aligned_size) {
            panic!("CPL3 stacks at 0x{aligned_base:016x} are not usable by Ring 3 after remapping: {e}");
        }

        log::info!("Mapped CPL3 stacks 0x{aligned_base:016x}-0x{aligned_end:016x} as user read/write, NX");
    }

    /// Patches every core's SMI-handler IDT descriptor to point to the Rust IDT.
    ///
    /// Each per-core MMI entry (copied to `sm_base[i] + 0x8000` during C relocation)
    /// carries a `Fixup64[FIXUP64_SMI_HANDLER_IDTR]` slot holding the address of the
    /// `IA32_DESCRIPTOR` that core's SMI entry `lidt`s.
    ///
    /// `sm_base_array` is the per-CPU SMBASE array (`u64[number_of_cpus]`) from the `PassDown`
    /// HOB; `number_of_cpus` is its length.
    fn patch_smi_handler_idt<S: SmiHandlerIdtPatchServices>(
        sm_base_array: u64,
        number_of_cpus: u64,
        mmi_entry_size: u64,
        services: &mut S,
    ) {
        let inputs = match validate_smi_handler_idt_patch_inputs(
            sm_base_array,
            number_of_cpus,
            mmi_entry_size,
            |address, size| services.is_inside_mmram(address, size),
        ) {
            Ok(inputs) => inputs,
            Err(error) => {
                log::warn!("Cannot patch SMI handler IDT: {error:?}");
                return;
            }
        };

        let idtr = services.read_idtr();
        // Copy packed fields into aligned locals before formatting; taking a reference to a
        // field of a `packed(2)` struct (as `log::info!` would) is undefined behavior.
        let idtr_base = idtr.base;
        let idtr_limit = idtr.limit;

        // Read the SMBASE array as bytes so an unaligned producer address does not create an
        // invalid `&[u64]`.
        // SAFETY: `sm_base_array` is non-zero and references `sm_base_array_size` initialized
        // bytes in MMRAM, as validated above, for the duration of initialization.
        let sm_base_bytes =
            unsafe { core::slice::from_raw_parts(sm_base_array as *const u8, inputs.sm_base_array_size) };

        for (cpu, smbase_bytes) in sm_base_bytes.chunks_exact(core::mem::size_of::<u64>()).enumerate() {
            let smbase = u64::from_ne_bytes(smbase_bytes.try_into().expect("SMBASE chunks are exactly 8 bytes"));
            if smbase == 0 {
                log::warn!("CPU {cpu}: SMBASE is 0, skipping SMI handler IDT patch");
                continue;
            }

            let Some(mmi_entry_base) = smbase.checked_add(SMM_HANDLER_OFFSET) else {
                log::error!("CPU {cpu}: SMBASE 0x{smbase:016x} overflows the SMI handler address");
                continue;
            };
            if !services.is_inside_mmram(mmi_entry_base, inputs.mmi_entry_size_u64) {
                log::error!(
                    "CPU {cpu}: SMI handler at 0x{mmi_entry_base:016x} with size 0x{:x} is not inside MMRAM",
                    inputs.mmi_entry_size
                );
                continue;
            }

            // SAFETY: the range check above establishes that the initialized SMI handler template
            // is fully contained in MMRAM.
            let mmi_entry = unsafe { core::slice::from_raw_parts(mmi_entry_base as *const u8, inputs.mmi_entry_size) };
            let idt_desc_addr = match parse_smi_handler_idt_descriptor(mmi_entry) {
                Ok(address) => address,
                Err(error) => {
                    log::error!("CPU {cpu}: invalid SMI handler fixup metadata: {error:?}");
                    continue;
                }
            };

            if idt_desc_addr == 0 {
                log::warn!("CPU {cpu}: Fixup64[{FIXUP64_SMI_HANDLER_IDTR}] (SMI_HANDLER_IDTR) is null");
                continue;
            }
            if !services.is_inside_mmram(idt_desc_addr, core::mem::size_of::<DescriptorTablePointer>() as u64) {
                log::error!("CPU {cpu}: SMI handler IDT descriptor at 0x{idt_desc_addr:016x} is not inside MMRAM");
                continue;
            }

            // SAFETY: the range check above establishes that the destination is a complete
            // writable descriptor in MMRAM.
            unsafe { services.write_idtr(idt_desc_addr, idtr) };

            log::info!(
                "CPU {cpu}: patched SMI handler IDT descriptor at 0x{idt_desc_addr:016x}: base=0x{idtr_base:016x}, limit=0x{idtr_limit:04x}"
            );
        }
    }

    /// Per-core initialization.
    ///
    /// This is called on every core (BSP and APs) during the first entry.
    /// Use this for setting up per-CPU state like syscall MSRs, GS base, etc.
    pub(crate) fn per_core_init(&'static self, cpu_id: u32, is_bsp: bool) {
        let core_type = if is_bsp { "BSP" } else { "AP" };
        log::trace!("{core_type} (CPU {cpu_id}) performing per-core initialization...");

        // IA32_SMM_MONITOR_CTL is per-logical-processor, so every core programs it.
        Self::program_mseg_base(cpu_id);

        let range =
            init_state().smrr_range().expect("SMRR range must be determined during BSP init before per-core init");
        smrr_initialize(range);
        configure_smm_code_access();

        log::trace!("{core_type} (CPU {cpu_id}) per-core initialization complete.");
    }

    /// Programs this logical processor's `IA32_SMM_MONITOR_CTL` with the MSEG base.
    ///
    /// The MSEG base discovered from the MSEG SMRAM HOB is written along with the
    /// Valid bit so an STM can later be activated, and so software can read the region back.
    ///
    /// No-op when the platform publishes no MSEG SMRAM HOB.
    fn program_mseg_base(cpu_id: u32) {
        let Some(mseg_base) = init_state().mseg_base() else {
            return;
        };

        let value = (mseg_base & SMM_MONITOR_CTL_MSEG_BASE_MASK) | SMM_MONITOR_CTL_VALID;

        if (get_current_cpu_id().ecx & (1 << 5)) == 0 {
            log::warn!("CPU {cpu_id} does not support VMX (CPUID.01H:ECX.VMX=0), cannot program IA32_SMM_MONITOR_CTL");
            return;
        }

        // SAFETY: IA32_SMM_MONITOR_CTL is an architectural MSR available whenever VMX is
        // reported by CPUID.01H:ECX.VMX. After the check above, only the architecturally
        // defined Valid and MsegBase fields are set; reserved bits are masked off above,
        // so the write cannot #GP on a reserved-bit violation. The write affects only this
        // logical processor's MSR.
        unsafe { write_msr(IA32_SMM_MONITOR_CTL_MSR, value) };
        log::debug!("CPU {cpu_id} programmed IA32_SMM_MONITOR_CTL = 0x{value:x}");
    }

    /// Initializes services from the HOB list.
    ///
    /// Discovers and processes the following HOBs in sequence:
    /// 1. `MM_SUPV_PASS_DOWN_HOB_GUID` — policy gate, syscall interface, memory policy, IDT patching
    /// 2. `MM_COMMON_REGION_HOB_GUID` — supervisor communication buffer
    /// 3. `MM_COMM_BUFFER_HOB_GUID` — user communication buffer + status buffer
    ///
    /// Finally, allocates the supervisor-to-user data buffer and stores the
    /// assembled [`CommBufferConfig`].
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `hob_list` points to a valid HOB list.
    unsafe fn init_policy_from_hob_list<S: PolicyInitServices>(
        &self,
        hob_list: *const c_void,
        services: &mut S,
    ) -> Result<(), PolicyInitError> {
        if hob_list.is_null() {
            return Err(PolicyInitError::NullHobList);
        }

        // SAFETY: `hob_list` was checked non-null above and, per this function's contract, points
        // to a valid HOB list, so taking a shared reference to the handoff table header is sound.
        let hob_list_info =
            unsafe { (hob_list as *const PhaseHandoffInformationTable).as_ref().ok_or(PolicyInitError::NullHobList)? };

        // 1. Process the MP Information HOB (`gMpInformationHobGuid`) for the CPU count. It sizes
        //    the Ring 3 stack array the PassDown HOB describes, so it is needed first.
        let mp_information =
            find_guid_hob(hob_list_info, crate::MP_INFORMATION_HOB_GUID).ok_or(PolicyInitError::HobNotFound)?;
        let number_of_cpus = self.parse_mp_information_hob(mp_information)?;

        // 1b. Process the PassDown HOB (policy, syscall, memory policy)
        let pass_down_data =
            find_guid_hob(hob_list_info, crate::MM_SUPV_PASS_DOWN_HOB_GUID).ok_or(PolicyInitError::HobNotFound)?;
        // SAFETY: `pass_down_data` is a slice into the validated HOB list, so the buffer pointers
        // it carries reference live memory as `init_from_pass_down_hob` requires.
        let (sm_base, mmi_entry_size) = unsafe { services.init_from_pass_down_hob(pass_down_data, number_of_cpus)? };

        services.set_save_state_info(SaveStateInfo { number_of_cpus, sm_base });
        log::info!("Save-state metadata initialized for {number_of_cpus} CPU(s)");

        // 1b-ii. Process the MSEG SMRAM HOB (`gMsegSmramGuid`), if published. It carries the
        //        MSEG region reserved for an STM. Each core programs the base into
        //        IA32_SMM_MONITOR_CTL during per-core init. Platforms without STM/SEA
        //        integration do not publish this HOB, so its absence is not an error.
        match find_guid_hob(hob_list_info, crate::MSEG_SMRAM_HOB_GUID).and_then(parse_mseg_smram_hob) {
            Some(mseg_base) => {
                services.set_mseg_base(mseg_base);
                log::info!("MSEG base 0x{mseg_base:x} discovered from MSEG SMRAM HOB");
            }
            _ => log::warn!("No usable MSEG SMRAM HOB; IA32_SMM_MONITOR_CTL will not be programmed"),
        }

        // 1c. Patch every core's SMI-handler IDT descriptor to the Rust IDT now that the
        //     CPU count is known (the SMI entry blocks were already copied per SMBASE, so
        //     each core must be patched, not just the BSP).
        services.patch_smi_handler_idt(sm_base, number_of_cpus, mmi_entry_size);

        // 2. Process the supervisor communication buffer HOB. Only one
        //    MM_COMM_REGION_HOB is published (the supervisor one); the user
        //    channel flows through MM_COMM_BUFFER_HOB_GUID below.
        let supv_region_data =
            find_guid_hob(hob_list_info, crate::MM_COMMON_REGION_HOB_GUID).ok_or(PolicyInitError::HobNotFound)?;
        let (supv_comm_buffer, supv_comm_buffer_size, supv_comm_buffer_internal, supv_status_buffer) =
            services.init_supv_comm_buffer(supv_region_data)?;

        // 3. Process the user communication buffer HOB. This still uses the
        //    legacy `MM_COMM_BUFFER_HOB_GUID` so the user core's own HOB walk
        //    keeps working (see the HACKHACK at the tail of
        //    init_user_comm_buffer).
        let (user_buffer_data, user_buffer_data_len) = {
            let data = find_guid_hob(hob_list_info, MM_COMM_BUFFER_HOB_GUID).ok_or(PolicyInitError::HobNotFound)?;
            (data.as_ptr().cast_mut(), data.len())
        };
        // SAFETY: the pointer and length identify the original HOB payload in the writable live
        // HOB list. The shared slice used to locate it is no longer used while it is rewritten.
        let (user_comm_buffer, user_comm_buffer_size, user_comm_buffer_internal, user_status_buffer) =
            unsafe { services.init_user_comm_buffer(user_buffer_data, user_buffer_data_len)? };

        // 4. Allocate the supervisor-to-user data buffer
        let supv_to_user_buffer = services.allocate_supv_to_user_buffer()?;

        // Validate all buffers are non-zero
        if supv_comm_buffer == 0
            || user_comm_buffer == 0
            || user_status_buffer == 0
            || supv_status_buffer == 0
            || supv_to_user_buffer == 0
        {
            log::error!("One or more communication buffers are not properly initialized");
            return Err(PolicyInitError::MissingCommunicationBuffer);
        }

        // Store the assembled communication buffer configuration
        services.set_comm_buffer_config(CommBufferConfig {
            supv_comm_buffer,
            supv_comm_buffer_internal,
            supv_comm_buffer_size,
            user_comm_buffer,
            user_comm_buffer_internal,
            user_comm_buffer_size,
            user_status_buffer,
            supv_status_buffer,
            supv_to_user_buffer,
            supv_to_user_buffer_size: UEFI_PAGE_SIZE as u64,
        });
        log::info!(
            "Comm buffers: supv=0x{supv_comm_buffer:x}/0x{supv_comm_buffer_internal:x} size=0x{supv_comm_buffer_size:x} status=0x{supv_status_buffer:x}, user=0x{user_comm_buffer:x}/0x{user_comm_buffer_internal:x} size=0x{user_comm_buffer_size:x} status=0x{user_status_buffer:x}"
        );

        Ok(())
    }

    /// Returns the CPU count from the MP Information HOB.
    fn parse_mp_information_hob(&self, data: &[u8]) -> Result<u64, PolicyInitError> {
        /// Offset of `ProcessorInfoBuffer[]` within `MP_INFORMATION_HOB_DATA`.
        const PROCESSOR_INFO_BUFFER_OFFSET: usize = 16;

        if data.len() < PROCESSOR_INFO_BUFFER_OFFSET {
            log::error!("MP Information HOB too small: {} < {}", data.len(), PROCESSOR_INFO_BUFFER_OFFSET);
            return Err(PolicyInitError::InvalidPolicyData);
        }

        let number_of_cpus = u64::from_le_bytes(
            data.get(0..8)
                .ok_or(PolicyInitError::InvalidPolicyData)?
                .try_into()
                .map_err(|_| PolicyInitError::InvalidPolicyData)?,
        );
        let cpu_count: usize = number_of_cpus
            .try_into()
            .map_err(|_| PolicyInitError::InvalidCpuCount { found: number_of_cpus, maximum: MAX_CPUS })?;
        if cpu_count == 0 || cpu_count > MAX_CPUS {
            log::error!("MP Information HOB CPU count {cpu_count} is outside the supported range 1..={MAX_CPUS}");
            return Err(PolicyInitError::InvalidCpuCount { found: number_of_cpus, maximum: MAX_CPUS });
        }

        let processor_info_size =
            cpu_count.checked_mul(PROCESSOR_INFO_ENTRY_SIZE).ok_or(PolicyInitError::InvalidPolicyData)?;
        let processor_info_end =
            PROCESSOR_INFO_BUFFER_OFFSET.checked_add(processor_info_size).ok_or(PolicyInitError::InvalidPolicyData)?;
        data.get(PROCESSOR_INFO_BUFFER_OFFSET..processor_info_end).ok_or(PolicyInitError::InvalidPolicyData)?;

        Ok(number_of_cpus)
    }

    /// Processes the MM Supervisor `PassDown` HOB.
    ///
    /// Handles: revision validation, per-core buffer setup,
    /// policy gate initialization, syscall interface setup, memory policy walk,
    /// and unblocked memory tracker initialization.
    ///
    /// Returns `(sm_base_array, mmi_entry_size)` carried by the HOB.
    ///
    /// ## Safety
    ///
    /// The buffer pointers carried in `data` (e.g. the firmware policy buffer)
    /// must reference valid memory for their declared sizes and remain resident
    /// for the supervisor's lifetime, as they are dereferenced during setup and
    /// runtime.
    unsafe fn init_from_pass_down_hob(&self, data: &[u8], number_of_cpus: u64) -> Result<(u64, u64), PolicyInitError> {
        let pass_down = parse_pass_down_hob(data)?;

        let mm_initialized_buffer = pass_down.mm_initialized_buffer;
        let firmware_policy_buffer = pass_down.mm_supv_firmware_policy_buffer;
        let cpl3_stack_buffer = pass_down.mm_supervisor_cpl3_stack_base;
        let cpl3_stack_buffer_size = pass_down.mm_supervisor_cpl3_per_core_stack_size;
        let mmi_entry_size = pass_down.mmi_entrypoint_size;
        let sm_base = pass_down.sm_base;

        // Store bounded per-core initialized slots.
        if mm_initialized_buffer != 0 {
            let cpu_count: usize = number_of_cpus
                .try_into()
                .map_err(|_| PolicyInitError::InvalidCpuCount { found: number_of_cpus, maximum: MAX_CPUS })?;
            if cpu_count == 0 || cpu_count > MAX_CPUS {
                return Err(PolicyInitError::InvalidCpuCount { found: number_of_cpus, maximum: MAX_CPUS });
            }
            if !is_buffer_inside_mmram(mm_initialized_buffer, number_of_cpus) {
                log::error!(
                    "MM initialized buffer at 0x{mm_initialized_buffer:016x} does not contain {cpu_count} slot(s) in MMRAM"
                );
                return Err(PolicyInitError::InvalidPolicyData);
            }
            let buffer_address =
                usize::try_from(mm_initialized_buffer).map_err(|_| PolicyInitError::InvalidPolicyData)?;
            let buffer_ptr = core::ptr::with_exposed_provenance::<AtomicU8>(buffer_address);
            // SAFETY: The PassDown HOB was validated before this routine is called and this
            // function's contract requires its buffer pointers to remain valid. The validated
            // CPU count is the number of one-byte initialized slots supplied by the MM IPL.
            let initialized_slots = unsafe { core::slice::from_raw_parts(buffer_ptr, cpu_count) };
            init_state().set_mm_initialized_buffer(initialized_slots);
            log::info!("MM Initialized buffer set to 0x{mm_initialized_buffer:016x} with {cpu_count} slot(s)");
        } else {
            log::warn!("MM Initialized buffer is null in PassDown HOB");
        }

        // Log the per-CPU SMBASE array passed down for the save-state read syscall.
        if sm_base != 0 {
            log::info!("CPU SMBASE array at 0x{sm_base:016x}");
        } else {
            log::warn!("CPU SMBASE array pointer is null in PassDown HOB");
        }

        let policy_ptr = firmware_policy_buffer as *const u8;
        let memory_policy_buffer = security_state().page_allocator().allocate_pages(1).map_err(|e| {
            log::error!("Failed to allocate page for memory policy buffer: {e:?}");
            PolicyInitError::MemoryAllocationFailed
        })?;

        // SAFETY: `policy_ptr` is the firmware policy buffer from the PassDown HOB, validated
        // non-zero above, and stays resident for the supervisor's lifetime.
        match unsafe { PolicyGate::new(policy_ptr) } {
            Ok(mut gate) => {
                log::info!("Policy gate initialized successfully");
                // SAFETY: `policy_ptr` is the same valid, resident firmware policy buffer.
                unsafe { dump_policy(policy_ptr) };

                let mem_policy_max_count = UEFI_PAGE_SIZE / core::mem::size_of::<MemDescriptorV1_0>();
                gate.set_memory_policy_buffer(memory_policy_buffer as *mut MemDescriptorV1_0, mem_policy_max_count);
                security_state().set_policy_gate(gate);
            }
            Err(e) => {
                log::error!("Failed to create policy gate: {e:?}");
                return Err(PolicyInitError::InvalidPolicyData);
            }
        }

        // Initialize syscall interface. The CPU count bounds `get_cpl3_stack`, so it must be the
        // count the MM IPL sized the stack array for, not the supervisor's `MAX_CPUS` capacity.
        self.syscall_interface
            .init(
                number_of_cpus.try_into().unwrap_or_else(|err| panic!("Invalid CPU count: {err:?}")),
                cpl3_stack_buffer,
                cpl3_stack_buffer_size
                    .try_into()
                    .unwrap_or_else(|err| panic!("Invalid CPL3 stack buffer size: {err:?}")),
            )
            .unwrap_or_else(|err| panic!("Failed to initialize syscall interface: {err:?}"));

        // Done before the policy walk below so the generated descriptors see the final attributes.
        self.map_cpl3_stacks_to_user(cpl3_stack_buffer, cpl3_stack_buffer_size, number_of_cpus);

        // Walk page table and generate memory policy
        let cr3 = read_cr3();
        // SAFETY: `cr3` is read from the active control register, so it points to the live PML4
        // table, and `memory_policy_buffer` is the page just allocated above with room for
        // `UEFI_PAGE_SIZE` bytes of descriptors.
        let count = unsafe {
            walk_page_table(cr3, memory_policy_buffer as *mut MemDescriptorV1_0, UEFI_PAGE_SIZE, is_buffer_inside_mmram)
        };

        if let Ok(descriptor_count) = count {
            log::info!("Successfully generated {descriptor_count} memory policy descriptors");
            // SAFETY: `walk_page_table` succeeded, so `memory_policy_buffer` holds `descriptor_count`
            // valid `MemDescriptorV1_0` entries.
            if let Err(e) = unsafe {
                security_state()
                    .unblocked_tracker()
                    .init_from_buffer(memory_policy_buffer as *const MemDescriptorV1_0, descriptor_count)
            } {
                log::error!("Failed to initialize unblocked memory tracker: {e:?}");
            } else {
                log::info!("Unblocked memory tracker initialized");
                security_state().unblocked_tracker().dump_regions();
            }
        } else {
            log::error!("Failed to generate memory policy descriptors: {:?}", count.err());
        }

        log::info!("Generated {} memory policy descriptors", count.unwrap_or(0));
        Ok((sm_base, mmi_entry_size))
    }
}

fn parse_pass_down_hob(data: &[u8]) -> Result<MmSupvPassDownHobData, PolicyInitError> {
    let (pass_down, _) = MmSupvPassDownHobData::read_from_prefix(data).map_err(|_| {
        log::error!("PassDown HOB data too small: {} < {}", data.len(), core::mem::size_of::<MmSupvPassDownHobData>());
        PolicyInitError::InvalidPolicyData
    })?;

    if pass_down.revision != crate::MM_SUPV_PASS_DOWN_HOB_REVISION {
        log::error!(
            "Invalid PassDown HOB revision: {} (expected {})",
            pass_down.revision,
            crate::MM_SUPV_PASS_DOWN_HOB_REVISION
        );
        return Err(PolicyInitError::InvalidRevision {
            found: pass_down.revision,
            expected: crate::MM_SUPV_PASS_DOWN_HOB_REVISION,
        });
    }

    if pass_down.mm_supv_firmware_policy_buffer == 0 || pass_down.mm_supv_firmware_policy_buffer_size == 0 {
        log::error!("Firmware policy buffer is null or empty");
        return Err(PolicyInitError::NullFirmwarePolicyBuffer);
    }

    if pass_down.mm_supv_firmware_policy_buffer.checked_add(pass_down.mm_supv_firmware_policy_buffer_size).is_none() {
        log::error!("Firmware policy buffer address range overflows");
        return Err(PolicyInitError::InvalidPolicyData);
    }

    Ok(pass_down)
}

fn find_user_module_entry<'a>(hobs: impl IntoIterator<Item = Hob<'a>>) -> Option<u64> {
    for current_hob in hobs {
        if let Hob::MemoryAllocationModule(mem_alloc_mod) = current_hob
            && mem_alloc_mod.alloc_descriptor.name == MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID
        {
            log::debug!(
                "Found MM Supervisor module HOB: module_name={:?}, entry_point=0x{:016x}",
                mem_alloc_mod.module_name,
                mem_alloc_mod.entry_point
            );

            if mem_alloc_mod.module_name == MM_SUPERVISOR_USER_GUID {
                log::info!(
                    "Found MM User module: entry_point=0x{:016x}, base=0x{:016x}, size=0x{:x}",
                    mem_alloc_mod.entry_point,
                    mem_alloc_mod.alloc_descriptor.memory_base_address,
                    mem_alloc_mod.alloc_descriptor.memory_length
                );
                return Some(mem_alloc_mod.entry_point);
            }
        }
    }

    None
}

/// Finds the first GUID HOB matching `target_guid` and returns its data slice.
///
/// Returns `None` if no matching HOB is found.
pub(crate) fn find_guid_hob(
    hob_list_info: &PhaseHandoffInformationTable,
    target_guid: patina::BinaryGuid,
) -> Option<&[u8]> {
    find_guid_hob_in(&Hob::Handoff(hob_list_info), target_guid)
}

fn find_guid_hob_in<'a>(hobs: impl IntoIterator<Item = Hob<'a>>, target_guid: patina::BinaryGuid) -> Option<&'a [u8]> {
    for current_hob in hobs {
        if let Hob::GuidHob(guid_hob, data) = current_hob
            && guid_hob.name == target_guid
        {
            return Some(data);
        }
    }
    None
}

/// Parses the MSEG SMRAM HOB payload (`gMsegSmramGuid`), a single
/// [`SmramDescriptor`] describing the MSEG region carved out of SMRAM.
///
/// Returns the MSEG base address, or `None` if the region is empty.
fn parse_mseg_smram_hob(data: &[u8]) -> Option<u64> {
    // `read_from_prefix` validates the length and copies the bytes out, so it imposes no
    // alignment or validity precondition on the HOB buffer and needs no `unsafe`.
    let (descriptor, _) = SmramDescriptor::read_from_prefix(data)
        .inspect_err(|_| {
            log::error!("MSEG SMRAM HOB too small: {} < {}", data.len(), core::mem::size_of::<SmramDescriptor>());
        })
        .ok()?;

    if descriptor.physical_size == 0 {
        log::warn!("MSEG SMRAM HOB describes an empty region");
        return None;
    }

    let base = descriptor.cpu_start;
    if base & !SMM_MONITOR_CTL_MSEG_BASE_MASK != 0 {
        log::error!("MSEG base 0x{base:x} is not 4 KiB aligned or lies above 4 GiB");
        return None;
    }

    Some(base)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedCommBuffer {
    address: u64,
    page_count: usize,
    size: u64,
    status_address: u64,
}

fn parse_comm_buffer_fields(
    address: u64,
    pages: u64,
    status_address: u64,
    description: &str,
) -> Result<ParsedCommBuffer, PolicyInitError> {
    let page_count = usize::try_from(pages).map_err(|_| {
        log::error!("{description} page count {pages} does not fit the target architecture");
        PolicyInitError::InvalidCommunicationBufferSize { pages }
    })?;
    let size = pages.checked_mul(UEFI_PAGE_SIZE as u64).filter(|size| *size != 0).ok_or_else(|| {
        log::error!("{description} page count {pages} produces an invalid byte size");
        PolicyInitError::InvalidCommunicationBufferSize { pages }
    })?;
    if address.checked_add(size).is_none() {
        log::error!("{description} address 0x{address:016x} plus size 0x{size:x} overflows");
        return Err(PolicyInitError::InvalidCommunicationBufferSize { pages });
    }

    Ok(ParsedCommBuffer { address, page_count, size, status_address })
}

fn parse_supv_comm_buffer_hob(data: &[u8]) -> Result<ParsedCommBuffer, PolicyInitError> {
    let (hob, _) = MmCommonRegionHobData::read_from_prefix(data).map_err(|_| {
        log::error!(
            "MM Common Region HOB data too small: {} < {}",
            data.len(),
            core::mem::size_of::<MmCommonRegionHobData>()
        );
        PolicyInitError::InvalidPolicyData
    })?;

    parse_comm_buffer_fields(hob.addr, hob.number_of_pages, hob.status_addr, "Supervisor communication buffer")
}

fn parse_user_comm_buffer_hob(data: &[u8]) -> Result<ParsedCommBuffer, PolicyInitError> {
    let (hob, _) = MmCommonBufferHobData::read_from_prefix(data).map_err(|_| {
        log::error!(
            "MM Communication Buffer HOB data too small: {} < {}",
            data.len(),
            core::mem::size_of::<MmCommonBufferHobData>()
        );
        PolicyInitError::InvalidPolicyData
    })?;

    parse_comm_buffer_fields(hob.physical_start, hob.number_of_pages, hob.status_buffer, "User communication buffer")
}

/// Processes the supervisor communication buffer HOB (`MM_COMMON_REGION_HOB_GUID`).
///
/// Returns `(buffer_addr, buffer_size, internal_copy_addr, status_buffer_addr)`.
fn init_supv_comm_buffer(data: &[u8]) -> Result<(u64, u64, u64, u64), PolicyInitError> {
    log::info!("Found MM Common Region HOB (supervisor)");

    let buffer = parse_supv_comm_buffer_hob(data)?;

    // Validate ownership if outside MMRAM
    if !is_buffer_inside_mmram(buffer.address, buffer.size) {
        match query_address_ownership(buffer.address, buffer.size) {
            Some(PageOwnership::Supervisor) => { /* expected */ }
            Some(PageOwnership::User) => {
                panic!(
                    "Supervisor common buffer at 0x{:016x}-0x{:016x} is not marked as supervisor-owned",
                    buffer.address,
                    buffer.address + buffer.size
                );
            }
            None => {
                panic!("Failed to query page ownership for supervisor common buffer at 0x{:016x}", buffer.address);
            }
        }
    }

    // Allocate internal copy
    let supv_comm_buffer_internal = security_state()
        .page_allocator()
        .allocate_pages_with_type(buffer.page_count, AllocationType::Supervisor)
        .map_err(|e| {
            log::error!("Failed to allocate internal supervisor common buffer: {e:?}");
            PolicyInitError::MemoryAllocationFailed
        })?;

    Ok((buffer.address, buffer.size, supv_comm_buffer_internal, buffer.status_address))
}

/// Processes the user communication buffer HOB (`MM_COMM_BUFFER_HOB_GUID`).
///
/// Returns `(buffer_addr, buffer_size, internal_copy_addr, status_buffer_addr)`.
///
/// ## Safety
///
/// `data` must be non-null and point to `data_len` readable, writable bytes in the
/// original HOB buffer. No references to those bytes may be live while this
/// function runs because `physical_start` is overwritten in place.
unsafe fn init_user_comm_buffer(data: *mut u8, data_len: usize) -> Result<(u64, u64, u64, u64), PolicyInitError> {
    log::info!("Found MM Communication Buffer HOB");

    let buffer = {
        // SAFETY: the caller guarantees that `data` points to `data_len` readable bytes. The
        // temporary shared slice is dropped before the payload is rewritten below.
        let bytes = unsafe { core::slice::from_raw_parts(data.cast_const(), data_len) };
        parse_user_comm_buffer_hob(bytes)?
    };

    // Validate ownership if outside MMRAM
    if !is_buffer_inside_mmram(buffer.address, buffer.size) {
        match query_address_ownership(buffer.address, buffer.size) {
            Some(PageOwnership::Supervisor) => { /* expected */ }
            Some(PageOwnership::User) => {
                panic!(
                    "User common buffer at 0x{:016x}-0x{:016x} is not marked as user-owned",
                    buffer.address,
                    buffer.address + buffer.size
                );
            }
            None => {
                panic!("Failed to query page ownership for user common buffer at 0x{:016x}", buffer.address);
            }
        }
    }

    // Validate status buffer
    if !is_buffer_inside_mmram(buffer.status_address, core::mem::size_of::<MmCommBufferStatus>() as u64) {
        match query_address_ownership(buffer.status_address, core::mem::size_of::<MmCommBufferStatus>() as u64) {
            Some(PageOwnership::Supervisor) => { /* expected */ }
            Some(PageOwnership::User) => {
                panic!("Status buffer at 0x{:016x} is not marked as supervisor-exposed", buffer.status_address);
            }
            None => {
                panic!("Failed to query page ownership for status buffer at 0x{:016x}", buffer.status_address);
            }
        }
    }

    // Allocate internal copy
    let user_comm_buffer_internal = security_state()
        .page_allocator()
        .allocate_pages_with_type(buffer.page_count, AllocationType::User)
        .map_err(|e| {
            log::error!("Failed to allocate internal user common buffer: {e:?}");
            PolicyInitError::MemoryAllocationFailed
        })?;

    // TODO: Remove the logic that overwrites the HOB's physical_start with the internal buffer address
    // so the user module sees it after demotion.
    // SAFETY:
    // - `data` points to the original writable HOB buffer and parsing above proved it contains
    //   `MmCommonBufferHobData`, so `physical_start` lies within the allocation. `addr_of_mut!`
    //   avoids forming a reference to the packed field, and `write_volatile` keeps the store from
    //   being elided.
    // - The HOB pages may be mapped read-only, so `disable_write_protection` clears `CR0.WP` to
    //   permit the supervisor store. This runs in Ring 0 during single-threaded BSP init in the MM
    //   (SMM) environment, where interrupts are masked, satisfying the privilege/atomicity
    //   requirements of `disable_write_protection`. `enable_write_protection` is handed exactly the
    //   value returned by `disable_write_protection`, restoring `CR0.WP` before this block returns
    //   and bounding the unprotected window to the single field write.
    unsafe {
        let hob_ptr = data.cast::<MmCommonBufferHobData>();
        let original_cr0 = disable_write_protection();
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*hob_ptr).physical_start), user_comm_buffer_internal);
        enable_write_protection(original_cr0);
    }

    Ok((buffer.address, buffer.size, user_comm_buffer_internal, buffer.status_address))
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use core::{alloc::Layout, mem::size_of, ptr::NonNull};
    use std::{
        alloc::{alloc_zeroed, dealloc, handle_alloc_error},
        panic::{AssertUnwindSafe, catch_unwind},
    };

    use patina::{
        management_mode::supervisor::MM_SUPERVISOR_CORE_GUID,
        pi::BootMode,
        pi::hob::{
            END_OF_HOB_LIST, GUID_EXTENSION, GuidHob, HANDOFF, HobHeader, MEMORY_ALLOCATION, MemoryAllocationHeader,
            MemoryAllocationModule, PhaseHandoffInformationTable,
        },
        standard::efi,
    };

    use crate::{
        mem::{
            PageAllocator, PagingPoolAllocator,
            page_allocator::{SMM_SMRAM_MEMORY_GUID, SmramReserveHobData},
        },
        state::InitState,
    };

    struct TestPlatform;

    impl PlatformInfo for TestPlatform {}

    struct RawHobList {
        storage: Vec<u64>,
        byte_len: usize,
    }

    impl RawHobList {
        fn new() -> Self {
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

        fn push_struct<T>(&mut self, value: T) {
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

        fn push_guid_hob(&mut self, name: patina::BinaryGuid, data: &[u8]) {
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

        fn finish(mut self) -> Self {
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

        fn as_ptr(&self) -> *const c_void {
            self.storage.as_ptr().cast()
        }
    }

    struct PageAlignedMemory {
        ptr: NonNull<u8>,
        layout: Layout,
    }

    impl PageAlignedMemory {
        fn new(pages: usize) -> Self {
            let layout =
                Layout::from_size_align(pages * UEFI_PAGE_SIZE, UEFI_PAGE_SIZE).expect("page allocation layout");
            // SAFETY: `layout` has non-zero size and valid page alignment.
            let ptr = NonNull::new(unsafe { alloc_zeroed(layout) }).unwrap_or_else(|| handle_alloc_error(layout));
            Self { ptr, layout }
        }

        fn base(&self) -> u64 {
            self.ptr.as_ptr() as u64
        }

        fn size(&self) -> u64 {
            self.layout.size() as u64
        }
    }

    impl Drop for PageAlignedMemory {
        fn drop(&mut self) {
            // SAFETY: `ptr` was allocated with this exact layout and has not been deallocated.
            unsafe { dealloc(self.ptr.as_ptr(), self.layout) };
        }
    }

    struct RecordingPolicyServices {
        calls: Vec<&'static str>,
        pass_down_result: Result<(u64, u64), PolicyInitError>,
        supv_result: Result<CommBufferInitResult, PolicyInitError>,
        user_result: Result<CommBufferInitResult, PolicyInitError>,
        allocation_result: Result<u64, PolicyInitError>,
        policy_validation: Option<Result<(), &'static str>>,
        pass_down_cpu_count: Option<u64>,
        save_state_info: Option<SaveStateInfo>,
        mseg_base: Option<u64>,
        patch_args: Option<(u64, u64, u64)>,
        config: Option<CommBufferConfig>,
    }

    impl RecordingPolicyServices {
        fn successful() -> Self {
            Self {
                calls: Vec::new(),
                pass_down_result: Ok((0xA000, 0xB000)),
                supv_result: Ok((0x1000, 0x2000, 0x3000, 0x4000)),
                user_result: Ok((0x5000, 0x6000, 0x7000, 0x8000)),
                allocation_result: Ok(0x9000),
                policy_validation: None,
                pass_down_cpu_count: None,
                save_state_info: None,
                mseg_base: None,
                patch_args: None,
                config: None,
            }
        }
    }

    impl PolicyInitServices for RecordingPolicyServices {
        type PolicyCheckError = &'static str;

        unsafe fn init_from_pass_down_hob(
            &mut self,
            data: &[u8],
            number_of_cpus: u64,
        ) -> Result<(u64, u64), PolicyInitError> {
            self.calls.push("pass_down");
            parse_pass_down_hob(data)?;
            self.pass_down_cpu_count = Some(number_of_cpus);
            self.pass_down_result
        }

        fn set_save_state_info(&mut self, info: SaveStateInfo) {
            self.calls.push("save_state");
            self.save_state_info = Some(info);
        }

        fn set_mseg_base(&mut self, base: u64) {
            self.calls.push("mseg");
            self.mseg_base = Some(base);
        }

        fn patch_smi_handler_idt(&mut self, sm_base: u64, number_of_cpus: u64, mmi_entry_size: u64) {
            self.calls.push("patch_idt");
            self.patch_args = Some((sm_base, number_of_cpus, mmi_entry_size));
        }

        fn init_supv_comm_buffer(&mut self, data: &[u8]) -> Result<CommBufferInitResult, PolicyInitError> {
            self.calls.push("supv_comm");
            parse_supv_comm_buffer_hob(data)?;
            self.supv_result
        }

        unsafe fn init_user_comm_buffer(
            &mut self,
            data: *mut u8,
            data_len: usize,
        ) -> Result<CommBufferInitResult, PolicyInitError> {
            self.calls.push("user_comm");
            // SAFETY: the policy initialization method provides the live HOB payload and length.
            let data = unsafe { core::slice::from_raw_parts(data.cast_const(), data_len) };
            parse_user_comm_buffer_hob(data)?;
            self.user_result
        }

        fn allocate_supv_to_user_buffer(&mut self) -> Result<u64, PolicyInitError> {
            self.calls.push("allocate");
            self.allocation_result
        }

        fn set_comm_buffer_config(&mut self, config: CommBufferConfig) {
            self.calls.push("config");
            self.config = Some(config);
        }

        fn validate_policy(&mut self) -> Option<Result<(), Self::PolicyCheckError>> {
            self.calls.push("validate");
            self.policy_validation
        }
    }

    struct RecordingSmiPatchServices {
        allowed_ranges: Vec<(u64, u64)>,
        idtr: DescriptorTablePointer,
        writes: Vec<(u64, u16, u64)>,
    }

    impl RecordingSmiPatchServices {
        fn new(allowed_ranges: Vec<(u64, u64)>) -> Self {
            Self {
                allowed_ranges,
                idtr: DescriptorTablePointer { limit: 0x1234, base: 0x5678_9ABC_DEF0_1234 },
                writes: Vec::new(),
            }
        }
    }

    impl SmiHandlerIdtPatchServices for RecordingSmiPatchServices {
        fn is_inside_mmram(&self, address: u64, size: u64) -> bool {
            self.allowed_ranges.contains(&(address, size))
        }

        fn read_idtr(&self) -> DescriptorTablePointer {
            self.idtr
        }

        unsafe fn write_idtr(&mut self, address: u64, idtr: DescriptorTablePointer) {
            let limit = idtr.limit;
            let base = idtr.base;
            self.writes.push((address, limit, base));
        }
    }

    fn pass_down_hob_data(pass_down: &MmSupvPassDownHobData) -> [u8; size_of::<MmSupvPassDownHobData>()] {
        let mut data = [0_u8; size_of::<MmSupvPassDownHobData>()];
        data[0..4].copy_from_slice(&pass_down.revision.to_ne_bytes());
        data[4..8].copy_from_slice(&pass_down.reserved.to_ne_bytes());

        let fields = [
            pass_down.mm_supervisor_cpl3_stack_base,
            pass_down.mm_supervisor_cpl3_per_core_stack_size,
            pass_down.sm_base,
            pass_down.mm_initialized_buffer,
            pass_down.mm_supv_firmware_policy_buffer,
            pass_down.mm_supv_firmware_policy_buffer_size,
            pass_down.mmi_entrypoint_size,
        ];
        for (index, field) in fields.into_iter().enumerate() {
            let offset = 8 + index * size_of::<u64>();
            data[offset..offset + size_of::<u64>()].copy_from_slice(&field.to_ne_bytes());
        }

        data
    }

    fn valid_pass_down_hob() -> MmSupvPassDownHobData {
        MmSupvPassDownHobData {
            revision: crate::MM_SUPV_PASS_DOWN_HOB_REVISION,
            reserved: 0,
            mm_supervisor_cpl3_stack_base: 0x10_0000,
            mm_supervisor_cpl3_per_core_stack_size: 0x4000,
            sm_base: 0x20_0000,
            mm_initialized_buffer: 0x30_0000,
            mm_supv_firmware_policy_buffer: 0x40_0000,
            mm_supv_firmware_policy_buffer_size: 0x2000,
            mmi_entrypoint_size: 0x100,
        }
    }

    fn supv_comm_buffer_hob_data(
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

    fn user_comm_buffer_hob_data(
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

    fn allocation_module(
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

    fn guid_hob(name: patina::BinaryGuid, data_len: usize) -> GuidHob {
        GuidHob {
            header: HobHeader { r#type: GUID_EXTENSION, length: (size_of::<GuidHob>() + data_len) as u16, reserved: 0 },
            name,
        }
    }

    fn mp_information_hob_data(number_of_cpus: usize) -> Vec<u8> {
        let mut data = vec![0_u8; 16 + number_of_cpus * PROCESSOR_INFO_ENTRY_SIZE];
        data[0..8].copy_from_slice(&(number_of_cpus as u64).to_le_bytes());
        data
    }

    fn policy_hob_list_with_cpu_count(number_of_cpus: usize, include_mseg: bool) -> RawHobList {
        let mut list = RawHobList::new();
        list.push_guid_hob(crate::MP_INFORMATION_HOB_GUID, &mp_information_hob_data(number_of_cpus));
        list.push_guid_hob(crate::MM_SUPV_PASS_DOWN_HOB_GUID, &pass_down_hob_data(&valid_pass_down_hob()));
        if include_mseg {
            list.push_guid_hob(crate::MSEG_SMRAM_HOB_GUID, &mseg_smram_hob_data(0x0040_0000, 0x0040_0000, 0x0002_0000));
        }
        list.push_guid_hob(crate::MM_COMMON_REGION_HOB_GUID, &supv_comm_buffer_hob_data(0x10_0000, 2, 0x20_0000));
        list.push_guid_hob(MM_COMM_BUFFER_HOB_GUID, &user_comm_buffer_hob_data(0x30_0000, 3, 0x40_0000));
        list.finish()
    }

    fn policy_hob_list(include_mseg: bool) -> RawHobList {
        policy_hob_list_with_cpu_count(2, include_mseg)
    }

    fn smram_hob_list(memory: &PageAlignedMemory) -> RawHobList {
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

    fn mmi_entry(fixup64_count: u8, idt_descriptor_address: u64) -> Vec<u8> {
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

    fn smi_handler_memory(entry: &[u8]) -> Vec<u64> {
        let byte_len = SMM_HANDLER_OFFSET as usize + entry.len();
        let mut memory = vec![0_u64; byte_len.div_ceil(size_of::<u64>())];
        // SAFETY: `memory` has at least `byte_len` writable bytes, and the source is
        // a distinct allocation.
        unsafe {
            core::ptr::copy_nonoverlapping(
                entry.as_ptr(),
                memory.as_mut_ptr().cast::<u8>().add(SMM_HANDLER_OFFSET as usize),
                entry.len(),
            );
        }
        memory
    }

    #[test]
    fn test_init_page_allocators_from_real_hob_list() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let memory = PageAlignedMemory::new(mem::DEFAULT_PAGING_POOL_PAGES + 8);
        let hob_list = smram_hob_list(&memory);
        let page_allocator = PageAllocator::new();
        let paging_allocator = PagingPoolAllocator::new();
        let state = InitState::new();

        // SAFETY: `hob_list` is a valid contiguous HOB list and its SMRAM descriptor
        // references the live, exclusively owned page-aligned `memory` allocation.
        let (regions, count) = unsafe {
            supervisor.init_page_allocators(hob_list.as_ptr(), &page_allocator, &paging_allocator, &state, |regions| {
                regions.first().copied()
            })
        };

        assert_eq!(count, 1);
        assert_eq!(regions[0], SmramRegion::new(memory.base(), memory.size(), false));
        assert_eq!(state.smrr_range(), Some(regions[0]));
        assert!(page_allocator.is_initialized());
        assert!(paging_allocator.is_initialized());
        assert_eq!(paging_allocator.free_page_count(), mem::DEFAULT_PAGING_POOL_PAGES);
    }

    #[test]
    fn test_init_page_allocators_rejects_hob_list_without_smram() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let hob_list = RawHobList::new().finish();
        let page_allocator = PageAllocator::new();
        let paging_allocator = PagingPoolAllocator::new();
        let state = InitState::new();

        let result = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: `hob_list` is a valid contiguous HOB list.
            unsafe {
                supervisor.init_page_allocators(
                    hob_list.as_ptr(),
                    &page_allocator,
                    &paging_allocator,
                    &state,
                    |regions| regions.first().copied(),
                );
            }
        }));

        assert!(result.is_err());
        assert!(!page_allocator.is_initialized());
        assert!(!paging_allocator.is_initialized());
    }

    #[test]
    fn test_init_page_allocators_rejects_missing_smrr_range() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let memory = PageAlignedMemory::new(mem::DEFAULT_PAGING_POOL_PAGES + 8);
        let hob_list = smram_hob_list(&memory);
        let page_allocator = PageAllocator::new();
        let paging_allocator = PagingPoolAllocator::new();
        let state = InitState::new();

        let result = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: `hob_list` and its SMRAM allocation remain valid for the call.
            unsafe {
                supervisor
                    .init_page_allocators(hob_list.as_ptr(), &page_allocator, &paging_allocator, &state, |_| None);
            }
        }));

        assert!(result.is_err());
        assert_eq!(state.smrr_range(), None);
        assert!(!paging_allocator.is_initialized());
    }

    #[test]
    fn test_init_page_allocators_reports_paging_pool_allocation_failure() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let memory = PageAlignedMemory::new(2);
        let hob_list = smram_hob_list(&memory);
        let page_allocator = PageAllocator::new();
        let paging_allocator = PagingPoolAllocator::new();
        let state = InitState::new();

        let result = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: `hob_list` and its SMRAM allocation remain valid for the call.
            unsafe {
                supervisor.init_page_allocators(
                    hob_list.as_ptr(),
                    &page_allocator,
                    &paging_allocator,
                    &state,
                    |regions| regions.first().copied(),
                );
            }
        }));

        assert!(result.is_err());
        assert!(page_allocator.is_initialized());
        assert!(!paging_allocator.is_initialized());
    }

    #[test]
    fn test_init_page_allocators_reports_paging_allocator_failure() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let existing_pool = PageAlignedMemory::new(mem::DEFAULT_PAGING_POOL_PAGES);
        let paging_allocator = PagingPoolAllocator::new();
        // SAFETY: `existing_pool` is page-aligned, exclusively owned, and remains alive.
        unsafe {
            paging_allocator
                .init(existing_pool.base(), mem::DEFAULT_PAGING_POOL_PAGES)
                .expect("pre-initialize paging allocator");
        }

        let memory = PageAlignedMemory::new(mem::DEFAULT_PAGING_POOL_PAGES + 8);
        let hob_list = smram_hob_list(&memory);
        let page_allocator = PageAllocator::new();
        let state = InitState::new();

        let result = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: `hob_list` and its SMRAM allocation remain valid for the call.
            unsafe {
                supervisor.init_page_allocators(
                    hob_list.as_ptr(),
                    &page_allocator,
                    &paging_allocator,
                    &state,
                    |regions| regions.first().copied(),
                );
            }
        }));

        assert!(result.is_err());
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

        // SAFETY: `hob_list` is a valid contiguous HOB list.
        unsafe { supervisor.discover_and_store_user_entry(hob_list.as_ptr(), &state) };

        assert_eq!(state.user_entry_point(), Some(0x1234_5678));
    }

    #[test]
    fn test_discover_and_store_user_entry_tolerates_missing_module() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let state = InitState::new();
        let hob_list = RawHobList::new().finish();

        // SAFETY: `hob_list` is a valid contiguous HOB list.
        unsafe { supervisor.discover_and_store_user_entry(hob_list.as_ptr(), &state) };

        assert_eq!(state.user_entry_point(), None);
    }

    #[test]
    fn test_init_policy_from_hob_list_runs_complete_flow() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let hob_list = policy_hob_list(true);
        let mut services = RecordingPolicyServices::successful();

        // SAFETY: `hob_list` is a valid contiguous HOB list whose payloads remain live.
        unsafe {
            supervisor
                .init_policy_from_hob_list(hob_list.as_ptr(), &mut services)
                .expect("complete policy HOB list should initialize");
        }

        assert_eq!(
            services.calls,
            ["pass_down", "save_state", "mseg", "patch_idt", "supv_comm", "user_comm", "allocate", "config"]
        );
        assert_eq!(services.pass_down_cpu_count, Some(2));
        let save_state = services.save_state_info.expect("save-state metadata should be stored");
        assert_eq!(save_state.number_of_cpus, 2);
        assert_eq!(save_state.sm_base, 0xA000);
        assert_eq!(services.mseg_base, Some(0x0040_0000));
        assert_eq!(services.patch_args, Some((0xA000, 2, 0xB000)));

        let config = services.config.expect("communication buffer configuration should be stored");
        assert_eq!(config.supv_comm_buffer, 0x1000);
        assert_eq!(config.supv_comm_buffer_size, 0x2000);
        assert_eq!(config.supv_comm_buffer_internal, 0x3000);
        assert_eq!(config.supv_status_buffer, 0x4000);
        assert_eq!(config.user_comm_buffer, 0x5000);
        assert_eq!(config.user_comm_buffer_size, 0x6000);
        assert_eq!(config.user_comm_buffer_internal, 0x7000);
        assert_eq!(config.user_status_buffer, 0x8000);
        assert_eq!(config.supv_to_user_buffer, 0x9000);
        assert_eq!(config.supv_to_user_buffer_size, UEFI_PAGE_SIZE as u64);
    }

    #[test]
    fn test_init_policy_from_hob_list_supports_absent_optional_mseg_hob() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let hob_list = policy_hob_list(false);
        let mut services = RecordingPolicyServices::successful();

        // SAFETY: `hob_list` is a valid contiguous HOB list.
        unsafe {
            supervisor.init_policy_from_hob_list(hob_list.as_ptr(), &mut services).expect("MSEG HOB is optional");
        }

        assert_eq!(services.mseg_base, None);
        assert!(!services.calls.contains(&"mseg"));
    }

    #[test]
    fn test_init_policy_from_hob_list_reports_null_and_missing_hobs() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let mut services = RecordingPolicyServices::successful();

        // SAFETY: null is explicitly rejected before dereference.
        let result = unsafe { supervisor.init_policy_from_hob_list(core::ptr::null(), &mut services) };
        assert_eq!(result, Err(PolicyInitError::NullHobList));

        let hob_list = RawHobList::new().finish();
        // SAFETY: `hob_list` is a valid contiguous HOB list.
        let result = unsafe { supervisor.init_policy_from_hob_list(hob_list.as_ptr(), &mut services) };
        assert_eq!(result, Err(PolicyInitError::HobNotFound));
        assert!(services.calls.is_empty());
    }

    #[test]
    fn test_init_policy_from_hob_list_propagates_service_failures() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();

        let hob_list = policy_hob_list(true);
        let mut pass_down_failure = RecordingPolicyServices::successful();
        pass_down_failure.pass_down_result = Err(PolicyInitError::InvalidPolicyData);
        // SAFETY: `hob_list` is a valid contiguous HOB list.
        let result = unsafe { supervisor.init_policy_from_hob_list(hob_list.as_ptr(), &mut pass_down_failure) };
        assert_eq!(result, Err(PolicyInitError::InvalidPolicyData));
        assert_eq!(pass_down_failure.calls, ["pass_down"]);

        let hob_list = policy_hob_list(true);
        let mut supervisor_buffer_failure = RecordingPolicyServices::successful();
        supervisor_buffer_failure.supv_result = Err(PolicyInitError::MemoryAllocationFailed);
        // SAFETY: `hob_list` is a valid contiguous HOB list.
        let result = unsafe { supervisor.init_policy_from_hob_list(hob_list.as_ptr(), &mut supervisor_buffer_failure) };
        assert_eq!(result, Err(PolicyInitError::MemoryAllocationFailed));
        assert_eq!(supervisor_buffer_failure.calls, ["pass_down", "save_state", "mseg", "patch_idt", "supv_comm"]);

        let hob_list = policy_hob_list(true);
        let mut user_buffer_failure = RecordingPolicyServices::successful();
        user_buffer_failure.user_result = Err(PolicyInitError::InvalidCommunicationBufferSize { pages: 0 });
        // SAFETY: `hob_list` is a valid contiguous HOB list.
        let result = unsafe { supervisor.init_policy_from_hob_list(hob_list.as_ptr(), &mut user_buffer_failure) };
        assert_eq!(result, Err(PolicyInitError::InvalidCommunicationBufferSize { pages: 0 }));
        assert_eq!(
            user_buffer_failure.calls,
            ["pass_down", "save_state", "mseg", "patch_idt", "supv_comm", "user_comm"]
        );

        let hob_list = policy_hob_list(true);
        let mut allocation_failure = RecordingPolicyServices::successful();
        allocation_failure.allocation_result = Err(PolicyInitError::MemoryAllocationFailed);
        // SAFETY: `hob_list` is a valid contiguous HOB list.
        let result = unsafe { supervisor.init_policy_from_hob_list(hob_list.as_ptr(), &mut allocation_failure) };
        assert_eq!(result, Err(PolicyInitError::MemoryAllocationFailed));
        assert_eq!(allocation_failure.calls.last(), Some(&"allocate"));
        assert!(allocation_failure.config.is_none());
    }

    #[test]
    fn test_init_policy_from_hob_list_rejects_zero_required_buffer() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let hob_list = policy_hob_list(true);
        let mut services = RecordingPolicyServices::successful();
        services.user_result = Ok((0, 0x6000, 0x7000, 0x8000));

        // SAFETY: `hob_list` is a valid contiguous HOB list.
        let result = unsafe { supervisor.init_policy_from_hob_list(hob_list.as_ptr(), &mut services) };
        assert_eq!(result, Err(PolicyInitError::MissingCommunicationBuffer));
        assert!(services.config.is_none());
    }

    #[test]
    fn test_init_policy_and_validate_runs_policy_validation() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let hob_list = policy_hob_list(true);
        let mut services = RecordingPolicyServices::successful();
        services.policy_validation = Some(Ok(()));

        // SAFETY: `hob_list` is a valid contiguous HOB list.
        unsafe { supervisor.init_policy_and_validate(hob_list.as_ptr(), &mut services) };

        assert_eq!(services.calls.last(), Some(&"validate"));
        assert!(services.config.is_some());
    }

    #[test]
    fn test_init_policy_and_validate_panics_on_initialization_failure() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let hob_list = RawHobList::new().finish();
        let mut services = RecordingPolicyServices::successful();
        services.policy_validation = Some(Ok(()));

        let result = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: `hob_list` is a valid contiguous HOB list.
            unsafe { supervisor.init_policy_and_validate(hob_list.as_ptr(), &mut services) };
        }));

        assert!(result.is_err());
        assert!(services.calls.is_empty());
    }

    #[test]
    fn test_init_policy_and_validate_panics_on_invalid_mp_cpu_count() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();

        for cpu_count in [0, 5] {
            let hob_list = policy_hob_list_with_cpu_count(cpu_count, true);
            let mut services = RecordingPolicyServices::successful();

            let result = catch_unwind(AssertUnwindSafe(|| {
                // SAFETY: `hob_list` is a valid contiguous HOB list.
                unsafe { supervisor.init_policy_and_validate(hob_list.as_ptr(), &mut services) };
            }));

            assert!(result.is_err(), "CPU count {cpu_count} should fail-stop initialization");
            assert!(services.calls.is_empty());
        }
    }

    #[test]
    fn test_init_policy_and_validate_tolerates_absent_policy() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let hob_list = policy_hob_list(true);
        let mut services = RecordingPolicyServices::successful();

        // SAFETY: `hob_list` is a valid contiguous HOB list.
        unsafe { supervisor.init_policy_and_validate(hob_list.as_ptr(), &mut services) };

        assert_eq!(services.calls.last(), Some(&"validate"));
    }

    #[test]
    #[should_panic(expected = "Security policy check failed during init")]
    fn test_init_policy_and_validate_panics_when_policy_check_fails() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let hob_list = policy_hob_list(true);
        let mut services = RecordingPolicyServices::successful();
        services.policy_validation = Some(Err("invalid policy"));

        // SAFETY: `hob_list` is a valid contiguous HOB list.
        unsafe { supervisor.init_policy_and_validate(hob_list.as_ptr(), &mut services) };
    }

    #[test]
    fn test_validate_smi_handler_idt_patch_inputs() {
        let inputs = validate_smi_handler_idt_patch_inputs(0x1000, 4, 0x200, |base, size| {
            base == 0x1000 && size == 4 * size_of::<u64>() as u64
        })
        .expect("valid patch inputs should pass");

        assert_eq!(
            inputs,
            SmiHandlerIdtPatchInputs {
                sm_base_array_size: 4 * size_of::<u64>(),
                mmi_entry_size: 0x200,
                mmi_entry_size_u64: 0x200,
            }
        );
    }

    #[test]
    fn test_validate_smi_handler_idt_patch_inputs_rejects_invalid_values() {
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(0x1000, 1, 0, |_, _| true),
            Err(SmiHandlerIdtPatchInputError::ZeroEntrySize)
        );
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(0, 1, 0x100, |_, _| true),
            Err(SmiHandlerIdtPatchInputError::MissingSmBaseArray)
        );
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(0x1000, 0, 0x100, |_, _| true),
            Err(SmiHandlerIdtPatchInputError::MissingSmBaseArray)
        );
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(0x1000, u64::MAX, 0x100, |_, _| true),
            Err(SmiHandlerIdtPatchInputError::SmBaseArraySizeOverflow)
        );
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(0x1000, 1, 0x100, |_, _| false),
            Err(SmiHandlerIdtPatchInputError::SmBaseArrayOutsideMmram)
        );
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(u64::MAX - 3, 1, 0x100, |_, _| true),
            Err(SmiHandlerIdtPatchInputError::SmBaseArrayOutsideMmram)
        );
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(0x1000, 1, isize::MAX as u64 + 1, |_, _| true),
            Err(SmiHandlerIdtPatchInputError::EntrySizeTooLarge)
        );
    }

    #[test]
    fn test_patch_smi_handler_idt_writes_valid_descriptor() {
        let descriptor_address = 0x1234_5000;
        let entry = mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, descriptor_address);
        let handler_memory = smi_handler_memory(&entry);
        let smbase = handler_memory.as_ptr() as u64;
        let sm_bases = [smbase];
        let sm_base_array = sm_bases.as_ptr() as u64;
        let mmi_entry_base = smbase + SMM_HANDLER_OFFSET;
        let mut services = RecordingSmiPatchServices::new(vec![
            (sm_base_array, size_of_val(&sm_bases) as u64),
            (mmi_entry_base, entry.len() as u64),
            (descriptor_address, size_of::<DescriptorTablePointer>() as u64),
        ]);

        MmSupervisorCore::<TestPlatform, 4>::patch_smi_handler_idt(
            sm_base_array,
            sm_bases.len() as u64,
            entry.len() as u64,
            &mut services,
        );

        assert_eq!(services.writes, [(descriptor_address, 0x1234, 0x5678_9ABC_DEF0_1234)]);
    }

    #[test]
    fn test_patch_smi_handler_idt_rejects_invalid_top_level_inputs() {
        let mut services = RecordingSmiPatchServices::new(Vec::new());

        MmSupervisorCore::<TestPlatform, 4>::patch_smi_handler_idt(0, 1, 0x100, &mut services);

        assert!(services.writes.is_empty());
    }

    #[test]
    fn test_patch_smi_handler_idt_skips_invalid_cpu_entries() {
        let zero_descriptor_entry = mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, 0);
        let malformed_entry = vec![0_u8; zero_descriptor_entry.len()];
        let malformed_memory = smi_handler_memory(&malformed_entry);
        let malformed_smbase = malformed_memory.as_ptr() as u64;
        let zero_descriptor_memory = smi_handler_memory(&zero_descriptor_entry);
        let zero_descriptor_smbase = zero_descriptor_memory.as_ptr() as u64;
        let outside_descriptor_entry = mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, 0xDEAD_0000);
        let outside_descriptor_memory = smi_handler_memory(&outside_descriptor_entry);
        let outside_descriptor_smbase = outside_descriptor_memory.as_ptr() as u64;
        let sm_bases = [
            0,
            u64::MAX - SMM_HANDLER_OFFSET + 1,
            0x1000,
            malformed_smbase,
            zero_descriptor_smbase,
            outside_descriptor_smbase,
        ];
        let sm_base_array = sm_bases.as_ptr() as u64;
        let mut services = RecordingSmiPatchServices::new(vec![
            (sm_base_array, size_of_val(&sm_bases) as u64),
            (malformed_smbase + SMM_HANDLER_OFFSET, malformed_entry.len() as u64),
            (zero_descriptor_smbase + SMM_HANDLER_OFFSET, zero_descriptor_entry.len() as u64),
            (outside_descriptor_smbase + SMM_HANDLER_OFFSET, outside_descriptor_entry.len() as u64),
        ]);

        MmSupervisorCore::<TestPlatform, 8>::patch_smi_handler_idt(
            sm_base_array,
            sm_bases.len() as u64,
            malformed_entry.len() as u64,
            &mut services,
        );

        assert!(services.writes.is_empty());
    }

    #[test]
    fn test_init_hob_layouts_match_c_abi() {
        assert_eq!(size_of::<MmCommonRegionHobData>(), 32);
        assert_eq!(size_of::<MmSupvPassDownHobData>(), 64);
        assert_eq!(size_of::<PerCoreMmiEntryStructHdr>(), 22);
        assert_eq!(size_of::<DescriptorTablePointer>(), 10);
    }

    #[test]
    fn test_parse_pass_down_hob() {
        let expected = valid_pass_down_hob();
        let parsed = parse_pass_down_hob(&pass_down_hob_data(&expected)).expect("valid PassDown HOB should parse");

        assert_eq!(parsed.revision, expected.revision);
        assert_eq!(parsed.mm_supervisor_cpl3_stack_base, expected.mm_supervisor_cpl3_stack_base);
        assert_eq!(parsed.sm_base, expected.sm_base);
        assert_eq!(parsed.mm_supv_firmware_policy_buffer, expected.mm_supv_firmware_policy_buffer);
        assert_eq!(parsed.mmi_entrypoint_size, expected.mmi_entrypoint_size);
    }

    #[test]
    fn test_parse_pass_down_hob_rejects_truncated_data() {
        let data = pass_down_hob_data(&valid_pass_down_hob());

        assert_eq!(
            parse_pass_down_hob(&data[..data.len() - 1]).expect_err("truncated PassDown HOB should fail"),
            PolicyInitError::InvalidPolicyData
        );
    }

    #[test]
    fn test_parse_pass_down_hob_rejects_invalid_revision() {
        let mut pass_down = valid_pass_down_hob();
        pass_down.revision += 1;

        assert_eq!(
            parse_pass_down_hob(&pass_down_hob_data(&pass_down))
                .expect_err("invalid PassDown HOB revision should fail"),
            PolicyInitError::InvalidRevision {
                found: pass_down.revision,
                expected: crate::MM_SUPV_PASS_DOWN_HOB_REVISION,
            }
        );
    }

    #[test]
    fn test_parse_pass_down_hob_rejects_invalid_policy_buffer() {
        for (address, size, expected) in [
            (0, 0x1000, PolicyInitError::NullFirmwarePolicyBuffer),
            (0x1000, 0, PolicyInitError::NullFirmwarePolicyBuffer),
            (u64::MAX - 0xFFF, 0x1000, PolicyInitError::InvalidPolicyData),
        ] {
            let mut pass_down = valid_pass_down_hob();
            pass_down.mm_supv_firmware_policy_buffer = address;
            pass_down.mm_supv_firmware_policy_buffer_size = size;

            assert_eq!(
                parse_pass_down_hob(&pass_down_hob_data(&pass_down)).expect_err("invalid policy buffer should fail"),
                expected
            );
        }
    }

    #[test]
    fn test_find_user_module_entry_selects_matching_module() {
        let wrong_allocation = allocation_module(MM_SUPERVISOR_CORE_GUID, MM_SUPERVISOR_USER_GUID, 0x1111);
        let wrong_module =
            allocation_module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_CORE_GUID, 0x2222);
        let matching = allocation_module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_USER_GUID, 0x3333);

        assert_eq!(
            find_user_module_entry([
                Hob::MemoryAllocationModule(&wrong_allocation),
                Hob::MemoryAllocationModule(&wrong_module),
                Hob::MemoryAllocationModule(&matching),
            ]),
            Some(0x3333)
        );
        assert_eq!(
            find_user_module_entry([
                Hob::MemoryAllocationModule(&wrong_allocation),
                Hob::MemoryAllocationModule(&wrong_module),
            ]),
            None
        );
    }

    #[test]
    fn test_find_guid_hob_in_selects_first_matching_hob() {
        let unrelated_data = [0x11];
        let first_data = [0x22, 0x33];
        let second_data = [0x44];
        let unrelated = guid_hob(crate::MM_SUPV_PASS_DOWN_HOB_GUID, unrelated_data.len());
        let first = guid_hob(crate::MM_COMMON_REGION_HOB_GUID, first_data.len());
        let second = guid_hob(crate::MM_COMMON_REGION_HOB_GUID, second_data.len());

        assert_eq!(
            find_guid_hob_in(
                [
                    Hob::GuidHob(&unrelated, &unrelated_data),
                    Hob::GuidHob(&first, &first_data),
                    Hob::GuidHob(&second, &second_data),
                ],
                crate::MM_COMMON_REGION_HOB_GUID,
            ),
            Some(first_data.as_slice())
        );
        assert_eq!(
            find_guid_hob_in([Hob::GuidHob(&unrelated, &unrelated_data)], crate::MM_COMMON_REGION_HOB_GUID),
            None
        );
    }

    #[test]
    fn test_parse_communication_buffer_hobs() {
        let supv = parse_supv_comm_buffer_hob(&supv_comm_buffer_hob_data(0x10_0000, 2, 0x20_0000))
            .expect("valid supervisor communication buffer HOB should parse");
        let user = parse_user_comm_buffer_hob(&user_comm_buffer_hob_data(0x30_0000, 3, 0x40_0000))
            .expect("valid user communication buffer HOB should parse");

        assert_eq!(
            supv,
            ParsedCommBuffer {
                address: 0x10_0000,
                page_count: 2,
                size: 2 * UEFI_PAGE_SIZE as u64,
                status_address: 0x20_0000,
            }
        );
        assert_eq!(
            user,
            ParsedCommBuffer {
                address: 0x30_0000,
                page_count: 3,
                size: 3 * UEFI_PAGE_SIZE as u64,
                status_address: 0x40_0000,
            }
        );
    }

    #[test]
    fn test_parse_communication_buffer_hobs_reject_truncated_data() {
        let supv = supv_comm_buffer_hob_data(0x10_0000, 1, 0x20_0000);
        let user = user_comm_buffer_hob_data(0x30_0000, 1, 0x40_0000);

        assert_eq!(parse_supv_comm_buffer_hob(&supv[..supv.len() - 1]), Err(PolicyInitError::InvalidPolicyData));
        assert_eq!(parse_user_comm_buffer_hob(&user[..user.len() - 1]), Err(PolicyInitError::InvalidPolicyData));
    }

    #[test]
    fn test_parse_communication_buffer_hobs_reject_invalid_ranges() {
        for (address, pages) in [(0x1000, 0), (0x1000, u64::MAX), (u64::MAX - 0xFFF, 1)] {
            assert_eq!(
                parse_supv_comm_buffer_hob(&supv_comm_buffer_hob_data(address, pages, 0x20_0000)),
                Err(PolicyInitError::InvalidCommunicationBufferSize { pages })
            );
            assert_eq!(
                parse_user_comm_buffer_hob(&user_comm_buffer_hob_data(address, pages, 0x20_0000)),
                Err(PolicyInitError::InvalidCommunicationBufferSize { pages })
            );
        }
    }

    #[test]
    fn test_parse_smi_handler_idt_descriptor() {
        let entry = mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, 0x1234_5678_9ABC_DEF0);

        assert_eq!(parse_smi_handler_idt_descriptor(&entry), Ok(0x1234_5678_9ABC_DEF0));
    }

    #[test]
    fn test_parse_smi_handler_idt_descriptor_rejects_malformed_metadata() {
        assert_eq!(parse_smi_handler_idt_descriptor(&[0; 3]), Err(SmiHandlerIdtPatchError::EntryTooSmall));

        let mut oversized_structure = [0_u8; 4];
        oversized_structure.copy_from_slice(&1_u32.to_ne_bytes());
        assert_eq!(
            parse_smi_handler_idt_descriptor(&oversized_structure),
            Err(SmiHandlerIdtPatchError::FixupStructureOutOfBounds)
        );

        let mut short_header = vec![0_u8; 5];
        short_header[1..].copy_from_slice(&1_u32.to_ne_bytes());
        assert_eq!(parse_smi_handler_idt_descriptor(&short_header), Err(SmiHandlerIdtPatchError::FixupHeaderTooSmall));

        let too_few_fixups = mmi_entry(FIXUP64_SMI_HANDLER_IDTR as u8, 0);
        assert_eq!(
            parse_smi_handler_idt_descriptor(&too_few_fixups),
            Err(SmiHandlerIdtPatchError::Fixup64ArrayTooSmall { found: FIXUP64_SMI_HANDLER_IDTR as u8 })
        );

        let mut out_of_bounds_fixup = mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, 0);
        out_of_bounds_fixup[8 + 6] = u8::MAX;
        assert_eq!(
            parse_smi_handler_idt_descriptor(&out_of_bounds_fixup),
            Err(SmiHandlerIdtPatchError::Fixup64EntryOutOfBounds)
        );
    }

    #[test]
    fn test_read_idtr_is_zeroed_in_unit_tests() {
        let idtr = read_idtr();
        let base = idtr.base;
        let limit = idtr.limit;

        assert_eq!(base, 0);
        assert_eq!(limit, 0);
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
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let mut data = [0_u8; 16 + PROCESSOR_INFO_ENTRY_SIZE];

        assert_eq!(supervisor.parse_mp_information_hob(&data[..15]), Err(PolicyInitError::InvalidPolicyData));

        data[..8].copy_from_slice(&2_u64.to_le_bytes());
        assert_eq!(supervisor.parse_mp_information_hob(&data), Err(PolicyInitError::InvalidPolicyData));
    }

    #[test]
    fn test_parse_mp_information_hob_rejects_invalid_cpu_count() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let mut data = [0_u8; 16];

        for cpu_count in [0_u64, 5] {
            data[..8].copy_from_slice(&cpu_count.to_le_bytes());
            assert_eq!(
                supervisor.parse_mp_information_hob(&data),
                Err(PolicyInitError::InvalidCpuCount { found: cpu_count, maximum: 4 })
            );
        }
    }

    fn mseg_smram_hob_data(
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

    #[test]
    fn test_parse_mseg_smram_hob_returns_cpu_start() {
        let data = mseg_smram_hob_data(0x0080_0000, 0x0040_0000, 0x0002_0000);

        assert_eq!(parse_mseg_smram_hob(&data), Some(0x0040_0000));
    }

    #[test]
    fn test_parse_mseg_smram_hob_rejects_truncated_descriptor() {
        let data = mseg_smram_hob_data(0x0040_0000, 0x0040_0000, 0x0002_0000);

        assert_eq!(parse_mseg_smram_hob(&data[..data.len() - 1]), None);
    }

    #[test]
    fn test_parse_mseg_smram_hob_rejects_empty_region() {
        let data = mseg_smram_hob_data(0x0040_0000, 0x0040_0000, 0);

        assert_eq!(parse_mseg_smram_hob(&data), None);
    }

    #[test]
    fn test_parse_mseg_smram_hob_rejects_invalid_base() {
        for invalid_base in [0x0040_0001, 0x1_0000_0000] {
            let data = mseg_smram_hob_data(invalid_base, invalid_base, 0x0002_0000);

            assert_eq!(parse_mseg_smram_hob(&data), None);
        }
    }
}
