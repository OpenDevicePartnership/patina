//! Per-Core MMI Entry Validation
//!
//! On its first entry each core checks the MMI entry it arrived through, at
//! `SMBASE + SMM_HANDLER_OFFSET`, and the state that entry left it in: the fixup slots, the
//! loaded registers, the GDT and TSS, the PSD, the Ring 0 stack and the page protections of each
//! structure. The BSP also checks every core's entry before any Ring 3 code runs, since the entry
//! code uses some of these structures before paging is on.
//!
//! Everything comes from the published [`SaveStateInfo`] and the MMI entries, because the HOB
//! list has been reclaimed by the time this runs.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::arch::x86_64::__cpuid;
use core::fmt;
use core::mem::size_of;

use patina::{MemoryRange, UEFI_PAGE_SIZE};
use patina_paging::{MemoryAttributes, PageTable, PtError};
use zerocopy::FromBytes;

use super::CoreInitError;
use super::smi_idt_patch::{
    FIXUP8_5_LEVEL_PAGING, FIXUP8_CET_SUPPORTED, FIXUP8_FIRST_RESERVED, FIXUP8_MISC_ENABLE_SUPPORTED,
    FIXUP8_XD_SUPPORTED, FIXUP32_CET_INTERRUPT_SSP, FIXUP32_CET_INTERRUPT_SSP_TABLE, FIXUP32_CET_PL0_SSP, FIXUP32_CR3,
    FIXUP32_GDTR, FIXUP32_MSR_SMM_BASE, FIXUP32_STACK_CPL0, FIXUP64_CET_SUPPORTED, FIXUP64_HOB_START,
    FIXUP64_SMI_HANDLER_IDTR, FIXUP64_SMI_RDZ_ENTRY, FIXUP64_SMM_DBG_ENTRY, FIXUP64_SMM_DBG_EXIT, FIXUP64_XD_SUPPORTED,
    MMI_ENTRY_STRUCT_VERSION, MmiEntryFixups, SMM_HANDLER_OFFSET, SmiHandlerIdtPatchError, locate_mmi_entry,
};
use crate::error::MmSupervisorResult;
use crate::hob::validation::{SMM_PSD_OFFSET, SMRAM_WINDOW_SIZE, verify_page_attributes, with_active_page_table};
use crate::intrinsics::{
    CR4_LA57, DescriptorTablePointer, cpuid_version_info, read_cr3, read_cr4, read_gdtr, read_idtr, read_ldtr, read_msr,
};
use crate::memory::mmram::is_range_inside_mmram;
use crate::privilege_mgmt::call_gate::{
    CALL_GATE_OFFSET, CALL_GATE_TYPE_ATTR, CallGateDescriptor, LONG_CS_R0, TSS_DESC_OFFSET, TSS_SEL_OFFSET,
    TssDescriptor,
};
use crate::privilege_mgmt::save_state::SaveStateInfo;
use crate::state::security_state;

/// `IA32_SMBASE`, readable in SMM when `IA32_VMX_MISC` bit 15 is set.
const MSR_IA32_SMBASE: u32 = 0x9E;

/// `IA32_VMX_MISC`, present when CPUID.01H:ECX.VMX is set.
const MSR_IA32_VMX_MISC: u32 = 0x485;

/// `IA32_VMX_MISC` bit 15: `IA32_SMBASE` can be read in SMM.
const VMX_MISC_SMBASE_READABLE: u64 = 1 << 15;

/// CPUID.01H:ECX bit 5: the processor supports VMX.
const CPUID_ECX_VMX: u32 = 1 << 5;

/// Limit of the supervisor's IDT: 256 gates of 16 bytes.
const IDT_LIMIT: u16 = 256 * 16 - 1;

/// Limit of the GDT the entry code loads: its descriptors end where the TSS begins.
const GDT_LIMIT: u16 = TSS_DESC_OFFSET - 1;

/// CR4 bits the entry code sets before it turns paging on: UMIP (bit 11), SMEP (bit 20) and SMAP
/// (bit 21).
const CR4_ENTRY_PROTECTIONS: u64 = (1 << 11) | (1 << 20) | (1 << 21);

/// Low 16 bits of the TSS descriptor's limit, `TSS_DESC_SIZE` in `SmiException.nasm`.
const TSS_LIMIT_LOW: u16 = 0x68;

/// Limit bits 19:16 and flags of the TSS descriptor: all clear, so G = 0 and the limit is in
/// bytes, leaving no room for an I/O permission bitmap.
const TSS_LIMIT_FLAGS: u8 = 0x00;

/// Type byte of a present, DPL 0, busy 64-bit TSS. The entry code's `ltr` sets the busy bit.
const TSS_TYPE_BUSY: u8 = 0x8B;

/// A code or data segment descriptor (Intel SDM Vol. 3A 3.4.5), laid out as `GdtEntry` in
/// `patina_internal_cpu::gdt`.
#[repr(C, packed)]
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    zerocopy_derive::FromBytes,
    zerocopy_derive::IntoBytes,
    zerocopy_derive::Immutable
)]
struct SegmentDescriptor {
    limit15_0: u16,
    base15_0: u16,
    base23_16: u8,
    /// P, DPL, S and the segment type.
    type_: u8,
    /// G, D/B, L and AVL in bits 7:4, limit bits 19:16 in bits 3:0.
    limit19_16_and_flags: u8,
    base31_24: u8,
}

const NULL_SEG: SegmentDescriptor = SegmentDescriptor {
    limit15_0: 0,
    base15_0: 0,
    base23_16: 0,
    type_: 0, // NULL
    limit19_16_and_flags: 0,
    base31_24: 0,
};

const CODE32_SEG: SegmentDescriptor = SegmentDescriptor {
    limit15_0: 0xFFFF,
    base15_0: 0,
    base23_16: 0,
    type_: 0x9B,                // present, ring 0, code, execute/read, accessed
    limit19_16_and_flags: 0xCF, // page-granular, 32-bit
    base31_24: 0,
};

const DATA32_SEG: SegmentDescriptor = SegmentDescriptor {
    limit15_0: 0xFFFF,
    base15_0: 0,
    base23_16: 0,
    type_: 0x93,                // present, ring 0, data, read/write, accessed
    limit19_16_and_flags: 0xCF, // page-granular, 32-bit
    base31_24: 0,
};

const CODE16_SEG: SegmentDescriptor = SegmentDescriptor {
    limit15_0: 0xFFFF,
    base15_0: 0,
    base23_16: 0,
    type_: 0x9B,                // present, ring 0, code, execute/read, accessed
    limit19_16_and_flags: 0x8F, // page-granular, 16-bit
    base31_24: 0,
};

const DATA16_SEG: SegmentDescriptor = SegmentDescriptor {
    limit15_0: 0xFFFF,
    base15_0: 0,
    base23_16: 0,
    type_: 0x93,                // present, ring 0, data, read/write, accessed
    limit19_16_and_flags: 0x8F, // page-granular, 16-bit
    base31_24: 0,
};

const CODE64_SEG: SegmentDescriptor = SegmentDescriptor {
    limit15_0: 0xFFFF,
    base15_0: 0,
    base23_16: 0,
    type_: 0x9B,                // present, ring 0, code, execute/read, accessed
    limit19_16_and_flags: 0xAF, // page-granular, 64-bit code
    base31_24: 0,
};

const USER_CODE_PLACEHOLDER_SEG: SegmentDescriptor = SegmentDescriptor {
    limit15_0: 0xFFFF,
    base15_0: 0,
    base23_16: 0,
    type_: 0x7B,                // NOT present, ring 3, code, execute/read, accessed
    limit19_16_and_flags: 0xAF, // page-granular, 64-bit code
    base31_24: 0,
};

const USER_DATA_SEG: SegmentDescriptor = SegmentDescriptor {
    limit15_0: 0xFFFF,
    base15_0: 0,
    base23_16: 0,
    type_: 0xF3,                // present, ring 3, data, read/write, accessed
    limit19_16_and_flags: 0xCF, // page-granular, 32-bit
    base31_24: 0,
};

const USER_CODE64_SEG: SegmentDescriptor = SegmentDescriptor {
    limit15_0: 0xFFFF,
    base15_0: 0,
    base23_16: 0,
    type_: 0xFB,                // present, ring 3, code, execute/read, accessed
    limit19_16_and_flags: 0xAF, // page-granular, 64-bit code
    base31_24: 0,
};

