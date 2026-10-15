//! MM Supervisor Core
//!
//! The phases, in the order a core passes through them:
//!
//! - [`entry`] - construction and the MM entry point every core arrives through
//! - [`init`] - the one-time setup a core runs on its first entry
//! - [`runtime`] - the dispatch loop and AP holding pen used on every later entry
//!
//! `entry_point` decides which of the other two a given core needs, so the three modules form a
//! sequence rather than a hierarchy and do not call sideways into each other.
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
pub(crate) mod perf_timer;
pub(crate) mod request_target;
pub(crate) mod runtime;
pub(crate) mod semaphore;
pub(crate) mod smi_idt_patch;
pub(crate) mod smrr;

use core::fmt;
use core::sync::atomic::AtomicBool;

use patina::error::EfiError;

use spin::Mutex;

use crate::{PlatformInfo, privilege_mgmt::syscall_setup::SyscallInterface};

use self::{cpu::CpuManager, mailbox::MailboxManager};

/// A failure during per-core bring-up, on either the BSP or an AP.
///
/// These describe the state of a single core's entry into the supervisor, as opposed to the
/// failures of the artifacts it is handed, which each report through their own type:
/// [`PassDownHobError`](crate::hob::pass_down::PassDownHobError),
/// [`CommBufferError`](crate::comm_buffer::CommBufferError), and
/// [`HobValidationError`](crate::hob::validation::HobValidationError).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreInitError {
    /// The per-core initialized buffer has not been published yet, so no core's initialization
    /// state can be read or recorded.
    InitializedBufferUnavailable,
    /// The CPU index is outside the per-core array it selects a slot in.
    ///
    /// Used for both the initialized buffer and the [`CpuManager`](crate::mmcore::cpu::CpuManager) slot
    /// array, so `len` is the length of whichever array was indexed.
    CpuIndexOutOfRange {
        /// The index the core entered with.
        index: usize,
        /// Number of slots the indexed array holds.
        len: usize,
    },
    /// The CPU index is in range but its slot is already held by a different APIC ID.
    ///
    /// Distinct from [`CoreInitError::CpuIndexOutOfRange`]: the index is valid, but two cores
    /// claim the same dense processor index. Re-registering the *same* APIC ID is idempotent and
    /// is not an error.
    CpuIndexAlreadyRegistered {
        /// The contested CPU index.
        index: usize,
        /// APIC ID already occupying the slot.
        existing: u32,
        /// APIC ID that tried to claim it.
        requested: u32,
    },
    /// The BSP found no configured user entry point to demote to.
    UserEntryPointMissing,
    /// The HOB list described no MM Init module allocation.
    ///
    /// `validate_incoming_hobs_pre_paging_init` already rejects a HOB list missing this module, so
    /// reaching this means discovery ran against a list that validation never accepted.
    InitModuleRegionMissing,
    /// The MM Init module allocation is empty or extends past the end of the address space.
    ///
    /// Validation rejects such an allocation too, so this is the same situation as
    /// [`CoreInitError::InitModuleRegionMissing`].
    ///
    /// Holds the raw base and length rather than a `MemoryRange`, because this is reported when
    /// `MemoryRange::new` rejects them.
    InitModuleRegionInvalid {
        /// Base address the allocation HOB reported.
        base: u64,
        /// Length the allocation HOB reported.
        length: u64,
    },
    /// The MP Information HOB was not present in the HOB list.
    MpInformationHobMissing,
    /// The MP Information HOB is too short, or its processor entries do not fit its payload.
    MpInformationHobMalformed,
    /// The CPU count the producer reported cannot be used to size the per-core arrays.
    InvalidCpuCount {
        /// CPU count reported by the HOB.
        found: usize,
        /// Maximum CPU count this supervisor instance supports.
        maximum: usize,
    },
    /// The per-core initialized buffer the `PassDown` HOB describes is not usable: its address
    /// does not fit the target architecture, or it does not hold one slot per CPU inside MMRAM.
    InitializedBufferInvalid,
    /// The walked HOB list is too long to copy for the user core on this architecture.
    HobListTooLargeToCopy {
        /// Byte length of the list, as walked from its headers.
        size: u64,
    },
    /// No page table is installed, so a region cannot be mapped for Ring 3.
    PageTableUnavailable,
    /// The HOB list copy could not be mapped read-only, so Ring 3 must not be given it.
    ///
    /// Reported rather than ignored because the copy is reachable from Ring 3 only once it is
    /// mapped, and a copy mapped any other way would be writable by the code it describes.
    HobListCopyNotMapped {
        /// Address of the copy that could not be mapped.
        base: u64,
        /// Byte length of the copy.
        size: u64,
    },
    /// The `PassDown` HOB does not describe a usable Ring 3 stack region.
    ///
    /// Covers a zero base or per-core size, a total that overflows, a size that does not fit the
    /// target architecture, and a range that cannot be page-aligned. All four mean the same thing
    /// to the caller: there is nowhere to demote to.
    Cpl3StackRegionInvalid {
        /// Base address the HOB reported.
        base: u64,
        /// Per-core stack size the HOB reported.
        per_core_size: u64,
        /// Number of CPUs the stacks were sized for.
        num_cpus: usize,
    },
    /// The Ring 3 stack region is not entirely inside MMRAM.
    ///
    /// The producer names this range, so mapping it user-accessible without this check would hand
    /// Ring 3 write access to whatever lies outside MMRAM at that address.
    Cpl3StackRegionOutsideMmram {
        /// The stack region, widened to whole pages.
        range: MemoryRange,
    },
    /// The Ring 3 stacks could not be mapped user-accessible, or are not usable after mapping.
    Cpl3StacksNotMapped {
        /// The stack region, widened to whole pages.
        range: MemoryRange,
    },
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
            Self::UserEntryPointMissing => write!(f, "no user entry point is configured for the BSP to demote to"),
            Self::InitModuleRegionMissing => write!(f, "the HOB list described no MM Init module allocation"),
            Self::InitModuleRegionInvalid { base, length } => {
                write!(f, "the MM Init module allocation at 0x{base:016x} (+0x{length:x}) is not a valid range")
            }
            Self::MpInformationHobMissing => write!(f, "the MP Information HOB is missing from the HOB list"),
            Self::MpInformationHobMalformed => {
                write!(f, "the MP Information HOB is too short or its processor entries do not fit")
            }
            Self::InvalidCpuCount { found, maximum } => {
                write!(f, "the producer reported {found} CPUs, more than the supported maximum of {maximum}")
            }
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
            Self::Cpl3StackRegionInvalid { base, per_core_size, num_cpus } => {
                write!(
                    f,
                    "the PassDown HOB describes no usable Ring 3 stack region \
                     (base 0x{base:016x}, 0x{per_core_size:x} bytes per core, {num_cpus} CPUs)"
                )
            }
            Self::Cpl3StackRegionOutsideMmram { range } => {
                write!(f, "the Ring 3 stacks at {range} are not inside MMRAM")
            }
            Self::Cpl3StacksNotMapped { range } => write!(f, "the Ring 3 stacks at {range} are not usable by Ring 3"),
            Self::InterruptManagerInit(err) => write!(f, "the interrupt manager could not be initialized: {err}"),
        }
    }
}

