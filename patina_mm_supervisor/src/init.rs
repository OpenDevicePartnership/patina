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

use core::ffi::c_void;

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
    /// Invalid PassDown HOB revision.
    InvalidRevision {
        /// The revision value found in the PassDown HOB.
        found: u32,
        /// The revision value the supervisor expected.
        expected: u32,
    },
    /// Firmware policy buffer is null or empty.
    NullFirmwarePolicyBuffer,
    /// Invalid policy data.
    InvalidPolicyData,
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

/// MM Supervisor PassDown HOB Data Structure
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
#[derive(Debug, Clone, Copy)]
struct PerCoreMmiEntryStructHdr {
    /// Header version (4 for version 4).
    header_version: u32,
    /// Offset from header start to FixUpStruct array.
    fixup_struct_offset: u8,
    /// Number of FixUpStruct array entries.
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
                in(reg) &mut descriptor as *mut DescriptorTablePointer,
                options(nostack, preserves_flags)
            );
        }
        descriptor
    };

    rt_descriptor
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
            panic!("Failed to initialize Interrupt Manager: {:?}", err);
        });

        // SAFETY: `hob_list` is provided by the MM IPL and is guaranteed to be a
        // valid HOB list (the caller asserts it is non-null before dispatching).
        let (scanned_regions, region_count) = unsafe { self.init_page_allocators(hob_list) };

        // Validate the critical incoming HOBs against the untrusted producer's
        // data before any of their contents are consumed below.
        let scanned_regions = scanned_regions.get(..region_count).unwrap_or(&scanned_regions);
        // SAFETY: `hob_list` is a valid HOB list per the caller's guarantee,
        // and the page allocator was just initialized so the MMRAM containment
        // checks are meaningful.
        if let Err(e) = unsafe { hob_validation::validate_incoming_hobs_pre_paging_init(hob_list, scanned_regions) } {
            panic!("Incoming HOB validation failed: {}", e);
        }

        self.init_page_table();

        // Validate the incoming HOBs that require an active page table, now
        // that it is available (the remaining checks that only need the page
        // allocator ran above).
        // SAFETY: `hob_list` is a valid HOB list per the caller's guarantee,
        // and the page table was just initialized so per-page attribute queries
        // are meaningful.
        if let Err(e) = unsafe { hob_validation::validate_incoming_hobs_post_paging_init(hob_list) } {
            panic!("Post-paging HOB validation failed: {}", e);
        }

        // SAFETY: `hob_list` is provided by the MM IPL and is guaranteed to be a
        // valid HOB list (the caller asserts it is non-null before dispatching).
        unsafe {
            self.discover_and_store_user_entry(hob_list);
            self.init_policy_and_validate(hob_list);
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
    unsafe fn init_page_allocators(&self, hob_list: *const c_void) -> ([SmramRegion; MAX_TEMP_REGIONS], usize) {
        // Initialize the page allocator from the HOB list. This finds all SMRAM
        // regions and sets up memory tracking.
        // SAFETY: `hob_list` is a valid HOB list per this function's contract.
        let (smram_regions, region_count) =
            match unsafe { security_state().page_allocator().init_from_hob_list(hob_list) } {
                Ok(scanned_regions) => scanned_regions,
                Err(e) => panic!("Failed to initialize page allocator: {:?}", e),
            };

        // Derive the SMRR range from the scanned SMRAM regions, coalescing
        // physically adjacent regions, and store it for later SMRR programming.
        match coalesced_smrr_range(smram_regions.get(..region_count).unwrap_or(&smram_regions)) {
            Some(range) => {
                log::info!("Discovered SMRR range: base=0x{:08x}, size=0x{:08x}", range.base, range.size);
                init_state().set_smrr_range(range);
            }
            None => panic!("Failed to determine SMRR range from scanned SMRAM regions"),
        }

        // Reserve pages from the page allocator for paging structures. This is
        // done before paging is initialized to avoid a circular dependency.
        let paging_pool_base = match security_state().page_allocator().allocate_pages(mem::DEFAULT_PAGING_POOL_PAGES) {
            Ok(base) => base,
            Err(e) => {
                panic!("Failed to reserve pages for paging structures: {:?}", e);
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
        let init_result =
            unsafe { security_state().paging_allocator().init(paging_pool_base, mem::DEFAULT_PAGING_POOL_PAGES) };
        if let Err(e) = init_result {
            panic!("Failed to initialize paging allocator: {:?}", e);
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
        log::info!("Page table initialized from CR3=0x{:016x}", cr3);
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
    unsafe fn discover_and_store_user_entry(&self, hob_list: *const c_void) {
        // SAFETY: `hob_list` is a valid HOB list per this function's contract.
        match unsafe { self.discover_user_module_entry(hob_list) } {
            Some(entry) => {
                log::info!("Discovered MM User module entry point: 0x{:016x}", entry);
                init_state().set_user_entry_point(entry);
            }
            None => log::warn!("MM User module entry point not found in HOB list"),
        }
    }

    /// Initializes the policy gate from the PassDown HOB and runs an initial
    /// security validation.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `hob_list` points to a valid HOB list.
    unsafe fn init_policy_and_validate(&self, hob_list: *const c_void) {
        // SAFETY: `hob_list` is a valid HOB list per this function's contract.
        if let Err(e) = unsafe { self.init_policy_from_hob_list(hob_list) } {
            log::error!("Failed to initialize policy gate: {:?}", e);
        }

        // Now that we have the policy, run an initial security validation.
        if let Some(gate) = security_state().policy_gate() {
            // SAFETY: `gate.as_ptr()` returns the firmware policy buffer pointer
            // validated while constructing the policy gate above.
            if let Err(e) = unsafe { mm_policy::helpers::security_policy_check(gate.as_ptr()) } {
                panic!("Security policy check failed during init: {:?}", e);
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
            .unwrap_or_else(|e| panic!("Failed to page-align HOB list region: {:?}", e));
        let aligned_end = aligned_base + hob_region_size;
        log::info!(
            "HOB list at 0x{:016x} size 0x{:x}, aligned region 0x{:016x}-0x{:016x} (0x{:x} bytes)",
            hob_base,
            hob_list_size,
            aligned_base,
            aligned_end,
            hob_region_size
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
                "Failed to remap HOB list to user level at 0x{:016x} (0x{:x} bytes): {:?}",
                aligned_base, hob_region_size, e
            );
        }
        log::info!("Remapped HOB list 0x{:016x}-0x{:016x} as user read-only", aligned_base, aligned_end);
    }

    /// Patches every core's SMI-handler IDT descriptor to point to the Rust IDT.
    ///
    /// Each per-core MMI entry (copied to `sm_base[i] + 0x8000` during C relocation)
    /// carries a `Fixup64[FIXUP64_SMI_HANDLER_IDTR]` slot holding the address of the
    /// `IA32_DESCRIPTOR` that core's SMI entry `lidt`s.
    ///
    /// `sm_base_array` is the per-CPU SMBASE array (`u64[number_of_cpus]`) from the PassDown
    /// HOB; `number_of_cpus` is its length.
    fn patch_smi_handler_idt(sm_base_array: u64, number_of_cpus: u64, mmi_entry_size: u64) {
        if mmi_entry_size == 0 {
            log::warn!("MMI entry size is 0 in PassDown HOB, cannot navigate fixup structure");
            return;
        }
        if sm_base_array == 0 || number_of_cpus == 0 {
            log::warn!("SMBASE array is null or CPU count is 0, cannot patch SMI handler IDT");
            return;
        }

        let idtr = read_idtr();
        // Copy packed fields into aligned locals before formatting; taking a reference to a
        // field of a `packed(2)` struct (as `log::info!` would) is undefined behavior.
        let idtr_base = idtr.base;
        let idtr_limit = idtr.limit;

        // Read the whole SMBASE array once as a slice (same pattern as the save-state read path).
        // SAFETY: `sm_base_array` references a valid `u64[number_of_cpus]` array in SMRAM from the
        // PassDown HOB (both operands validated non-zero above), so the slice spans only valid,
        // initialized memory. We cannot do too much other null check, from above.
        let sm_bases = unsafe { core::slice::from_raw_parts(sm_base_array as *const u64, number_of_cpus as usize) };

        for (cpu, &smbase) in sm_bases.iter().enumerate() {
            if smbase == 0 {
                log::warn!("CPU {}: SMBASE is 0, skipping SMI handler IDT patch", cpu);
                continue;
            }

            let mmi_entry_base = smbase + SMM_HANDLER_OFFSET;

            // The last u32 in the MMI entry binary is the total fixup structure size.
            let whole_struct_size_addr = mmi_entry_base + mmi_entry_size - 4;
            // SAFETY: points into this core's SMI handler template in SMRAM.
            let whole_struct_size = unsafe { core::ptr::read_unaligned(whole_struct_size_addr as *const u32) };

            // The structure header starts before the trailing size field.
            let hdr_addr =
                (mmi_entry_base + mmi_entry_size - 4 - whole_struct_size as u64) as *const PerCoreMmiEntryStructHdr;
            // SAFETY: hdr_addr points to the packed fixup header within this core's SMI handler binary.
            let hdr = unsafe { core::ptr::read_unaligned(hdr_addr) };
            let f64_offset = hdr.fixup64_offset;
            let f64_num = hdr.fixup64_num;

            // Validate the Fixup64 array has the IDTR entry.
            if (FIXUP64_SMI_HANDLER_IDTR as u8) >= f64_num {
                log::error!(
                    "CPU {}: Fixup64 array too small: need index {} but only {} entries",
                    cpu,
                    FIXUP64_SMI_HANDLER_IDTR,
                    f64_num
                );
                continue;
            }

            // Navigate to this core's Fixup64 array and read the descriptor pointer it holds.
            let fixup64_base = (hdr_addr as u64 + f64_offset as u64) as *const u64;
            // SAFETY: fixup64_base + index is within this core's fixup array.
            let idt_desc_addr = unsafe { core::ptr::read_unaligned(fixup64_base.add(FIXUP64_SMI_HANDLER_IDTR)) };

            if idt_desc_addr == 0 {
                log::warn!("CPU {}: Fixup64[{}] (SMI_HANDLER_IDTR) is null", cpu, FIXUP64_SMI_HANDLER_IDTR);
                continue;
            }

            // Overwrite the IA32_DESCRIPTOR this core references with the Rust IDT base/limit.
            let idt_desc_ptr = idt_desc_addr as *mut DescriptorTablePointer;
            // SAFETY: idt_desc_ptr references an IA32_DESCRIPTOR in SMRAM (allocated by the C
            // relocation via AllocateCodePages). DescriptorTablePointer (packed(2)) and
            // IA32_DESCRIPTOR (packed(1)) share the same 10-byte {u16, u64} layout.
            unsafe { core::ptr::write_unaligned(idt_desc_ptr, idtr) };

            log::info!(
                "CPU {}: patched SMI handler IDT descriptor at 0x{:016x}: base=0x{:016x}, limit=0x{:04x}",
                cpu,
                idt_desc_addr,
                idtr_base,
                idtr_limit
            );
        }
    }

    /// Per-core initialization.
    ///
    /// This is called on every core (BSP and APs) during the first entry.
    /// Use this for setting up per-CPU state like syscall MSRs, GS base, etc.
    pub(crate) fn per_core_init(&'static self, cpu_id: u32, is_bsp: bool) {
        let core_type = if is_bsp { "BSP" } else { "AP" };
        log::trace!("{} (CPU {}) performing per-core initialization...", core_type, cpu_id);

        // IA32_SMM_MONITOR_CTL is per-logical-processor, so every core programs it.
        Self::program_mseg_base(cpu_id);

        let range =
            init_state().smrr_range().expect("SMRR range must be determined during BSP init before per-core init");
        smrr_initialize(range);
        configure_smm_code_access();

        log::trace!("{} (CPU {}) per-core initialization complete.", core_type, cpu_id);
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
            log::warn!(
                "CPU {} does not support VMX (CPUID.01H:ECX.VMX=0), cannot program IA32_SMM_MONITOR_CTL",
                cpu_id
            );
            return;
        }

        // SAFETY: IA32_SMM_MONITOR_CTL is an architectural MSR available whenever VMX is
        // reported by CPUID.01H:ECX.VMX. After the check above, only the architecturally
        // defined Valid and MsegBase fields are set; reserved bits are masked off above,
        // so the write cannot #GP on a reserved-bit violation. The write affects only this
        // logical processor's MSR.
        unsafe { write_msr(IA32_SMM_MONITOR_CTL_MSR, value) };
        log::debug!("CPU {} programmed IA32_SMM_MONITOR_CTL = 0x{:x}", cpu_id, value);
    }

    /// Discovers the MM Supervisor User module entry point from the HOB list.
    ///
    /// This function iterates through the HOB list looking for `MemoryAllocationModule` HOBs
    /// that match the MM Supervisor memory allocation module GUID and have the MM Supervisor
    /// User GUID as their module name.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `hob_list` points to a valid HOB list.
    unsafe fn discover_user_module_entry(&self, hob_list: *const c_void) -> Option<u64> {
        if hob_list.is_null() {
            return None;
        }

        // Get the HOB list header
        // SAFETY: `hob_list` was checked non-null above and, per this function's contract, points
        // to a valid HOB list, so reinterpreting it as the handoff table header and taking a
        // shared reference is sound.
        let hob_list_info = unsafe { (hob_list as *const PhaseHandoffInformationTable).as_ref()? };

        let hob = Hob::Handoff(hob_list_info);

        // Iterate through the HOB list looking for MemoryAllocationModule HOBs
        for current_hob in &hob {
            if let Hob::MemoryAllocationModule(mem_alloc_mod) = current_hob {
                // Check if this is an MM Supervisor module allocation
                // (MemoryAllocationHeader.Name == gMmSupervisorHobMemoryAllocModuleGuid)
                if mem_alloc_mod.alloc_descriptor.name == MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID {
                    log::debug!(
                        "Found MM Supervisor module HOB: module_name={:?}, entry_point=0x{:016x}",
                        mem_alloc_mod.module_name,
                        mem_alloc_mod.entry_point
                    );

                    // Check if this is the User module (ModuleName == gMmSupervisorUserGuid)
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
        }

        None
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
    unsafe fn init_policy_from_hob_list(&self, hob_list: *const c_void) -> Result<(), PolicyInitError> {
        if hob_list.is_null() {
            return Err(PolicyInitError::NullHobList);
        }

        // SAFETY: `hob_list` was checked non-null above and, per this function's contract, points
        // to a valid HOB list, so taking a shared reference to the handoff table header is sound.
        let hob_list_info =
            unsafe { (hob_list as *const PhaseHandoffInformationTable).as_ref().ok_or(PolicyInitError::NullHobList)? };

        // 1. Process the PassDown HOB (policy, syscall, memory policy)
        let pass_down_data =
            find_guid_hob(hob_list_info, crate::MM_SUPV_PASS_DOWN_HOB_GUID).ok_or(PolicyInitError::HobNotFound)?;
        // SAFETY: `pass_down_data` is a slice into the validated HOB list, so the buffer pointers
        // it carries reference live memory as `init_from_pass_down_hob` requires.
        let (sm_base, mmi_entry_size) = unsafe { self.init_from_pass_down_hob(pass_down_data)? };

        // 1b. Process the MP Information HOB (`gMpInformationHobGuid`) for the CPU count.
        let mp_information =
            find_guid_hob(hob_list_info, crate::MP_INFORMATION_HOB_GUID).ok_or(PolicyInitError::HobNotFound)?;
        let number_of_cpus = self.parse_mp_information_hob(mp_information).ok_or(PolicyInitError::InvalidPolicyData)?;
        security_state().set_save_state_info(SaveStateInfo { number_of_cpus, sm_base });
        log::info!("Save-state metadata initialized for {} CPU(s)", number_of_cpus);

        // 1b-ii. Process the MSEG SMRAM HOB (`gMsegSmramGuid`), if published. It carries the
        //        MSEG region reserved for an STM. Each core programs the base into
        //        IA32_SMM_MONITOR_CTL during per-core init. Platforms without STM/SEA
        //        integration do not publish this HOB, so its absence is not an error.
        match find_guid_hob(hob_list_info, crate::MSEG_SMRAM_HOB_GUID).and_then(parse_mseg_smram_hob) {
            Some(mseg_base) => {
                init_state().set_mseg_base(mseg_base);
                log::info!("MSEG base 0x{:x} discovered from MSEG SMRAM HOB", mseg_base);
            }
            _ => log::warn!("No usable MSEG SMRAM HOB; IA32_SMM_MONITOR_CTL will not be programmed"),
        }

        // 1c. Patch every core's SMI-handler IDT descriptor to the Rust IDT now that the
        //     CPU count is known (the SMI entry blocks were already copied per SMBASE, so
        //     each core must be patched, not just the BSP).
        Self::patch_smi_handler_idt(sm_base, number_of_cpus, mmi_entry_size);

        // 2. Process the supervisor communication buffer HOB. Only one
        //    MM_COMM_REGION_HOB is published (the supervisor one); the user
        //    channel flows through MM_COMM_BUFFER_HOB_GUID below.
        let supv_region_data =
            find_guid_hob(hob_list_info, crate::MM_COMMON_REGION_HOB_GUID).ok_or(PolicyInitError::HobNotFound)?;
        let (supv_comm_buffer, supv_comm_buffer_size, supv_comm_buffer_internal, supv_status_buffer) =
            init_supv_comm_buffer(supv_region_data)?;

        // 3. Process the user communication buffer HOB. This still uses the
        //    legacy `MM_COMM_BUFFER_HOB_GUID` so the user core's own HOB walk
        //    keeps working (see the HACKHACK at the tail of
        //    init_user_comm_buffer).
        let user_buffer_data =
            find_guid_hob(hob_list_info, MM_COMM_BUFFER_HOB_GUID).ok_or(PolicyInitError::HobNotFound)?;
        // SAFETY: `user_buffer_data` aliases the original, writable HOB buffer in the live HOB
        // list, satisfying `init_user_comm_buffer`'s contract that it may rewrite the HOB's
        // `physical_start` field in place.
        let (user_comm_buffer, user_comm_buffer_size, user_comm_buffer_internal, user_status_buffer) =
            unsafe { init_user_comm_buffer(user_buffer_data)? };

        // 4. Allocate the supervisor-to-user data buffer
        let supv_to_user_buffer =
            security_state().page_allocator().allocate_pages_with_type(1, AllocationType::User).map_err(|e| {
                log::error!("Failed to allocate page for supervisor-to-user buffer: {:?}", e);
                PolicyInitError::MemoryAllocationFailed
            })?;

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
        security_state().set_comm_buffer_config(CommBufferConfig {
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
            "Comm buffers: supv=0x{:x}/0x{:x} size=0x{:x} status=0x{:x}, user=0x{:x}/0x{:x} size=0x{:x} status=0x{:x}",
            supv_comm_buffer,
            supv_comm_buffer_internal,
            supv_comm_buffer_size,
            supv_status_buffer,
            user_comm_buffer,
            user_comm_buffer_internal,
            user_comm_buffer_size,
            user_status_buffer
        );

        Ok(())
    }

    /// Returns the CPU count from the MP Information HOB.
    fn parse_mp_information_hob(&self, data: &[u8]) -> Option<u64> {
        /// Offset of `ProcessorInfoBuffer[]` within `MP_INFORMATION_HOB_DATA`.
        const PROCESSOR_INFO_BUFFER_OFFSET: usize = 16;

        if data.len() < PROCESSOR_INFO_BUFFER_OFFSET {
            log::error!("MP Information HOB too small: {} < {}", data.len(), PROCESSOR_INFO_BUFFER_OFFSET);
            return None;
        }

        let number_of_cpus = u64::from_le_bytes(data.get(0..8)?.try_into().ok()?);
        let cpu_count: usize = number_of_cpus.try_into().ok()?;
        if cpu_count > MAX_CPUS {
            panic!("MP Information HOB CPU count {} exceeds supervisor maximum {}", cpu_count, MAX_CPUS);
        }

        let processor_info_size = cpu_count.checked_mul(PROCESSOR_INFO_ENTRY_SIZE)?;
        data.get(PROCESSOR_INFO_BUFFER_OFFSET..PROCESSOR_INFO_BUFFER_OFFSET.checked_add(processor_info_size)?)?;

        Some(number_of_cpus)
    }

    /// Processes the MM Supervisor PassDown HOB.
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
    /// must reference valid memory, as they are dereferenced during setup.
    unsafe fn init_from_pass_down_hob(&self, data: &[u8]) -> Result<(u64, u64), PolicyInitError> {
        // Copy the HOB bytes once into an owned, naturally-aligned struct so the rest of
        // the function uses ordinary, safe field access. `read_from_prefix` validates the
        // length and copies the bytes, imposing no alignment or validity precondition on
        // the caller's buffer.
        let (pass_down, _) = MmSupvPassDownHobData::read_from_prefix(data).map_err(|_| {
            log::error!(
                "PassDown HOB data too small: {} < {}",
                data.len(),
                core::mem::size_of::<MmSupvPassDownHobData>()
            );
            PolicyInitError::InvalidPolicyData
        })?;

        let revision = pass_down.revision;
        let mm_initialized_buffer = pass_down.mm_initialized_buffer;
        let firmware_policy_buffer = pass_down.mm_supv_firmware_policy_buffer;
        let firmware_policy_buffer_size = pass_down.mm_supv_firmware_policy_buffer_size;
        let cpl3_stack_buffer = pass_down.mm_supervisor_cpl3_stack_base;
        let cpl3_stack_buffer_size = pass_down.mm_supervisor_cpl3_per_core_stack_size;
        let mmi_entry_size = pass_down.mmi_entrypoint_size;
        let sm_base = pass_down.sm_base;

        // Validate revision
        if revision != crate::MM_SUPV_PASS_DOWN_HOB_REVISION {
            log::error!(
                "Invalid PassDown HOB revision: {} (expected {})",
                revision,
                crate::MM_SUPV_PASS_DOWN_HOB_REVISION
            );
            return Err(PolicyInitError::InvalidRevision {
                found: revision,
                expected: crate::MM_SUPV_PASS_DOWN_HOB_REVISION,
            });
        }

        // Store per-core initialized buffer address
        if mm_initialized_buffer != 0 {
            init_state().set_mm_initialized_buffer(mm_initialized_buffer);
            log::info!("MM Initialized buffer set to 0x{:016x}", mm_initialized_buffer);
        } else {
            log::warn!("MM Initialized buffer is null in PassDown HOB");
        }

        // Log the per-CPU SMBASE array passed down for the save-state read syscall.
        if sm_base != 0 {
            log::info!("CPU SMBASE array at 0x{:016x}", sm_base);
        } else {
            log::warn!("CPU SMBASE array pointer is null in PassDown HOB");
        }

        // Initialize the policy gate
        if firmware_policy_buffer == 0 || firmware_policy_buffer_size == 0 {
            log::error!("Firmware policy buffer is null or empty");
            return Err(PolicyInitError::NullFirmwarePolicyBuffer);
        }

        let policy_ptr = firmware_policy_buffer as *const u8;
        let memory_policy_buffer = security_state().page_allocator().allocate_pages(1).map_err(|e| {
            log::error!("Failed to allocate page for memory policy buffer: {:?}", e);
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
                log::error!("Failed to create policy gate: {:?}", e);
                return Err(PolicyInitError::InvalidPolicyData);
            }
        }

        // Initialize syscall interface
        self.syscall_interface
            .init(
                self.cpu_manager.max_cpus(),
                cpl3_stack_buffer,
                cpl3_stack_buffer_size
                    .try_into()
                    .unwrap_or_else(|err| panic!("Invalid CPL3 stack buffer size: {:?}", err)),
            )
            .unwrap_or_else(|err| panic!("Failed to initialize syscall interface: {:?}", err));

        // Walk page table and generate memory policy
        let cr3 = read_cr3();
        // SAFETY: `cr3` is read from the active control register, so it points to the live PML4
        // table, and `memory_policy_buffer` is the page just allocated above with room for
        // `UEFI_PAGE_SIZE` bytes of descriptors.
        let count = unsafe {
            walk_page_table(cr3, memory_policy_buffer as *mut MemDescriptorV1_0, UEFI_PAGE_SIZE, is_buffer_inside_mmram)
        };

        if let Ok(descriptor_count) = count {
            log::info!("Successfully generated {} memory policy descriptors", descriptor_count);
            // SAFETY: `walk_page_table` succeeded, so `memory_policy_buffer` holds `descriptor_count`
            // valid `MemDescriptorV1_0` entries.
            if let Err(e) = unsafe {
                security_state()
                    .unblocked_tracker()
                    .init_from_buffer(memory_policy_buffer as *const MemDescriptorV1_0, descriptor_count)
            } {
                log::error!("Failed to initialize unblocked memory tracker: {:?}", e);
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

/// Finds the first GUID HOB matching `target_guid` and returns its data slice.
///
/// Returns `None` if no matching HOB is found.
pub(crate) fn find_guid_hob(
    hob_list_info: &PhaseHandoffInformationTable,
    target_guid: patina::BinaryGuid,
) -> Option<&[u8]> {
    let hob = Hob::Handoff(hob_list_info);
    for current_hob in &hob {
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
            log::error!("MSEG SMRAM HOB too small: {} < {}", data.len(), core::mem::size_of::<SmramDescriptor>())
        })
        .ok()?;

    if descriptor.physical_size == 0 {
        log::warn!("MSEG SMRAM HOB describes an empty region");
        return None;
    }

    let base = descriptor.cpu_start;
    if base & !SMM_MONITOR_CTL_MSEG_BASE_MASK != 0 {
        log::error!("MSEG base 0x{:x} is not 4 KiB aligned or lies above 4 GiB", base);
        return None;
    }

    Some(base)
}

/// Processes the supervisor communication buffer HOB (`MM_COMMON_REGION_HOB_GUID`).
///
/// Returns `(buffer_addr, buffer_size, internal_copy_addr, status_buffer_addr)`.
fn init_supv_comm_buffer(data: &[u8]) -> Result<(u64, u64, u64, u64), PolicyInitError> {
    log::info!("Found MM Common Region HOB (supervisor)");

    // Copy the HOB bytes once into an owned struct for safe field access.
    let (supv_buffer_hob, _) = MmCommonRegionHobData::read_from_prefix(data).map_err(|_| {
        log::error!(
            "MM Common Region HOB data too small: {} < {}",
            data.len(),
            core::mem::size_of::<MmCommonRegionHobData>()
        );
        PolicyInitError::InvalidPolicyData
    })?;
    let supv_comm_buffer = supv_buffer_hob.addr;
    let supv_comm_buffer_pages = supv_buffer_hob.number_of_pages;
    let supv_status_buffer = supv_buffer_hob.status_addr;

    let supv_comm_buffer_size = supv_comm_buffer_pages.checked_mul(UEFI_PAGE_SIZE as u64).unwrap_or_else(|| {
        panic!(
            "Invalid supervisor common buffer size: {} pages * {} page size overflows",
            supv_comm_buffer_pages, UEFI_PAGE_SIZE
        );
    });

    // Validate ownership if outside MMRAM
    if !is_buffer_inside_mmram(supv_comm_buffer, supv_comm_buffer_size) {
        match query_address_ownership(supv_comm_buffer, supv_comm_buffer_size) {
            Some(PageOwnership::Supervisor) => { /* expected */ }
            Some(PageOwnership::User) => {
                panic!(
                    "Supervisor common buffer at 0x{:016x}-0x{:016x} is not marked as supervisor-owned",
                    supv_comm_buffer,
                    supv_comm_buffer + supv_comm_buffer_size
                );
            }
            None => {
                panic!("Failed to query page ownership for supervisor common buffer at 0x{:016x}", supv_comm_buffer);
            }
        };
    }

    // Allocate internal copy
    let supv_comm_buffer_internal = security_state()
        .page_allocator()
        .allocate_pages_with_type(supv_comm_buffer_pages as usize, AllocationType::Supervisor)
        .map_err(|e| {
            log::error!("Failed to allocate internal supervisor common buffer: {:?}", e);
            PolicyInitError::MemoryAllocationFailed
        })?;

    Ok((supv_comm_buffer, supv_comm_buffer_size, supv_comm_buffer_internal, supv_status_buffer))
}

/// Processes the user communication buffer HOB (`MM_COMM_BUFFER_HOB_GUID`).
///
/// Returns `(buffer_addr, buffer_size, internal_copy_addr, status_buffer_addr)`.
///
/// ## Safety
///
/// `data` must reference the original, writable HOB buffer: its `physical_start`
/// field is overwritten in place with the internal copy address so the demoted
/// user module observes it after demotion.
unsafe fn init_user_comm_buffer(data: &[u8]) -> Result<(u64, u64, u64, u64), PolicyInitError> {
    log::info!("Found MM Communication Buffer HOB");

    // Copy the HOB fields once into an owned struct for safe access.
    let (comm_buffer_hob, _) = MmCommonBufferHobData::read_from_prefix(data).map_err(|_| {
        log::error!(
            "MM Communication Buffer HOB data too small: {} < {}",
            data.len(),
            core::mem::size_of::<MmCommonBufferHobData>()
        );
        PolicyInitError::InvalidPolicyData
    })?;
    let user_comm_buffer = comm_buffer_hob.physical_start;
    let user_comm_buffer_pages = comm_buffer_hob.number_of_pages;
    let status_buffer = comm_buffer_hob.status_buffer;

    let user_comm_buffer_size = user_comm_buffer_pages.checked_mul(UEFI_PAGE_SIZE as u64).unwrap_or_else(|| {
        panic!(
            "Invalid user common buffer size: {} pages * {} page size overflows",
            user_comm_buffer_pages, UEFI_PAGE_SIZE
        );
    });

    // Validate ownership if outside MMRAM
    if !is_buffer_inside_mmram(user_comm_buffer, user_comm_buffer_size) {
        match query_address_ownership(user_comm_buffer, user_comm_buffer_size) {
            Some(PageOwnership::Supervisor) => { /* expected */ }
            Some(PageOwnership::User) => {
                panic!(
                    "User common buffer at 0x{:016x}-0x{:016x} is not marked as user-owned",
                    user_comm_buffer,
                    user_comm_buffer + user_comm_buffer_size
                );
            }
            None => {
                panic!("Failed to query page ownership for user common buffer at 0x{:016x}", user_comm_buffer);
            }
        };
    }

    // Validate status buffer
    if !is_buffer_inside_mmram(status_buffer, core::mem::size_of::<MmCommBufferStatus>() as u64) {
        match query_address_ownership(status_buffer, core::mem::size_of::<MmCommBufferStatus>() as u64) {
            Some(PageOwnership::Supervisor) => { /* expected */ }
            Some(PageOwnership::User) => {
                panic!("Status buffer at 0x{:016x} is not marked as supervisor-exposed", status_buffer);
            }
            None => {
                panic!("Failed to query page ownership for status buffer at 0x{:016x}", status_buffer);
            }
        };
    }

    // Allocate internal copy
    let user_comm_buffer_internal = security_state()
        .page_allocator()
        .allocate_pages_with_type(user_comm_buffer_pages as usize, AllocationType::User)
        .map_err(|e| {
            log::error!("Failed to allocate internal user common buffer: {:?}", e);
            PolicyInitError::MemoryAllocationFailed
        })?;

    // TODO: Remove the logic that overwrites the HOB's physical_start with the internal buffer address
    // so the user module sees it after demotion.
    // SAFETY:
    // - `data` aliases the original, writable HOB buffer (per this function's `## Safety` contract),
    //   and the `read_from_prefix` above proved it is at least `size_of::<MmCommonBufferHobData>()`
    //   bytes, so `physical_start` lies within the allocation. `addr_of_mut!` avoids forming a
    //   reference to the (potentially unaligned) field, and `write_volatile` keeps the store from
    //   being elided.
    // - The HOB pages may be mapped read-only, so `disable_write_protection` clears `CR0.WP` to
    //   permit the supervisor store. This runs in Ring 0 during single-threaded BSP init in the MM
    //   (SMM) environment, where interrupts are masked, satisfying the privilege/atomicity
    //   requirements of `disable_write_protection`. `enable_write_protection` is handed exactly the
    //   value returned by `disable_write_protection`, restoring `CR0.WP` before this block returns
    //   and bounding the unprotected window to the single field write.
    unsafe {
        let hob_ptr = data.as_ptr() as *mut MmCommonBufferHobData;
        let original_cr0 = disable_write_protection();
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*hob_ptr).physical_start), user_comm_buffer_internal);
        enable_write_protection(original_cr0);
    }

    Ok((user_comm_buffer, user_comm_buffer_size, user_comm_buffer_internal, status_buffer))
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    struct TestPlatform;

    impl PlatformInfo for TestPlatform {}

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

        assert_eq!(supervisor.parse_mp_information_hob(&data), Some(3));
    }

    #[test]
    fn test_parse_mp_information_hob_rejects_invalid_size() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let mut data = [0_u8; 16 + PROCESSOR_INFO_ENTRY_SIZE];

        assert_eq!(supervisor.parse_mp_information_hob(&data[..15]), None);

        data[..8].copy_from_slice(&2_u64.to_le_bytes());
        assert_eq!(supervisor.parse_mp_information_hob(&data), None);
    }

    #[test]
    #[should_panic(expected = "MP Information HOB CPU count 5 exceeds supervisor maximum 4")]
    fn test_parse_mp_information_hob_panics_when_cpu_count_exceeds_maximum() {
        let supervisor = MmSupervisorCore::<TestPlatform, 4>::new();
        let mut data = [0_u8; 16];
        data[..8].copy_from_slice(&5_u64.to_le_bytes());

        let _ = supervisor.parse_mp_information_hob(&data);
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