/// The code and data segment descriptors below the call gate, in `SmiException.nasm` order. Each
/// has its accessed bit set, so loading it never makes the CPU write the read-only GDT.
const GDT_SEGMENTS: [SegmentDescriptor; 12] = [
    NULL_SEG,                  // 0x00: NullSeg
    CODE32_SEG,                // 0x08: CodeSeg32, the entry code's protected-mode CS
    CODE32_SEG,                // 0x10: ProtModeCodeSeg32
    DATA32_SEG,                // 0x18: ProtModeSsSeg32
    DATA32_SEG,                // 0x20: DataSeg32, the entry code's protected-mode DS
    CODE16_SEG,                // 0x28: CodeSeg16
    DATA16_SEG,                // 0x30: DataSeg16
    CODE64_SEG,                // 0x38: CodeSeg64R0, the Ring 0 CS and the call gate's target
    DATA32_SEG,                // 0x40: DataSeg64R0, the Ring 0 DS, ES, FS, GS and SS
    USER_CODE_PLACEHOLDER_SEG, // 0x48: CodeSeg64ComR3, the 32-bit Ring 3 CS slot of STAR's SYSRET base
    USER_DATA_SEG,             // 0x50: DataSeg64R3, the Ring 3 SS
    USER_CODE64_SEG,           // 0x58: CodeSeg64R3, the Ring 3 CS
];

/// `TXT_PROCESSOR_SMM_DESCRIPTOR_SIGNATURE`: `SIGNATURE_64('T','X','T','P','S','S','I','G')`.
const PSD_SIGNATURE: u64 = u64::from_le_bytes(*b"TXTPSSIG");

/// The processor SMM descriptor (PSD) at `SMBASE + SMM_PSD_OFFSET`, laid out as the packed
/// `TXT_PROCESSOR_SMM_DESCRIPTOR` in `StmApi.h`. The entry code loads DS, ES, FS, GS and SS from
/// it, and an STM reads the rest.
#[repr(C, packed)]
#[derive(Clone, Copy, zerocopy_derive::FromBytes, zerocopy_derive::Immutable)]
struct Psd {
    signature: u64,
    size: u16,
    smm_descriptor_ver_major: u8,
    smm_descriptor_ver_minor: u8,
    local_apic_id: u32,
    smm_entry_state: u8,
    smm_resume_state: u8,
    stm_smm_state: u8,
    reserved4: u8,
    smm_cs: u16,
    smm_ds: u16,
    smm_ss: u16,
    smm_other_segment: u16,
    smm_tr: u16,
    reserved5: u16,
    smm_cr3: u64,
    smm_stm_setup_rip: u64,
    smm_stm_teardown_rip: u64,
    smm_smi_handler_rip: u64,
    smm_smi_handler_rsp: u64,
    smm_gdt_ptr: u64,
    smm_gdt_size: u32,
    required_stm_smm_revid: u32,
    /// `STM_PROTECTION_EXCEPTION_HANDLER`: two `u64`s, a `u16`, a `u16` of flags and a `u32`.
    stm_protection_exception_handler: [u8; 24],
    reserved6: u64,
    bios_hw_resource_requirements_ptr: u64,
    acpi_rsdp: u64,
    physical_address_bits: u8,
}

/// The GDT the V5 entry code loads, followed by the TSS its TSS descriptor points at.
#[repr(C, packed)]
#[derive(Clone, Copy, zerocopy_derive::FromBytes, zerocopy_derive::Immutable)]
struct SmiGdtImage {
    /// The code and data segment descriptors, offsets `0x00` to `0x58`.
    segments: [SegmentDescriptor; 12],
    /// The call gate a demoted routine returns through, at [`CALL_GATE_OFFSET`].
    call_gate: CallGateDescriptor,
    /// The TSS descriptor, at [`TSS_SEL_OFFSET`].
    tss_descriptor: TssDescriptor,
    /// The TSS fields below IST1: a reserved dword, RSP0 to RSP2 and a reserved qword.
    tss_head: [u8; 0x24],
    /// IST1, the stack the supervisor's #DF and #PF gates switch to.
    tss_ist1: u64,
    /// The TSS fields between IST1 and the I/O map base: IST2 to IST7 and reserved fields.
    tss_tail: [u8; 0x3A],
    /// Offset of the I/O permission bitmap from the TSS base. At or above the TSS limit there is
    /// no bitmap, so every Ring 3 `in` or `out` raises #GP.
    tss_io_map_base: u16,
}

const _: () = assert!(size_of::<SmiGdtImage>() == TSS_DESC_OFFSET as usize + TSS_LIMIT_LOW as usize);

/// Errors produced while validating a core's MMI entry and the state it established.
///
/// Each one fails that core's first entry, since the entry runs again on every SMI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmiEntryValidationError {
    /// The save-state metadata that locates each core's SMBASE has not been published.
    SaveStateInfoUnavailable,
    /// `IA32_SMBASE` disagrees with the SMBASE array.
    SmBaseMismatch {
        /// Index of the core.
        cpu: usize,
        /// SMBASE the array records.
        smbase: u64,
        /// SMBASE the MSR reports.
        msr: u64,
    },
    /// A structure the MMI entry names is not entirely inside MMRAM.
    OutsideMmram {
        /// Index of the core.
        cpu: usize,
        /// What the structure is.
        structure: &'static str,
        /// Address the entry names for it.
        base: u64,
    },
    /// The fixup structure is not the version the supervisor runs with.
    UnexpectedHeaderVersion {
        /// Index of the core.
        cpu: usize,
        /// Version the header reports.
        version: u32,
    },
    /// A fixup slot, or a PSD field, holds a value other than the one the supervisor expects.
    UnexpectedFixup {
        /// Index of the core.
        cpu: usize,
        /// Name of the slot or field.
        slot: &'static str,
        /// Value the slot holds.
        value: u64,
    },
    /// A register does not hold what the MMI entry loads into it.
    RegisterMismatch {
        /// Index of the core.
        cpu: usize,
        /// Name of the register, or of its field.
        register: &'static str,
        /// Value the MMI entry loads.
        expected: u64,
        /// Value the register holds.
        found: u64,
    },
    /// A GDT entry differs from the `SmiException.nasm` template.
    GdtEntryMismatch {
        /// Index of the core.
        cpu: usize,
        /// Offset of the entry in the GDT.
        offset: usize,
    },
    /// The slot at the top of the Ring 0 stack does not hold the core's index.
    StackTopMismatch {
        /// Index of the core.
        cpu: usize,
        /// Value the slot holds.
        found: u64,
    },
    /// The TSS's I/O map base is below its limit, so Ring 3 port I/O is checked against TSS bytes
    /// rather than refused.
    TssIoBitmapPresent {
        /// Index of the core.
        cpu: usize,
        /// I/O map base the TSS holds.
        io_map_base: u16,
    },
    /// Two cores share a structure each of them must have to itself.
    SharedAcrossCores {
        /// What the structure is.
        structure: &'static str,
        /// Index of the first core.
        first: usize,
        /// Index of the second core.
        second: usize,
    },
    /// IST1 is not page aligned, or the page it starts is present. That page is the guard between
    /// the core's exception stack and its Ring 0 stack.
    StackGuardMissing {
        /// Index of the core.
        cpu: usize,
        /// IST1 from the core's TSS.
        ist1: u64,
    },
}

impl core::error::Error for MmiEntryValidationError {}

impl fmt::Display for MmiEntryValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SaveStateInfoUnavailable => {
                write!(f, "the save-state metadata that locates each core's SMBASE has not been published")
            }
            Self::SmBaseMismatch { cpu, smbase, msr } => {
                write!(f, "CPU {cpu}: IA32_SMBASE is 0x{msr:x}, but the SMBASE array records 0x{smbase:x}")
            }
            Self::OutsideMmram { cpu, structure, base } => {
                write!(f, "CPU {cpu}: the {structure} at 0x{base:x} is not inside MMRAM")
            }
            Self::UnexpectedHeaderVersion { cpu, version } => {
                write!(f, "CPU {cpu}: the MMI entry has header version {version}, not {MMI_ENTRY_STRUCT_VERSION}")
            }
            Self::UnexpectedFixup { cpu, slot, value } => {
                write!(f, "CPU {cpu}: {slot} holds unexpected value 0x{value:x}")
            }
            Self::RegisterMismatch { cpu, register, expected, found } => {
                write!(f, "CPU {cpu}: {register} is 0x{found:x}, expected 0x{expected:x}")
            }
            Self::GdtEntryMismatch { cpu, offset } => {
                write!(f, "CPU {cpu}: the GDT entry at offset 0x{offset:x} differs from the entry code's template")
            }
            Self::StackTopMismatch { cpu, found } => {
                write!(f, "CPU {cpu}: the top of the Ring 0 stack holds 0x{found:x}, not the CPU index")
            }
            Self::TssIoBitmapPresent { cpu, io_map_base } => write!(
                f,
                "CPU {cpu}: the TSS I/O map base 0x{io_map_base:x} is below the TSS limit 0x{TSS_LIMIT_LOW:x}, \
                 so Ring 3 port I/O is not refused"
            ),
            Self::SharedAcrossCores { structure, first, second } => {
                write!(f, "CPUs {first} and {second} share a {structure}")
            }
            Self::StackGuardMissing { cpu, ist1 } => {
                write!(f, "CPU {cpu}: the page at IST1 0x{ist1:x} is not a page-aligned, not-present stack guard")
            }
        }
    }
}