/// The MM Supervisor Core responsible for managing the standalone MM environment.
///
/// This struct is generic over the [`PlatformInfo`] trait, which provides platform-specific
/// configuration including compile-time constants for array sizes.
///
/// The supervisor manages:
/// - BSP initialization and request handling
/// - AP management through the holding pen and mailbox system
/// - Request dispatching and response handling
///
/// ## Memory Model
///
/// This struct does not perform heap allocation. All internal structures use fixed-size
/// arrays sized by the `MAX_CPUS` const generic parameter.
///
/// ## Usage
///
/// Create a static instance of the supervisor and call `entry_point` from all cores:
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
/// // The const generic argument is the maximum CPU count used to size internal arrays.
/// static SUPERVISOR: MmSupervisorCore<MyPlatform, 8> = MmSupervisorCore::new();
///
/// // The MM IPL invokes this entry point on every core.
/// pub extern "efiapi" fn mm_entry(cpu_index: usize, hob_list: *const c_void) {
///     // SAFETY: invoked once per core by the MM environment with a valid HOB list.
///     unsafe { SUPERVISOR.entry_point(cpu_index, hob_list) };
/// }
/// # }
/// ```
pub struct MmSupervisorCore<P: PlatformInfo, const MAX_CPUS: usize> {
    /// Manager for CPU-related operations.
    pub(crate) cpu_manager: CpuManager<MAX_CPUS>,
    /// Manager for AP mailboxes.
    pub(crate) mailbox_manager: MailboxManager<MAX_CPUS>,
    /// Syscall interface for privilege transitions.
    pub(crate) syscall_interface: SyscallInterface<MAX_CPUS>,
    /// Flag indicating if the core has been initialized.
    pub(crate) initialized: AtomicBool,
    /// TESTING: serializes per-core initialization so only one core runs it at a time.
    pub(crate) init_lock: Mutex<()>,
    /// Phantom data for the platform type.
    pub(crate) _phantom: core::marker::PhantomData<fn() -> P>,
}

impl<P: PlatformInfo, const MAX_CPUS: usize> Default for MmSupervisorCore<P, MAX_CPUS> {
    fn default() -> Self {
        Self::new()
    }
}
