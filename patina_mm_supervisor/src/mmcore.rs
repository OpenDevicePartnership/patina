//! MM Supervisor Core
//!
//! Each core passes through three phases:
//!
//! - [`entry`]: construction and the MM entry point every core arrives through.
//! - [`init`]: one-time setup on a core's first entry.
//! - [`runtime`]: request dispatch and the AP holding pen on every later entry.
//!
//! The remaining modules hold the CPU and mailbox bookkeeping, the SMRR, MSEG and SMI IDT setup,
//! and the per-core MMI entry validation.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub(crate) mod cpu;
pub(crate) mod entry;
pub(crate) mod init;
pub(crate) mod mailbox;
pub(crate) mod mseg;
pub(crate) mod per_core;
pub(crate) mod perf_timer;
pub(crate) mod request_target;
pub(crate) mod runtime;
pub(crate) mod smi_idt_patch;
pub(crate) mod smrr;
pub(crate) mod validation;

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicUsize};

use patina::error::EfiError;

use crate::{PlatformInfo, privilege_mgmt::syscall_setup::SyscallInterface};

use self::{cpu::CpuManager, mailbox::MailboxManager};

/// A failure during per-core bring-up, on either the BSP or an AP.
///
/// Faults in the HOBs and buffers a core is handed report through their own types, such as
/// [`HobValidationError`](crate::hob::validation::HobValidationError).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreInitError {
    /// The per-core initialized buffer has not been published yet.
    InitializedBufferUnavailable,
    /// The CPU index is outside the per-core array it selects a slot in.
    ///
    /// Used for the initialized buffer, the [`CpuManager`] slots and the SMBASE array, so `len`
    /// is the length of whichever array was indexed.
    CpuIndexOutOfRange {
        /// The index the core entered with.
        index: usize,
        /// Number of slots the indexed array holds.
        len: usize,
    },
    /// The CPU index is in range but its slot is already held by a different APIC ID.
    /// Re-registering the same APIC ID is not an error.
    CpuIndexAlreadyRegistered {
        /// The contested CPU index.
        index: usize,
        /// APIC ID already occupying the slot.
        existing: u32,
        /// APIC ID that tried to claim it.
        requested: u32,
    },
    /// A core tried to register before the BSP published the CPU slots.
    CpuSlotsUnavailable,
    /// The CPU slots were already published.
    CpuSlotsAlreadyPublished,
    /// The BSP found no configured user entry point to demote to.
    UserEntryPointMissing,
    /// The HOB list described no MM Init module allocation. HOB validation already rejects such a
    /// list, so this is not expected after it.
    InitModuleRegionMissing,
    /// The MM Init module allocation is empty or extends past the end of the address space. HOB
    /// validation already rejects such an allocation.
    InitModuleRegionInvalid {
        /// Base address the allocation HOB reported.
        base: u64,
        /// Length the allocation HOB reported.
        length: u64,
    },
    /// The MP Information HOB was not present in the HOB list.
    MpInformationHobMissing,
    /// The MP Information HOB's payload does not lie inside MMRAM.
    MpInformationHobOutsideMmram,
    /// The MP Information HOB is too short, or its processor entries do not fit its payload.
    MpInformationHobMalformed,
    /// The MP Information HOB reports zero cores.
    ZeroCoreCount,
    /// The per-core initialized buffer the `PassDown` HOB describes is not one supervisor-only
    /// slot per CPU inside MMRAM, or its address does not fit the target architecture.
    InitializedBufferInvalid,
    /// The walked HOB list is too long to copy for the user core on this architecture.
    HobListTooLargeToCopy {
        /// Byte length of the list, as walked from its headers.
        size: u64,
    },
    /// No page table is installed, so a region cannot be mapped for Ring 3.
    PageTableUnavailable,
    /// The HOB list copy could not be mapped read-only, so Ring 3 must not be given it.
    HobListCopyNotMapped {
        /// Address of the copy that could not be mapped.
        base: u64,
        /// Byte length of the copy.
        size: u64,
    },
    /// The per-core Ring 3 stack size in the `PassDown` HOB does not fit the target architecture.
    Cpl3StackRegionInvalid {
        /// Base address the HOB reported.
        base: u64,
        /// Per-core stack size the HOB reported.
        per_core_size: u64,
        /// Number of CPUs the stacks were sized for.
        number_of_cores: usize,
    },
    /// The page table hierarchy at the active CR3 could not be adopted.
    PageTableFromCr3Failed {
        /// The CR3 value that was read.
        cr3: u64,
    },
    /// The policy gate was not initialized before the security policy check.
    PolicyGateUnavailable,
    /// Per-core init ran before the BSP recorded the SMRR range.
    SmrrRangeUnavailable,
    /// The interrupt manager could not be initialized.
    InterruptManagerInit(EfiError),
}

impl core::error::Error for CoreInitError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::InterruptManagerInit(err) => Some(err),
            _ => None,
        }
    }
}