/// The values the fixup slots that do not name a structure must hold.
#[derive(Debug, Clone, Copy)]
struct ExpectedFixups {
    /// The core's SMBASE.
    smbase: u64,
    /// The HOB list the entry code passed to the supervisor.
    hob_list: u64,
    /// The supervisor entry point the entry code calls.
    rendezvous: u64,
    /// Whether `MSR_IA32_MISC_ENABLE` exists, which is true for every vendor but AMD.
    misc_enable_supported: bool,
    /// Whether CR4.LA57 is set.
    five_level_paging: bool,
}

/// What a core's MMI entry names, each read only after its range was shown to lie inside MMRAM.
struct MmiEntryStructures {
    /// The core's SMBASE.
    smbase: u64,
    /// The MMI entry code at `SMBASE + SMM_HANDLER_OFFSET`.
    entry: MemoryRange,
    /// The entry's fixup structure.
    fixups: MmiEntryFixups<'static>,
    /// The CR3 the entry code loads.
    cr3: u64,
    /// Where the IDT descriptor the entry code loads into IDTR lives.
    idt_descriptor: MemoryRange,
    /// The IDT descriptor.
    idtr: DescriptorTablePointer,
    /// Where the GDT descriptor the entry code loads into GDTR lives.
    gdt_descriptor: MemoryRange,
    /// The GDT descriptor.
    gdtr: DescriptorTablePointer,
    /// Where the GDT and the TSS that follows it live.
    gdt: MemoryRange,
    /// The GDT and TSS.
    gdt_image: SmiGdtImage,
    /// The slot at the top of the Ring 0 stack the entry code switches to.
    stack_top: MemoryRange,
    /// The value in that slot.
    stack_top_value: u64,
    /// The slot below IST1, the first one an exception through IST1 pushes to.
    ist1_top: MemoryRange,
}

/// Validates the MMI entry of core `cpu_index` and the state it established on this core.
/// `hob_list` is the HOB list pointer the entry code passed to the supervisor.
///
/// Checks:
/// - The core's SMI handler and save state, from `SMBASE + SMM_HANDLER_OFFSET` to the end of
///   its window, lie inside MMRAM.
/// - `IA32_SMBASE` matches the SMBASE, when VMX reports the MSR readable in SMM.
/// - The fixup slots hold the expected values ([`check_fixups`]).
/// - The GDT and TSS match the template ([`check_gdt`]), and the PSD matches the entry
///   ([`check_psd`]).
/// - CR3, IDTR and GDTR hold what the entry loads, the IDTR and GDTR limits are the supervisor's,
///   and CR4 has UMIP, SMEP and SMAP set.
/// - The Ring 0 stack top holds `cpu_index`.
/// - The entry's structures are mapped with the protections they need ([`check_protections`]).
/// - On the BSP, every core's entry passes [`check_cores_distinct`].
///
/// Must run on core `cpu_index` during its first entry, after the BSP has patched the IDT
/// descriptors. APs receive their first SMI only after BSP initialization, so they load the
/// patched descriptor.
#[inline(never)]
pub(crate) fn validate_mmi_entry(cpu_index: usize, hob_list: u64, is_bsp: bool) -> MmSupervisorResult<()> {
    check_ldtr(cpu_index, read_ldtr())?;
    let info = security_state().save_state_info().ok_or(MmiEntryValidationError::SaveStateInfoUnavailable)?;
    let entry_size = usize::try_from(info.mmi_entry_size()).map_err(|_| SmiHandlerIdtPatchError::EntrySizeTooLarge)?;
    let core = read_mmi_entry_structures(&info, cpu_index, entry_size)?;
    let smbase = core.smbase;

    // The core uses only the upper half of its 64 KiB window, for its entry code, PSD and
    // save-state map. When the SMBASEs are tiled, other cores' pieces also sit in that half. The
    // lower half overlaps other cores' windows, or lies below MMRAM for the first core.
    let owned_base = smbase.saturating_add(SMM_HANDLER_OFFSET);
    if !MemoryRange::new(owned_base, SMRAM_WINDOW_SIZE - SMM_HANDLER_OFFSET)
        .is_ok_and(|owned| is_range_inside_mmram(&owned))
    {
        return Err(MmiEntryValidationError::OutsideMmram {
            cpu: cpu_index,
            structure: "SMI handler and save state",
            base: owned_base,
        }
        .into());
    }

    if cpuid_version_info().ecx & CPUID_ECX_VMX != 0
        // SAFETY: `IA32_VMX_MISC` exists on every processor that reports VMX.
        && unsafe { read_msr(MSR_IA32_VMX_MISC) } & VMX_MISC_SMBASE_READABLE != 0
    {
        // SAFETY: `IA32_VMX_MISC` bit 15 reports that `IA32_SMBASE` can be read in SMM.
        let msr = unsafe { read_msr(MSR_IA32_SMBASE) };
        if msr != smbase {
            return Err(MmiEntryValidationError::SmBaseMismatch { cpu: cpu_index, smbase, msr }.into());
        }
    }

    // The entry code calls the supervisor's image entry point, `efi_main` in `entry_point.asm`.
    #[cfg(target_os = "uefi")]
    let rendezvous = {
        unsafe extern "C" {
            fn efi_main();
        }
        efi_main as unsafe extern "C" fn() as usize as u64
    };
    #[cfg(not(target_os = "uefi"))]
    let rendezvous = 0;

    // The vendor string "AuthenticAMD" spans EBX, EDX and ECX of CPUID leaf 0.
    let vendor = __cpuid(0);
    let is_amd = [vendor.ebx, vendor.edx, vendor.ecx]
        == [u32::from_le_bytes(*b"Auth"), u32::from_le_bytes(*b"enti"), u32::from_le_bytes(*b"cAMD")];

    let expected = ExpectedFixups {
        smbase,
        hob_list,
        rendezvous,
        misc_enable_supported: !is_amd,
        five_level_paging: read_cr4() & CR4_LA57 != 0,
    };
    check_fixups(cpu_index, &core.fixups, &expected)?;
    check_gdt(cpu_index, &core.gdt_image, core.gdtr.base)?;
    check_psd(cpu_index, smbase, core.cr3, core.gdt_descriptor.base())?;

    let (cr3, idtr, gdtr) = (read_cr3(), read_idtr(), read_gdtr());
    let registers = [
        ("CR3", core.cr3, cr3),
        ("CR4 UMIP, SMEP and SMAP", CR4_ENTRY_PROTECTIONS, read_cr4() & CR4_ENTRY_PROTECTIONS),
        ("IDTR base", core.idtr.base, idtr.base),
        ("IDTR limit", u64::from(core.idtr.limit), u64::from(idtr.limit)),
        ("IDTR limit", u64::from(IDT_LIMIT), u64::from(idtr.limit)),
        ("GDTR base", core.gdtr.base, gdtr.base),
        ("GDTR limit", u64::from(core.gdtr.limit), u64::from(gdtr.limit)),
        ("GDTR limit", u64::from(GDT_LIMIT), u64::from(gdtr.limit)),
    ];
    for (register, expected, found) in registers {
        if expected != found {
            return Err(MmiEntryValidationError::RegisterMismatch { cpu: cpu_index, register, expected, found }.into());
        }
    }

    if core.stack_top_value != cpu_index as u64 {
        return Err(MmiEntryValidationError::StackTopMismatch { cpu: cpu_index, found: core.stack_top_value }.into());
    }

    check_protections(cpu_index, &core)?;

    if is_bsp {
        check_cores_distinct(&info, entry_size, cr3)?;
    }

    log::debug!("CPU {cpu_index}: MMI entry at 0x{:016x} validated", smbase + SMM_HANDLER_OFFSET);
    Ok(())
}

/// The entry shim must have disabled the local descriptor table on this CPU.
fn check_ldtr(cpu: usize, selector: u16) -> MmSupervisorResult<()> {
    if selector != 0 {
        return Err(MmiEntryValidationError::RegisterMismatch {
            cpu,
            register: "LDTR",
            expected: 0,
            found: u64::from(selector),
        }
        .into());
    }
    Ok(())
}

/// Checks that each structure core `cpu`'s MMI entry names is mapped with the protections it
/// needs.
///
/// Checks:
/// - The IDT lies inside MMRAM.
/// - The entry code is supervisor-only, read-only and executable.
/// - The GDT and TSS are supervisor-only, read-only and non-executable.
/// - The IDT and the IDT and GDT descriptors are supervisor-only and non-executable.
/// - The tops of the Ring 0 and IST1 stacks are supervisor-only, writable and non-executable.
/// - The page IST1 starts is a not-present stack guard.
fn check_protections(cpu: usize, core: &MmiEntryStructures) -> MmSupervisorResult<()> {
    let idt = MemoryRange::new(core.idtr.base, u64::from(IDT_LIMIT) + 1)
        .ok()
        .filter(is_range_inside_mmram)
        .ok_or(MmiEntryValidationError::OutsideMmram { cpu, structure: "IDT", base: core.idtr.base })?;

    let supervisor_data = MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect;
    let protections = [
        (
            "MMI entry code",
            core.entry,
            MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly,
            MemoryAttributes::ExecuteProtect | MemoryAttributes::ReadProtect,
        ),
        // The IDT is a static in the supervisor's data section, so it is writable.
        ("IDT", idt, supervisor_data, MemoryAttributes::ReadProtect),
        ("IDT descriptor", core.idt_descriptor, supervisor_data, MemoryAttributes::ReadProtect),
        ("GDT descriptor", core.gdt_descriptor, supervisor_data, MemoryAttributes::ReadProtect),
        ("GDT", core.gdt, supervisor_data | MemoryAttributes::ReadOnly, MemoryAttributes::ReadProtect),
        ("Ring 0 stack", core.stack_top, supervisor_data, MemoryAttributes::ReadOnly | MemoryAttributes::ReadProtect),
        ("IST1 stack", core.ist1_top, supervisor_data, MemoryAttributes::ReadOnly | MemoryAttributes::ReadProtect),
    ];
    with_active_page_table(|page_table| {
        for (structure, range, required, forbidden) in protections {
            if let Err(error) = verify_page_attributes(page_table, &range, required, forbidden) {
                log::error!("CPU {cpu}: the {structure} at {range} is not mapped as it must be: {error}");
                return Err(error);
            }
        }

        // The page at IST1 is the stack guard.
        let ist1 = core.ist1_top.end();
        if !ist1.is_multiple_of(UEFI_PAGE_SIZE as u64)
            || !matches!(page_table.query_memory_region(ist1, UEFI_PAGE_SIZE as u64), Err(PtError::NoMapping))
        {
            return Err(MmiEntryValidationError::StackGuardMissing { cpu, ist1 }.into());
        }

        Ok(())
    })
}

/// Reads the MMI entry of core `cpu` and each structure its fixup slots name, after showing each
/// one lies inside MMRAM. The values read are not checked here.
fn read_mmi_entry_structures(
    info: &SaveStateInfo,
    cpu: usize,
    entry_size: usize,
) -> MmSupervisorResult<MmiEntryStructures> {
    let smbase =
        info.smbase(cpu).map_err(|_| CoreInitError::CpuIndexOutOfRange { index: cpu, len: info.number_of_cores() })?;
    // SAFETY: `locate_mmi_entry` places the entry inside MMRAM, which the supervisor owns and
    // keeps resident, and nothing rewrites it while the supervisor runs.
    let entry_bytes = unsafe { locate_mmi_entry(cpu, smbase, entry_size) }?;
    let entry = MemoryRange::new(smbase + SMM_HANDLER_OFFSET, entry_size as u64)
        .map_err(|_| SmiHandlerIdtPatchError::MmiEntryOutsideMmram { cpu })?;
    let fixups = MmiEntryFixups::parse(entry_bytes)?;

    let (idt_descriptor, idtr) =
        read_inside_mmram::<DescriptorTablePointer>(cpu, "IDT descriptor", fixups.fixup64(FIXUP64_SMI_HANDLER_IDTR)?)?;
    let (gdt_descriptor, gdtr) =
        read_inside_mmram::<DescriptorTablePointer>(cpu, "GDT descriptor", u64::from(fixups.fixup32(FIXUP32_GDTR)?))?;
    let (gdt, gdt_image) = read_inside_mmram::<SmiGdtImage>(cpu, "GDT", gdtr.base)?;
    let (stack_top, stack_top_value) =
        read_inside_mmram::<u64>(cpu, "Ring 0 stack top", u64::from(fixups.fixup32(FIXUP32_STACK_CPL0)?))?;

    let ist1 = gdt_image.tss_ist1;
    let ist1_top = ist1
        .checked_sub(size_of::<u64>() as u64)
        .and_then(|slot| MemoryRange::new(slot, size_of::<u64>() as u64).ok())
        .filter(is_range_inside_mmram)
        .ok_or(MmiEntryValidationError::OutsideMmram { cpu, structure: "IST1 stack", base: ist1 })?;

    Ok(MmiEntryStructures {
        smbase,
        entry,
        cr3: u64::from(fixups.fixup32(FIXUP32_CR3)?),
        fixups,
        idt_descriptor,
        idtr,
        gdt_descriptor,
        gdtr,
        gdt,
        gdt_image,
        stack_top,
        stack_top_value,
        ist1_top,
    })
}

/// Reads the `T` at `address` once its bytes are shown to lie inside MMRAM, and returns it with
/// its range. `structure` names it in the error.
fn read_inside_mmram<T: FromBytes>(
    cpu: usize,
    structure: &'static str,
    address: u64,
) -> MmSupervisorResult<(MemoryRange, T)> {
    let outside = MmiEntryValidationError::OutsideMmram { cpu, structure, base: address };
    let range = MemoryRange::new(address, size_of::<T>() as u64).ok().filter(is_range_inside_mmram).ok_or(outside)?;
    // SAFETY: the range was just shown to lie inside MMRAM, which the supervisor owns and keeps
    // resident.
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, size_of::<T>()) };
    let value = T::read_from_bytes(bytes).map_err(|_| outside)?;
    Ok((range, value))
}

/// Checks the fixup slots that do not name a structure against `expected`. The others are checked
/// where their structure is read.
///
/// Checks:
/// - The fixup header version is [`MMI_ENTRY_STRUCT_VERSION`].
/// - The rendezvous, HOB list, SMBASE, `MISC_ENABLE` and 5-level paging slots match `expected`.
/// - The debug, CET and XD slots hold their fixed values.
/// - Every reserved Fixup8 slot is zero.
fn check_fixups(cpu: usize, fixups: &MmiEntryFixups<'_>, expected: &ExpectedFixups) -> MmSupervisorResult<()> {
    let version = fixups.header().header_version;
    if version != MMI_ENTRY_STRUCT_VERSION {
        return Err(MmiEntryValidationError::UnexpectedHeaderVersion { cpu, version }.into());
    }

    let slots = [
        ("FIXUP64_SMM_DBG_ENTRY", fixups.fixup64(FIXUP64_SMM_DBG_ENTRY)?, 0),
        ("FIXUP64_SMM_DBG_EXIT", fixups.fixup64(FIXUP64_SMM_DBG_EXIT)?, 0),
        ("FIXUP64_SMI_RDZ_ENTRY", fixups.fixup64(FIXUP64_SMI_RDZ_ENTRY)?, expected.rendezvous),
        ("FIXUP64_XD_SUPPORTED", fixups.fixup64(FIXUP64_XD_SUPPORTED)?, 0),
        ("FIXUP64_CET_SUPPORTED", fixups.fixup64(FIXUP64_CET_SUPPORTED)?, 0),
        ("FIXUP64_HOB_START", fixups.fixup64(FIXUP64_HOB_START)?, expected.hob_list),
        ("FIXUP32_CET_PL0_SSP", u64::from(fixups.fixup32(FIXUP32_CET_PL0_SSP)?), 0),
        ("FIXUP32_CET_INTERRUPT_SSP", u64::from(fixups.fixup32(FIXUP32_CET_INTERRUPT_SSP)?), 0),
        ("FIXUP32_CET_INTERRUPT_SSP_TABLE", u64::from(fixups.fixup32(FIXUP32_CET_INTERRUPT_SSP_TABLE)?), 0),
        ("FIXUP32_MSR_SMM_BASE", u64::from(fixups.fixup32(FIXUP32_MSR_SMM_BASE)?), expected.smbase),
        ("FIXUP8_XD_SUPPORTED", u64::from(fixups.fixup8(FIXUP8_XD_SUPPORTED)?), 1),
        (
            "FIXUP8_MISC_ENABLE_SUPPORTED",
            u64::from(fixups.fixup8(FIXUP8_MISC_ENABLE_SUPPORTED)?),
            u64::from(expected.misc_enable_supported),
        ),
        (
            "FIXUP8_5_LEVEL_PAGING",
            u64::from(fixups.fixup8(FIXUP8_5_LEVEL_PAGING)?),
            u64::from(expected.five_level_paging),
        ),
        ("FIXUP8_CET_SUPPORTED", u64::from(fixups.fixup8(FIXUP8_CET_SUPPORTED)?), 0),
    ];
    for (slot, value, want) in slots {
        if value != want {
            return Err(MmiEntryValidationError::UnexpectedFixup { cpu, slot, value }.into());
        }
    }

    for index in FIXUP8_FIRST_RESERVED..usize::from(fixups.header().fixup8_num) {
        let value = fixups.fixup8(index)?;
        if value != 0 {
            let slot = "a reserved Fixup8 slot";
            return Err(MmiEntryValidationError::UnexpectedFixup { cpu, slot, value: value.into() }.into());
        }
    }

    Ok(())
}