impl fmt::Display for CoreInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InitializedBufferUnavailable => {
                write!(f, "the per-core initialized buffer has not been published yet")
            }
            Self::CpuIndexOutOfRange { index, len } => {
                write!(f, "CPU index {index} is outside the {len}-slot per-core array")
            }
            Self::CpuIndexAlreadyRegistered { index, existing, requested } => {
                write!(
                    f,
                    "CPU index {index} is already registered to APIC {existing}, cannot register APIC {requested}"
                )
            }
            Self::CpuSlotsUnavailable => write!(f, "the CPU slots have not been published yet"),
            Self::CpuSlotsAlreadyPublished => write!(f, "the CPU slots were already published"),
            Self::UserEntryPointMissing => write!(f, "no user entry point is configured for the BSP to demote to"),
            Self::InitModuleRegionMissing => write!(f, "the HOB list described no MM Init module allocation"),
            Self::InitModuleRegionInvalid { base, length } => {
                write!(f, "the MM Init module allocation at 0x{base:016x} (+0x{length:x}) is not a valid range")
            }
            Self::MpInformationHobMissing => write!(f, "the MP Information HOB is missing from the HOB list"),
            Self::MpInformationHobOutsideMmram => write!(f, "the MP Information HOB payload is not inside MMRAM"),
            Self::MpInformationHobMalformed => {
                write!(f, "the MP Information HOB is too short or its processor entries do not fit")
            }
            Self::ZeroCoreCount => write!(f, "the MP Information HOB reports zero cores"),
            Self::InitializedBufferInvalid => {
                write!(f, "the per-core initialized buffer is not addressable or not resident in MMRAM")
            }
            Self::HobListTooLargeToCopy { size } => {
                write!(f, "the HOB list of 0x{size:x} bytes does not fit the target architecture")
            }
            Self::PageTableUnavailable => {
                write!(f, "no page table is installed to map a region for the user core")
            }
            Self::HobListCopyNotMapped { base, size } => {
                write!(f, "the HOB list copy at 0x{base:016x} (0x{size:x} bytes) could not be mapped read-only")
            }
            Self::Cpl3StackRegionInvalid { base, per_core_size, number_of_cores } => {
                write!(
                    f,
                    "the PassDown HOB describes no usable Ring 3 stack region \
                     (base 0x{base:016x}, 0x{per_core_size:x} bytes per core, {number_of_cores} cores)"
                )
            }
            Self::PageTableFromCr3Failed { cr3 } => {
                write!(f, "the page table at CR3 0x{cr3:016x} could not be adopted")
            }
            Self::PolicyGateUnavailable => {
                write!(f, "the policy gate was not initialized before the security policy check")
            }
            Self::SmrrRangeUnavailable => write!(f, "per-core init ran before the BSP recorded the SMRR range"),
            Self::InterruptManagerInit(err) => write!(f, "the interrupt manager could not be initialized: {err}"),
        }
    }
}

/// The MM Supervisor Core for a standalone MM environment.
///
/// Initializes the BSP, dispatches requests, and drives the APs through the holding pen and
/// mailboxes. [`PlatformInfo`] supplies the platform's extra MMI handlers. The per-core arrays are
/// allocated during BSP initialization, so the core itself can live in a `static`.
///
/// ## Examples
///
/// Create a static instance and call `entry_point` from every core:
///
/// ```rust,no_run
/// # #[cfg(target_arch = "x86_64")]
/// # mod example {
/// use core::ffi::c_void;
/// use patina_mm_supervisor::*;
///
/// struct MyPlatform;
///
/// impl PlatformInfo for MyPlatform {}
///
/// static SUPERVISOR: MmSupervisorCore<MyPlatform> = MmSupervisorCore::new();
///
/// // The MM IPL invokes this entry point on every core.
/// pub extern "efiapi" fn mm_entry(cpu_index: usize, hob_list: *const c_void) {
///     // SAFETY: invoked once per core by the MM environment with a valid HOB list.
///     unsafe { SUPERVISOR.entry_point(cpu_index, hob_list) };
/// }
/// # }
/// ```
pub struct MmSupervisorCore<P: PlatformInfo> {
    /// Registered CPUs and their states.
    pub(crate) cpu_manager: CpuManager,
    /// Per-AP command mailboxes.
    pub(crate) mailbox_manager: MailboxManager,
    /// Syscall interface and per-CPU Ring 3 stacks.
    pub(crate) syscall_interface: SyscallInterface,
    /// Set once the BSP has finished its one-time initialization.
    pub(crate) initialized: AtomicBool,
    /// Number of cores the MP Information HOB reports. Zero until the BSP has read that HOB.
    pub(crate) number_of_cores: AtomicUsize,
    /// Ties the instance to its platform type.
    pub(crate) _phantom: core::marker::PhantomData<fn() -> P>,
}

impl<P: PlatformInfo> Default for MmSupervisorCore<P> {
    fn default() -> Self {
        Self::new()
    }
}