/// Checks the GDT and TSS at `gdt_base` against the `SmiException.nasm` template.
///
/// Checks:
/// - Every segment descriptor matches the template.
/// - The call gate selector, IST, type and reserved fields match the template.
/// - The TSS descriptor matches the template and points at the TSS that follows the GDT.
/// - The TSS has no I/O permission bitmap.
///
/// The call gate's target is not checked, because the supervisor sets it on each demotion.
fn check_gdt(cpu: usize, gdt: &SmiGdtImage, gdt_base: u64) -> MmSupervisorResult<()> {
    let segments = gdt.segments;
    if let Some(index) = segments.iter().zip(GDT_SEGMENTS).position(|(found, expected)| *found != expected) {
        return Err(
            MmiEntryValidationError::GdtEntryMismatch { cpu, offset: index * size_of::<SegmentDescriptor>() }.into()
        );
    }

    let gate = gdt.call_gate;
    if gate.selector != LONG_CS_R0 || gate.ist != 0 || gate.type_attr != CALL_GATE_TYPE_ATTR || gate.reserved != 0 {
        return Err(MmiEntryValidationError::GdtEntryMismatch { cpu, offset: CALL_GATE_OFFSET.into() }.into());
    }

    let tss = gdt.tss_descriptor;
    if tss.limit_low != TSS_LIMIT_LOW
        || tss.limit_flags != TSS_LIMIT_FLAGS
        || tss.type_attr != TSS_TYPE_BUSY
        || tss.reserved != 0
        || Some(tss.base()) != gdt_base.checked_add(TSS_DESC_OFFSET.into())
    {
        return Err(MmiEntryValidationError::GdtEntryMismatch { cpu, offset: TSS_SEL_OFFSET.into() }.into());
    }

    // The limit is byte-granular, so a base at or above it leaves Ring 3 no port access except
    // through the policy-checked syscall.
    let io_map_base = gdt.tss_io_map_base;
    if io_map_base < TSS_LIMIT_LOW {
        return Err(MmiEntryValidationError::TssIoBitmapPresent { cpu, io_map_base }.into());
    }

    Ok(())
}

/// Checks the PSD at `smbase + SMM_PSD_OFFSET` against the MMI entry.
///
/// Checks:
/// - The PSD lies inside MMRAM.
/// - It carries the PSD signature.
/// - Its CR3 and GDT descriptor are `cr3` and `gdt_descriptor`, the ones the entry code loads.
fn check_psd(cpu: usize, smbase: u64, cr3: u64, gdt_descriptor: u64) -> MmSupervisorResult<()> {
    let (_, psd) = read_inside_mmram::<Psd>(cpu, "PSD", smbase.saturating_add(SMM_PSD_OFFSET))?;

    let fields = [
        ("PSD Signature", psd.signature, PSD_SIGNATURE),
        ("PSD SmmCr3", psd.smm_cr3, cr3),
        ("PSD SmmGdtPtr", psd.smm_gdt_ptr, gdt_descriptor),
    ];
    for (slot, value, expected) in fields {
        if value != expected {
            return Err(MmiEntryValidationError::UnexpectedFixup { cpu, slot, value }.into());
        }
    }

    Ok(())
}

/// Checks every core's MMI entry from the BSP, before any Ring 3 code runs.
///
/// Checks:
/// - Every core's entry loads `bsp_cr3`.
/// - Every core's structures pass [`check_protections`].
/// - No two cores share a GDT, a GDT descriptor, a Ring 0 stack or an IST1 stack.
///
/// A core's own check on its first entry comes too late, because its entry code has already used
/// the GDT and Ring 0 stack with paging off. Stack sizes are not known, so two stacks count as
/// shared when their tops fall in the same page.
fn check_cores_distinct(info: &SaveStateInfo, entry_size: usize, bsp_cr3: u64) -> MmSupervisorResult<()> {
    let page = |range: &MemoryRange| range.base() / UEFI_PAGE_SIZE as u64;
    let count = info.number_of_cores();
    for first in 0..count {
        let a = read_mmi_entry_structures(info, first, entry_size)?;
        if a.cr3 != bsp_cr3 {
            return Err(
                MmiEntryValidationError::UnexpectedFixup { cpu: first, slot: "FIXUP32_CR3", value: a.cr3 }.into()
            );
        }
        check_protections(first, &a)?;

        for second in first + 1..count {
            let b = read_mmi_entry_structures(info, second, entry_size)?;
            let shared = [
                ("GDT", a.gdt.overlaps(&b.gdt)),
                ("GDT descriptor", a.gdt_descriptor.overlaps(&b.gdt_descriptor)),
                ("Ring 0 stack", page(&a.stack_top) == page(&b.stack_top)),
                ("IST1 stack", page(&a.ist1_top) == page(&b.ist1_top)),
            ];
            if let Some((structure, _)) = shared.into_iter().find(|(_, shared)| *shared) {
                return Err(MmiEntryValidationError::SharedAcrossCores { structure, first, second }.into());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    use serial_test::serial;
    use zerocopy::{FromZeros, IntoBytes};

    use crate::memory;
    use crate::test_support::{self, PageAlignedMemory, init_global_state_over};

    const SMBASE: u64 = 0x7F00_0000;
    const HOB_LIST: u64 = 0x7E00_0000;
    const RENDEZVOUS: u64 = 0x7F80_1000;

    const EXPECTED: ExpectedFixups = ExpectedFixups {
        smbase: SMBASE,
        hob_list: HOB_LIST,
        rendezvous: RENDEZVOUS,
        misc_enable_supported: true,
        five_level_paging: false,
    };

    /// The Fixup64 array `SmmCpuFeaturesInstallSmiHandler` writes for a V5 entry.
    fn fixup64(idt_descriptor: u64) -> [u64; 7] {
        [0, 0, RENDEZVOUS, 0, 0, idt_descriptor, HOB_LIST]
    }

    /// The Fixup32 array `SmmCpuFeaturesInstallSmiHandler` writes for a V5 entry.
    fn fixup32(gdt_descriptor: u32, cr3: u32, stack_top: u32, smbase: u32) -> [u32; 7] {
        [0, gdt_descriptor, cr3, 0, 0, stack_top, smbase]
    }

    /// The Fixup8 array `SmmCpuFeaturesInstallSmiHandler` writes for a V5 entry on Intel without
    /// 5-level paging.
    const FIXUP8: [u8; 8] = [1, 1, 0, 0, 0, 0, 0, 0];

    /// The GDT and TSS `InitGdt` builds at `gdt_base`, as a core that has run its entry code sees
    /// them, with IST1 set to `ist1` and the I/O map base at the TSS limit.
    fn template_gdt(gdt_base: u64, ist1: u64) -> SmiGdtImage {
        let mut call_gate = CallGateDescriptor::default();
        call_gate.set_return_gate(0);
        let mut tss_descriptor = TssDescriptor {
            limit_low: TSS_LIMIT_LOW,
            type_attr: TSS_TYPE_BUSY,
            limit_flags: TSS_LIMIT_FLAGS,
            ..Default::default()
        };
        tss_descriptor.set_base(gdt_base + u64::from(TSS_DESC_OFFSET));
        SmiGdtImage {
            segments: GDT_SEGMENTS,
            call_gate,
            tss_descriptor,
            tss_head: [0; 0x24],
            tss_ist1: ist1,
            tss_tail: [0; 0x3A],
            tss_io_map_base: TSS_LIMIT_LOW,
        }
    }

    #[test]
    fn test_mmi_entry_validation_errors_render_each_variant_distinctly() {
        let cases = [
            (
                MmiEntryValidationError::SaveStateInfoUnavailable,
                "the save-state metadata that locates each core's SMBASE has not been published",
            ),
            (
                MmiEntryValidationError::SmBaseMismatch { cpu: 1, smbase: 0x3000, msr: 0x4000 },
                "CPU 1: IA32_SMBASE is 0x4000, but the SMBASE array records 0x3000",
            ),
            (
                MmiEntryValidationError::OutsideMmram { cpu: 1, structure: "GDT", base: 0x1000 },
                "CPU 1: the GDT at 0x1000 is not inside MMRAM",
            ),
            (
                MmiEntryValidationError::UnexpectedHeaderVersion { cpu: 1, version: 4 },
                "CPU 1: the MMI entry has header version 4, not 5",
            ),
            (
                MmiEntryValidationError::UnexpectedFixup { cpu: 1, slot: "FIXUP64_HOB_START", value: 0x10 },
                "CPU 1: FIXUP64_HOB_START holds unexpected value 0x10",
            ),
            (
                MmiEntryValidationError::RegisterMismatch { cpu: 1, register: "CR3", expected: 0x1000, found: 0x2000 },
                "CPU 1: CR3 is 0x2000, expected 0x1000",
            ),
            (
                MmiEntryValidationError::GdtEntryMismatch { cpu: 1, offset: 0x70 },
                "CPU 1: the GDT entry at offset 0x70 differs from the entry code's template",
            ),
            (
                MmiEntryValidationError::StackTopMismatch { cpu: 1, found: 2 },
                "CPU 1: the top of the Ring 0 stack holds 0x2, not the CPU index",
            ),
            (
                MmiEntryValidationError::TssIoBitmapPresent { cpu: 1, io_map_base: 0 },
                "CPU 1: the TSS I/O map base 0x0 is below the TSS limit 0x68, so Ring 3 port I/O is not refused",
            ),
            (
                MmiEntryValidationError::SharedAcrossCores { structure: "GDT", first: 0, second: 3 },
                "CPUs 0 and 3 share a GDT",
            ),
            (
                MmiEntryValidationError::StackGuardMissing { cpu: 1, ist1: 0x7F60_1000 },
                "CPU 1: the page at IST1 0x7f601000 is not a page-aligned, not-present stack guard",
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(format!("{error}"), expected);
        }
    }

    #[test]
    fn test_smi_gdt_image_matches_the_template_layout() {
        assert_eq!(size_of::<SmiGdtImage>(), 0xE8);
        assert_eq!(core::mem::offset_of!(SmiGdtImage, call_gate), usize::from(CALL_GATE_OFFSET));
        assert_eq!(core::mem::offset_of!(SmiGdtImage, tss_descriptor), usize::from(TSS_SEL_OFFSET));
        // IST1 sits 0x24 bytes into the TSS, and the I/O map base 0x66.
        assert_eq!(core::mem::offset_of!(SmiGdtImage, tss_ist1), usize::from(TSS_DESC_OFFSET) + 0x24);
        assert_eq!(core::mem::offset_of!(SmiGdtImage, tss_io_map_base), usize::from(TSS_DESC_OFFSET) + 0x66);
    }

    #[test]
    fn test_check_ldtr_requires_the_null_selector() {
        assert_eq!(check_ldtr(2, 0), Ok(()));
        for selector in [0x28, 0xfff8] {
            assert_eq!(
                check_ldtr(2, selector),
                Err(MmiEntryValidationError::RegisterMismatch {
                    cpu: 2,
                    register: "LDTR",
                    expected: 0,
                    found: u64::from(selector)
                }
                .into())
            );
        }
    }

    #[test]
    fn test_gdt_segments_encode_the_template_descriptors() {
        // The descriptors `SmiException.nasm` assembles, as the 64-bit values the CPU reads.
        const TEMPLATE: [u64; 12] = [
            0,
            0x00CF_9B00_0000_FFFF,
            0x00CF_9B00_0000_FFFF,
            0x00CF_9300_0000_FFFF,
            0x00CF_9300_0000_FFFF,
            0x008F_9B00_0000_FFFF,
            0x008F_9300_0000_FFFF,
            0x00AF_9B00_0000_FFFF,
            0x00CF_9300_0000_FFFF,
            0x00AF_7B00_0000_FFFF,
            0x00CF_F300_0000_FFFF,
            0x00AF_FB00_0000_FFFF,
        ];

        assert_eq!(size_of::<SegmentDescriptor>(), 8);
        for (index, (segment, expected)) in GDT_SEGMENTS.iter().zip(TEMPLATE).enumerate() {
            let bytes: [u8; 8] = segment.as_bytes().try_into().unwrap();
            assert_eq!(u64::from_le_bytes(bytes), expected, "GDT offset 0x{:x}", index * 8);
        }
    }

    #[test]
    fn test_psd_matches_the_stm_api_layout() {
        assert_eq!(size_of::<Psd>(), 0x89);
        // The entry code loads DS, SS, and ES, FS and GS from these offsets (`DSC_DS`, `DSC_SS` and
        // `DSC_OTHERSEG`).
        assert_eq!(core::mem::offset_of!(Psd, smm_ds), 0x16);
        assert_eq!(core::mem::offset_of!(Psd, smm_ss), 0x18);
        assert_eq!(core::mem::offset_of!(Psd, smm_other_segment), 0x1A);
        assert_eq!(core::mem::offset_of!(Psd, smm_cr3), 0x20);
        assert_eq!(core::mem::offset_of!(Psd, smm_gdt_ptr), 0x48);
    }

    #[test]
    fn test_check_fixups_accepts_the_values_init_writes() {
        let entry = test_support::mmi_entry_with(
            &fixup64(0x7F10_0000),
            &fixup32(0x7F20_0000, 0x7F30_0000, 0x7F40_0FF8, SMBASE as u32),
            &FIXUP8,
        );
        let fixups = MmiEntryFixups::parse(&entry).unwrap();

        assert_eq!(check_fixups(1, &fixups, &EXPECTED), Ok(()));

        // AMD has no MSR_IA32_MISC_ENABLE, and LA57 follows CR4.
        let mut fixup8 = FIXUP8;
        fixup8[FIXUP8_MISC_ENABLE_SUPPORTED] = 0;
        fixup8[FIXUP8_5_LEVEL_PAGING] = 1;
        let entry = test_support::mmi_entry_with(
            &fixup64(0x7F10_0000),
            &fixup32(0x7F20_0000, 0x7F30_0000, 0x7F40_0FF8, SMBASE as u32),
            &fixup8,
        );
        let fixups = MmiEntryFixups::parse(&entry).unwrap();
        let amd_la57 = ExpectedFixups { misc_enable_supported: false, five_level_paging: true, ..EXPECTED };
        assert_eq!(check_fixups(1, &fixups, &amd_la57), Ok(()));
    }

    #[test]
    fn test_check_fixups_rejects_each_unexpected_slot() {
        let fixup32 = fixup32(0x7F20_0000, 0x7F30_0000, 0x7F40_0FF8, SMBASE as u32);
        let unexpected = |slot: &'static str, value: u64| -> MmSupervisorResult<()> {
            Err(MmiEntryValidationError::UnexpectedFixup { cpu: 2, slot, value }.into())
        };

        let cases64 = [
            (FIXUP64_SMM_DBG_ENTRY, "FIXUP64_SMM_DBG_ENTRY", 0x1000),
            (FIXUP64_SMM_DBG_EXIT, "FIXUP64_SMM_DBG_EXIT", 0x1000),
            (FIXUP64_SMI_RDZ_ENTRY, "FIXUP64_SMI_RDZ_ENTRY", RENDEZVOUS + 1),
            (FIXUP64_XD_SUPPORTED, "FIXUP64_XD_SUPPORTED", 0x1000),
            (FIXUP64_CET_SUPPORTED, "FIXUP64_CET_SUPPORTED", 0x1000),
            (FIXUP64_HOB_START, "FIXUP64_HOB_START", HOB_LIST + 8),
        ];
        for (index, slot, value) in cases64 {
            let mut fixup64 = fixup64(0x7F10_0000);
            fixup64[index] = value;
            let entry = test_support::mmi_entry_with(&fixup64, &fixup32, &FIXUP8);
            let fixups = MmiEntryFixups::parse(&entry).unwrap();
            assert_eq!(check_fixups(2, &fixups, &EXPECTED), unexpected(slot, value), "{slot}");
        }

        let cases32 = [
            (FIXUP32_CET_PL0_SSP, "FIXUP32_CET_PL0_SSP", 0x1000),
            (FIXUP32_CET_INTERRUPT_SSP, "FIXUP32_CET_INTERRUPT_SSP", 0x1000),
            (FIXUP32_CET_INTERRUPT_SSP_TABLE, "FIXUP32_CET_INTERRUPT_SSP_TABLE", 0x1000),
            (FIXUP32_MSR_SMM_BASE, "FIXUP32_MSR_SMM_BASE", SMBASE as u32 + 0x2000),
        ];
        for (index, slot, value) in cases32 {
            let mut fixup32 = fixup32;
            fixup32[index] = value;
            let entry = test_support::mmi_entry_with(&fixup64(0x7F10_0000), &fixup32, &FIXUP8);
            let fixups = MmiEntryFixups::parse(&entry).unwrap();
            assert_eq!(check_fixups(2, &fixups, &EXPECTED), unexpected(slot, value.into()), "{slot}");
        }

        let cases8 = [
            (FIXUP8_XD_SUPPORTED, "FIXUP8_XD_SUPPORTED", 0),
            (FIXUP8_MISC_ENABLE_SUPPORTED, "FIXUP8_MISC_ENABLE_SUPPORTED", 0),
            (FIXUP8_5_LEVEL_PAGING, "FIXUP8_5_LEVEL_PAGING", 1),
            (FIXUP8_CET_SUPPORTED, "FIXUP8_CET_SUPPORTED", 1),
            (FIXUP8_FIRST_RESERVED, "a reserved Fixup8 slot", 1),
            (FIXUP8.len() - 1, "a reserved Fixup8 slot", 1),
        ];
        for (index, slot, value) in cases8 {
            let mut fixup8 = FIXUP8;
            fixup8[index] = value;
            let entry = test_support::mmi_entry_with(&fixup64(0x7F10_0000), &fixup32, &fixup8);
            let fixups = MmiEntryFixups::parse(&entry).unwrap();
            assert_eq!(check_fixups(2, &fixups, &EXPECTED), unexpected(slot, value.into()), "{slot}");
        }
    }

    #[test]
    fn test_check_fixups_rejects_another_version_or_short_arrays() {
        let fixup32 = fixup32(0x7F20_0000, 0x7F30_0000, 0x7F40_0FF8, SMBASE as u32);
        let mut entry = test_support::mmi_entry_with(&fixup64(0x7F10_0000), &fixup32, &FIXUP8);
        // The header follows the 8-byte stand-in for the entry code.
        entry[8..12].copy_from_slice(&4_u32.to_ne_bytes());
        let fixups = MmiEntryFixups::parse(&entry).unwrap();
        assert_eq!(
            check_fixups(2, &fixups, &EXPECTED),
            Err(MmiEntryValidationError::UnexpectedHeaderVersion { cpu: 2, version: 4 }.into())
        );

        // A V5 entry missing its HOB slot is rejected rather than read past its array.
        let entry = test_support::mmi_entry_with(&fixup64(0x7F10_0000)[..FIXUP64_HOB_START], &fixup32, &FIXUP8);
        let fixups = MmiEntryFixups::parse(&entry).unwrap();
        assert_eq!(
            check_fixups(2, &fixups, &EXPECTED),
            Err(SmiHandlerIdtPatchError::FixupArrayTooSmall {
                width: 64,
                index: FIXUP64_HOB_START,
                found: FIXUP64_HOB_START as u8
            }
            .into())
        );
    }

    #[test]
    fn test_check_gdt_accepts_the_template() {
        assert_eq!(check_gdt(1, &template_gdt(0x7F50_0000, 0x7F60_1000), 0x7F50_0000), Ok(()));

        // The call gate's target is programmed on demotion, so it may hold anything.
        let mut gdt = template_gdt(0x7F50_0000, 0x7F60_1000);
        gdt.call_gate.set_offset(0xFFFF_8000_0000_1000);
        assert_eq!(check_gdt(1, &gdt, 0x7F50_0000), Ok(()));
    }

    #[test]
    fn test_check_gdt_rejects_each_altered_entry() {
        const BASE: u64 = 0x7F50_0000;
        let mismatch = |offset: usize| -> MmSupervisorResult<()> {
            Err(MmiEntryValidationError::GdtEntryMismatch { cpu: 1, offset }.into())
        };

        for index in 0..GDT_SEGMENTS.len() {
            let mut gdt = template_gdt(BASE, 0x7F60_1000);
            let mut segments = gdt.segments;
            // Flip the DPL bits, which turns a Ring 0 segment into a Ring 3 one.
            segments[index].type_ ^= 0x60;
            gdt.segments = segments;
            assert_eq!(check_gdt(1, &gdt, BASE), mismatch(index * 8), "segment {index}");
        }

        let gate = usize::from(CALL_GATE_OFFSET);
        let mut gdt = template_gdt(BASE, 0x7F60_1000);
        gdt.call_gate.selector = 0x58;
        assert_eq!(check_gdt(1, &gdt, BASE), mismatch(gate));
        let mut gdt = template_gdt(BASE, 0x7F60_1000);
        gdt.call_gate.ist = 1;
        assert_eq!(check_gdt(1, &gdt, BASE), mismatch(gate));
        let mut gdt = template_gdt(BASE, 0x7F60_1000);
        gdt.call_gate.type_attr = 0x8C;
        assert_eq!(check_gdt(1, &gdt, BASE), mismatch(gate));
        let mut gdt = template_gdt(BASE, 0x7F60_1000);
        gdt.call_gate.reserved = 1;
        assert_eq!(check_gdt(1, &gdt, BASE), mismatch(gate));

        let tss = usize::from(TSS_SEL_OFFSET);
        let mut gdt = template_gdt(BASE, 0x7F60_1000);
        gdt.tss_descriptor.limit_low = 0xFFFF;
        assert_eq!(check_gdt(1, &gdt, BASE), mismatch(tss));
        let mut gdt = template_gdt(BASE, 0x7F60_1000);
        gdt.tss_descriptor.limit_flags = 0x8F;
        assert_eq!(check_gdt(1, &gdt, BASE), mismatch(tss));
        // The old template's G = 1, which scales the limit to 0x68FFF.
        let mut gdt = template_gdt(BASE, 0x7F60_1000);
        gdt.tss_descriptor.limit_flags = 0x80;
        assert_eq!(check_gdt(1, &gdt, BASE), mismatch(tss));
        // A TSS that is not busy was not the one this core's `ltr` loaded.
        let mut gdt = template_gdt(BASE, 0x7F60_1000);
        gdt.tss_descriptor.type_attr = 0x89;
        assert_eq!(check_gdt(1, &gdt, BASE), mismatch(tss));
        let mut gdt = template_gdt(BASE, 0x7F60_1000);
        gdt.tss_descriptor.reserved = 1;
        assert_eq!(check_gdt(1, &gdt, BASE), mismatch(tss));
        // The TSS must be the one that follows this GDT.
        assert_eq!(check_gdt(1, &template_gdt(BASE, 0x7F60_1000), BASE + 0x1000), mismatch(tss));
        assert_eq!(check_gdt(1, &template_gdt(BASE, 0x7F60_1000), u64::MAX), mismatch(tss));

        // An I/O map base below the limit leaves a bitmap made of the TSS's own bytes, with the old
        // template's 0 as the worst case. Anything at or above the limit means no bitmap.
        for io_map_base in [0, TSS_LIMIT_LOW - 1] {
            let mut gdt = template_gdt(BASE, 0x7F60_1000);
            gdt.tss_io_map_base = io_map_base;
            assert_eq!(
                check_gdt(1, &gdt, BASE),
                Err(MmiEntryValidationError::TssIoBitmapPresent { cpu: 1, io_map_base }.into())
            );
        }
        let mut gdt = template_gdt(BASE, 0x7F60_1000);
        gdt.tss_io_map_base = u16::MAX;
        assert_eq!(check_gdt(1, &gdt, BASE), Ok(()));
    }

    /// Allocates `pages` pages of the test MMRAM.
    fn allocate(pages: usize) -> u64 {
        security_state().page_allocator().allocate_pages(pages).expect("pages in the test MMRAM")
    }

    /// Writes `value` at `address`, which must be a live allocation large enough to hold it.
    fn place<T>(address: u64, value: T) {
        // SAFETY: each caller passes an allocation from `allocate` with room for `T`.
        unsafe { core::ptr::write_unaligned(address as *mut T, value) };
    }

    /// Copies `entry` to `SMBASE + 0x8000` of a fresh SMRAM window in the test MMRAM, records
    /// that SMBASE in a one-entry SMBASE array, and returns the metadata that describes it.
    fn place_single_core(entry: &[u8]) -> SaveStateInfo {
        let smbase = allocate((SMRAM_WINDOW_SIZE / UEFI_PAGE_SIZE as u64) as usize);
        // SAFETY: the SMRAM window is a live 64 KiB allocation, and the entry ends well inside it
        // because it starts at `SMM_HANDLER_OFFSET`, halfway through.
        unsafe {
            core::ptr::copy_nonoverlapping(entry.as_ptr(), (smbase + SMM_HANDLER_OFFSET) as *mut u8, entry.len());
        }
        let array = allocate(1);
        place(array, smbase);
        // SAFETY: the array holds one initialized SMBASE and stays allocated for the test, and no
        // save-state map is read through it.
        unsafe { SaveStateInfo::new(1, array, entry.len() as u64) }.expect("a one-entry SMBASE array")
    }

    #[test]
    #[serial]
    fn test_read_inside_mmram_reads_only_from_mmram() {
        test_support::init_test_logger();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        init_global_state_over(&memory);

        let page = allocate(1);
        place(page + 8, 0x1122_3344_5566_7788_u64);
        let (range, value) = read_inside_mmram::<u64>(1, "slot", page + 8).expect("the slot is in MMRAM");
        assert_eq!((range, value), (MemoryRange::new(page + 8, 8).unwrap(), 0x1122_3344_5566_7788));

        // Outside MMRAM and wrapping the address space are both refused before anything is read.
        // A range straddling an MMRAM boundary panics in `is_range_inside_mmram` by design.
        for address in [0x1000, u64::MAX - 3] {
            assert_eq!(
                read_inside_mmram::<u64>(1, "slot", address),
                Err(MmiEntryValidationError::OutsideMmram { cpu: 1, structure: "slot", base: address }.into()),
                "address 0x{address:x}"
            );
        }
    }

    #[test]
    #[serial]
    fn test_read_mmi_entry_structures_refuses_a_slot_outside_mmram() {
        test_support::init_test_logger();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 64);
        init_global_state_over(&memory);

        let idt_descriptor = allocate(1);
        place(idt_descriptor, DescriptorTablePointer { limit: IDT_LIMIT, base: 0x7F70_0000 });
        // Host test memory lies above 4 GiB, out of reach of the Fixup32 slots, so this checks
        // that each slot is refused, in order, before it is read.
        let fixup32 = fixup32(0x1000, 0x7F30_0000, 0x2000, SMBASE as u32);
        let cases = [(fixup64(0x1000), "IDT descriptor", 0x1000), (fixup64(idt_descriptor), "GDT descriptor", 0x1000)];
        for (fixup64, structure, base) in cases {
            let info = place_single_core(&test_support::mmi_entry_with(&fixup64, &fixup32, &FIXUP8));
            let entry_size = info.mmi_entry_size() as usize;
            assert_eq!(
                read_mmi_entry_structures(&info, 0, entry_size).err(),
                Some(MmiEntryValidationError::OutsideMmram { cpu: 0, structure, base }.into()),
                "{structure}"
            );
        }

        // A CPU past the end of the SMBASE array is not looked up at all.
        let info = place_single_core(&test_support::mmi_entry_with(&fixup64(idt_descriptor), &fixup32, &FIXUP8));
        assert_eq!(
            read_mmi_entry_structures(&info, 1, info.mmi_entry_size() as usize).err(),
            Some(CoreInitError::CpuIndexOutOfRange { index: 1, len: 1 }.into())
        );
    }

    #[test]
    #[serial]
    fn test_check_cores_distinct_fails_on_a_core_it_cannot_read() {
        test_support::init_test_logger();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 32);
        init_global_state_over(&memory);

        // The BSP refuses the whole set when any core's IDT descriptor lies outside MMRAM.
        let fixup32 = fixup32(0x1000, 0x7F30_0000, 0x2000, SMBASE as u32);
        let info = place_single_core(&test_support::mmi_entry_with(&fixup64(0x1000), &fixup32, &FIXUP8));
        assert_eq!(
            check_cores_distinct(&info, info.mmi_entry_size() as usize, 0x7F30_0000),
            Err(MmiEntryValidationError::OutsideMmram { cpu: 0, structure: "IDT descriptor", base: 0x1000 }.into())
        );
    }

    #[test]
    #[serial]
    fn test_check_psd_compares_the_psd_with_the_mmi_entry() {
        const CR3: u64 = 0x7F30_0000;
        const GDT_DESCRIPTOR: u64 = 0x7F20_0000;

        test_support::init_test_logger();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 32);
        init_global_state_over(&memory);

        let smbase = allocate((SMRAM_WINDOW_SIZE / UEFI_PAGE_SIZE as u64) as usize);
        let mut psd = Psd::new_zeroed();
        psd.signature = PSD_SIGNATURE;
        psd.smm_cr3 = CR3;
        psd.smm_gdt_ptr = GDT_DESCRIPTOR;
        place(smbase + SMM_PSD_OFFSET, psd);
        assert_eq!(check_psd(1, smbase, CR3, GDT_DESCRIPTOR), Ok(()));

        let unexpected = |slot: &'static str, value: u64| -> MmSupervisorResult<()> {
            Err(MmiEntryValidationError::UnexpectedFixup { cpu: 1, slot, value }.into())
        };
        assert_eq!(check_psd(1, smbase, CR3 + 0x1000, GDT_DESCRIPTOR), unexpected("PSD SmmCr3", CR3));
        assert_eq!(check_psd(1, smbase, CR3, GDT_DESCRIPTOR + 0x10), unexpected("PSD SmmGdtPtr", GDT_DESCRIPTOR));

        let mut unsigned = psd;
        unsigned.signature = 0;
        place(smbase + SMM_PSD_OFFSET, unsigned);
        assert_eq!(check_psd(1, smbase, CR3, GDT_DESCRIPTOR), unexpected("PSD Signature", 0));

        // A PSD outside MMRAM is refused before it is read.
        assert_eq!(
            check_psd(1, 0x1000, CR3, GDT_DESCRIPTOR),
            Err(MmiEntryValidationError::OutsideMmram { cpu: 1, structure: "PSD", base: 0x1000 + SMM_PSD_OFFSET }
                .into())
        );
    }

    #[test]
    #[serial]
    fn test_check_protections_requires_each_structure_mapped_as_the_entry_code_needs() {
        use crate::hob::validation::HobValidationError;

        test_support::init_test_logger();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 32);
        init_global_state_over(&memory);

        // One supervisor-only, writable, non-executable page per structure, then the entry code
        // and the GDT remapped the way init leaves them. The IST1 stack gets the page above it as
        // well, unmapped as the stack guard IST1 points at.
        let [entry, idt, idt_descriptor, gdt_descriptor, gdt, stack] = [(); 6].map(|()| allocate(1));
        let ist1 = allocate(2);
        let guard = ist1 + UEFI_PAGE_SIZE as u64;
        let map = |base: u64, attributes: MemoryAttributes| {
            let mut page_table = security_state().lock_page_table();
            let page_table = page_table.as_mut().expect("a page table is installed");
            page_table.map_memory_region(base, UEFI_PAGE_SIZE as u64, attributes).expect("map the page");
        };
        let unmap = |base: u64| {
            let mut page_table = security_state().lock_page_table();
            let page_table = page_table.as_mut().expect("a page table is installed");
            page_table.unmap_memory_region(base, UEFI_PAGE_SIZE as u64).expect("unmap the page");
        };
        let found = |base: u64| {
            let page_table = security_state().lock_page_table();
            let page_table = page_table.as_ref().expect("a page table is installed");
            page_table.query_memory_region(base, UEFI_PAGE_SIZE as u64).expect("query the page").bits()
        };
        let supervisor_data = MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect;
        map(entry, MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly);
        map(gdt, supervisor_data | MemoryAttributes::ReadOnly);
        unmap(guard);

        // `check_protections` reads nothing through the fixups, so they hold the usual test values.
        let fixup32 = fixup32(0x7F20_0000, 0x7F30_0000, 0x7F40_0FF8, SMBASE as u32);
        let entry_bytes: &'static [u8] =
            Box::leak(test_support::mmi_entry_with(&fixup64(0x7F10_0000), &fixup32, &FIXUP8).into_boxed_slice());
        let span = |base: u64, size: usize| MemoryRange::new(base, size as u64).unwrap();
        let core = |idt_base: u64| MmiEntryStructures {
            smbase: SMBASE,
            entry: span(entry, entry_bytes.len()),
            fixups: MmiEntryFixups::parse(entry_bytes).unwrap(),
            cr3: 0x7F30_0000,
            idt_descriptor: span(idt_descriptor, size_of::<DescriptorTablePointer>()),
            idtr: DescriptorTablePointer { limit: IDT_LIMIT, base: idt_base },
            gdt_descriptor: span(gdt_descriptor, size_of::<DescriptorTablePointer>()),
            gdtr: DescriptorTablePointer { limit: GDT_LIMIT, base: gdt },
            gdt: span(gdt, size_of::<SmiGdtImage>()),
            gdt_image: template_gdt(gdt, guard),
            stack_top: span(stack + 0xFF8, size_of::<u64>()),
            stack_top_value: 3,
            ist1_top: span(ist1 + 0xFF8, size_of::<u64>()),
        };
        assert_eq!(check_protections(3, &core(idt)), Ok(()));

        // The IDT must lie in MMRAM before its page is looked at.
        assert_eq!(
            check_protections(3, &core(0x1000)),
            Err(MmiEntryValidationError::OutsideMmram { cpu: 3, structure: "IDT", base: 0x1000 }.into())
        );

        // Entry code Ring 0 could rewrite and run.
        map(entry, MemoryAttributes::Supervisor);
        assert_eq!(
            check_protections(3, &core(idt)),
            Err(HobValidationError::PageWritableAndExecutable { addr: entry, found: found(entry) }.into())
        );
        map(entry, MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly);

        // A writable GDT, and a Ring 0 stack Ring 3 can reach.
        map(gdt, supervisor_data);
        let desired = (supervisor_data | MemoryAttributes::ReadOnly).bits();
        assert_eq!(
            check_protections(3, &core(idt)),
            Err(HobValidationError::PageMissingAttribute { addr: gdt, desired, found: found(gdt) }.into())
        );
        map(gdt, supervisor_data | MemoryAttributes::ReadOnly);
        map(stack, MemoryAttributes::ExecuteProtect);
        assert_eq!(
            check_protections(3, &core(idt)),
            Err(HobValidationError::PageMissingAttribute {
                addr: stack,
                desired: supervisor_data.bits(),
                found: found(stack)
            }
            .into())
        );
        map(stack, supervisor_data);

        // IST1 must start a page, and that page must not be present, so an overflow of the Ring 0
        // stack above it faults rather than running into the exception stack below it.
        let unaligned = MmiEntryStructures { ist1_top: span(ist1 + 0xFF0, size_of::<u64>()), ..core(idt) };
        assert_eq!(
            check_protections(3, &unaligned),
            Err(MmiEntryValidationError::StackGuardMissing { cpu: 3, ist1: ist1 + 0xFF8 }.into())
        );
        map(guard, supervisor_data);
        assert_eq!(
            check_protections(3, &core(idt)),
            Err(MmiEntryValidationError::StackGuardMissing { cpu: 3, ist1: guard }.into())
        );
    }
}
