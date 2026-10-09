//! Aggregate MM Supervisor error type.
//!
//! This module provides [`MmSupervisorError`], a single enum that groups the error types declared
//! across the supervisor's subsystems. Each variant names a subsystem and carries that subsystem's
//! existing error enum unchanged, so no detail is lost when an error is widened into the aggregate.
//!
//! ## Coverage
//!
//! Every error type the supervisor declares is represented here, either as its own variant or
//! nested inside one. The external [`EfiError`](patina::error::EfiError) is reached through
//! [`CoreInitError::InterruptManagerInit`], because it describes a stage of core bring-up rather
//! than a subsystem of its own.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{
    comm_buffer::CommBufferError,
    handlers::supv_request::unblock_memory::{PageUpdateError, UnblockError},
    hob::{pass_down::PassDownHobError, validation::HobValidationError},
    memory::{AllocError, mmram::MmramBoundError},
    mmcore::{CoreInitError, mailbox::MailboxError, smi_idt_patch::SmiHandlerIdtPatchError, smrr::SmrrError},
    policy::{
        PolicyGateError,
        validation::{PageTableWalkError, PolicyValidationError},
    },
    privilege_mgmt::{call_gate::CallGateError, syscall_setup::SyscallSetupError},
    state::save_state::SaveStateValidationError,
};
use core::fmt;

/// Result of any fallible MM Supervisor operation.
///
/// Every subsystem returns this, so a caller propagates with `?` across subsystem boundaries and
/// the aggregate [`MmSupervisorError`] records which one failed.
pub type MmSupervisorResult<T> = Result<T, MmSupervisorError>;

/// An error reported by any MM Supervisor subsystem.
///
/// Variants are grouped by the subsystem that produces them. Each one wraps that subsystem's own
/// error enum, so matching on the inner value gives the same detail the subsystem reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmSupervisorError {
    /// Validation of the incoming HOB list failed.
    HobValidation(HobValidationError),
    /// A communication channel could not be built from the HOB list.
    CommBuffer(CommBufferError),
    /// The `PassDown` HOB could not be used.
    PassDownHob(PassDownHobError),
    /// Per-core bring-up failed on the BSP or an AP.
    CoreInit(CoreInitError),
    /// A page or paging-structure allocation or free request failed.
    Alloc(AllocError),
    /// The policy gate rejected a request, or could not be constructed from the policy blob.
    PolicyGate(PolicyGateError),
    /// The one-time security validation of the policy blob failed.
    PolicyValidation(PolicyValidationError),
    /// A page table walk could not be completed.
    PageTableWalk(PageTableWalkError),
    /// Syscall entry points could not be configured.
    SyscallSetup(SyscallSetupError),
    /// The `PassDown` HOB did not describe usable save-state regions.
    SaveStateValidation(SaveStateValidationError),
    /// The MMRAM bound could not be established from the incoming SMRAM descriptors.
    MmramBound(MmramBoundError),
    /// The SMRRs could not be programmed to protect MMRAM.
    Smrr(SmrrError),
    /// The GDT privilege transition entries could not be programmed.
    CallGate(CallGateError),
    /// The SMI handler IDT patch could not be applied.
    SmiHandlerIdtPatch(SmiHandlerIdtPatchError),
    /// A command could not be posted to an AP's mailbox.
    Mailbox(MailboxError),
    /// A request to unblock memory for Ring 3 was rejected.
    Unblock(UnblockError),
    /// The page table could not be updated for an unblock request.
    PageUpdate(PageUpdateError),
}

impl core::error::Error for MmSupervisorError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::HobValidation(err) => Some(err),
            Self::CommBuffer(err) => Some(err),
            Self::PassDownHob(err) => Some(err),
            Self::CoreInit(err) => Some(err),
            Self::Alloc(err) => Some(err),
            Self::PolicyGate(err) => Some(err),
            Self::PolicyValidation(err) => Some(err),
            Self::PageTableWalk(err) => Some(err),
            Self::SyscallSetup(err) => Some(err),
            Self::SaveStateValidation(err) => Some(err),
            Self::MmramBound(err) => Some(err),
            Self::Smrr(err) => Some(err),
            Self::CallGate(err) => Some(err),
            Self::SmiHandlerIdtPatch(err) => Some(err),
            Self::Mailbox(err) => Some(err),
            Self::Unblock(err) => Some(err),
            Self::PageUpdate(err) => Some(err),
        }
    }
}

impl fmt::Display for MmSupervisorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The subsystem names the stage that failed; the wrapped error says what went wrong there.
        match self {
            Self::HobValidation(err) => write!(f, "HOB validation: {err}"),
            Self::CommBuffer(err) => write!(f, "Communication buffer: {err}"),
            Self::PassDownHob(err) => write!(f, "PassDown HOB: {err}"),
            Self::CoreInit(err) => write!(f, "Core bring-up: {err}"),
            Self::Alloc(err) => write!(f, "Alloc allocation: {err}"),
            Self::PolicyGate(err) => write!(f, "Policy gate: {err}"),
            Self::PolicyValidation(err) => write!(f, "Policy validation: {err}"),
            Self::PageTableWalk(err) => write!(f, "Page table walk: {err}"),
            Self::SyscallSetup(err) => write!(f, "Syscall setup: {err}"),
            Self::SaveStateValidation(err) => write!(f, "Save-state validation: {err}"),
            Self::MmramBound(err) => write!(f, "MMRAM bound: {err}"),
            Self::Smrr(err) => write!(f, "SMRR Programming: {err}"),
            Self::CallGate(err) => write!(f, "Call gate setup: {err}"),
            Self::SmiHandlerIdtPatch(err) => write!(f, "SMI handler IDT patch: {err}"),
            Self::Mailbox(err) => write!(f, "AP mailbox: {err}"),
            Self::Unblock(err) => write!(f, "Unblock memory: {err}"),
            Self::PageUpdate(err) => write!(f, "Unblock page table update: {err}"),
        }
    }
}

impl From<HobValidationError> for MmSupervisorError {
    fn from(error: HobValidationError) -> Self {
        Self::HobValidation(error)
    }
}

impl From<CommBufferError> for MmSupervisorError {
    fn from(error: CommBufferError) -> Self {
        Self::CommBuffer(error)
    }
}

impl From<PassDownHobError> for MmSupervisorError {
    fn from(error: PassDownHobError) -> Self {
        Self::PassDownHob(error)
    }
}

impl From<CoreInitError> for MmSupervisorError {
    fn from(error: CoreInitError) -> Self {
        Self::CoreInit(error)
    }
}

impl From<AllocError> for MmSupervisorError {
    fn from(error: AllocError) -> Self {
        Self::Alloc(error)
    }
}

impl From<PolicyGateError> for MmSupervisorError {
    fn from(error: PolicyGateError) -> Self {
        Self::PolicyGate(error)
    }
}

impl From<PolicyValidationError> for MmSupervisorError {
    fn from(error: PolicyValidationError) -> Self {
        Self::PolicyValidation(error)
    }
}

impl From<PageTableWalkError> for MmSupervisorError {
    fn from(error: PageTableWalkError) -> Self {
        Self::PageTableWalk(error)
    }
}

impl From<SyscallSetupError> for MmSupervisorError {
    fn from(error: SyscallSetupError) -> Self {
        Self::SyscallSetup(error)
    }
}

impl From<SaveStateValidationError> for MmSupervisorError {
    fn from(error: SaveStateValidationError) -> Self {
        Self::SaveStateValidation(error)
    }
}

impl From<SmrrError> for MmSupervisorError {
    fn from(error: SmrrError) -> Self {
        Self::Smrr(error)
    }
}

impl From<MmramBoundError> for MmSupervisorError {
    fn from(error: MmramBoundError) -> Self {
        Self::MmramBound(error)
    }
}

impl From<CallGateError> for MmSupervisorError {
    fn from(error: CallGateError) -> Self {
        Self::CallGate(error)
    }
}

impl From<SmiHandlerIdtPatchError> for MmSupervisorError {
    fn from(error: SmiHandlerIdtPatchError) -> Self {
        Self::SmiHandlerIdtPatch(error)
    }
}

impl From<MailboxError> for MmSupervisorError {
    fn from(error: MailboxError) -> Self {
        Self::Mailbox(error)
    }
}

impl From<UnblockError> for MmSupervisorError {
    fn from(error: UnblockError) -> Self {
        Self::Unblock(error)
    }
}

impl From<PageUpdateError> for MmSupervisorError {
    fn from(error: PageUpdateError) -> Self {
        Self::PageUpdate(error)
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_mm_supervisor_error_wraps_each_subsystem_error() {
        assert_eq!(
            MmSupervisorError::from(HobValidationError::NoMmramRegions),
            MmSupervisorError::HobValidation(HobValidationError::NoMmramRegions)
        );
        assert_eq!(
            MmSupervisorError::from(CommBufferError::Missing),
            MmSupervisorError::CommBuffer(CommBufferError::Missing)
        );
        assert_eq!(
            MmSupervisorError::from(PassDownHobError::TooSmall),
            MmSupervisorError::PassDownHob(PassDownHobError::TooSmall)
        );
        assert_eq!(
            MmSupervisorError::from(CoreInitError::InitializedBufferUnavailable),
            MmSupervisorError::CoreInit(CoreInitError::InitializedBufferUnavailable)
        );
        assert_eq!(
            MmSupervisorError::from(MmramBoundError::NoSmrrRange),
            MmSupervisorError::MmramBound(MmramBoundError::NoSmrrRange)
        );
        assert_eq!(MmSupervisorError::from(AllocError::OutOfMemory), MmSupervisorError::Alloc(AllocError::OutOfMemory));
        assert_eq!(
            MmSupervisorError::from(AllocError::PoolTooSmall),
            MmSupervisorError::Alloc(AllocError::PoolTooSmall)
        );
        assert_eq!(
            MmSupervisorError::from(PolicyGateError::AccessDenied),
            MmSupervisorError::PolicyGate(PolicyGateError::AccessDenied)
        );
        assert_eq!(
            MmSupervisorError::from(PolicyValidationError::UnrecognizedHeaderBits),
            MmSupervisorError::PolicyValidation(PolicyValidationError::UnrecognizedHeaderBits)
        );
        assert_eq!(
            MmSupervisorError::from(PageTableWalkError::InvalidCr3),
            MmSupervisorError::PageTableWalk(PageTableWalkError::InvalidCr3)
        );
        assert_eq!(
            MmSupervisorError::from(SyscallSetupError::NotInitialized),
            MmSupervisorError::SyscallSetup(SyscallSetupError::NotInitialized)
        );
        assert_eq!(
            MmSupervisorError::from(SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 0, smbase: 0x3000 }),
            MmSupervisorError::SaveStateValidation(SaveStateValidationError::UnusableSaveStateRegion {
                cpu_index: 0,
                smbase: 0x3000
            })
        );
        assert_eq!(
            MmSupervisorError::from(SmrrError::SmrrUnsupported),
            MmSupervisorError::Smrr(SmrrError::SmrrUnsupported)
        );
        assert_eq!(
            MmSupervisorError::from(CallGateError::GdtTooSmall),
            MmSupervisorError::CallGate(CallGateError::GdtTooSmall)
        );
        assert_eq!(
            MmSupervisorError::from(SmiHandlerIdtPatchError::EntryTooSmall),
            MmSupervisorError::SmiHandlerIdtPatch(SmiHandlerIdtPatchError::EntryTooSmall)
        );
        assert_eq!(
            MmSupervisorError::from(MailboxError::CommandAlreadyPending { index: 2 }),
            MmSupervisorError::Mailbox(MailboxError::CommandAlreadyPending { index: 2 })
        );
        assert_eq!(
            MmSupervisorError::from(UnblockError::OverlapsWithMmram),
            MmSupervisorError::Unblock(UnblockError::OverlapsWithMmram)
        );
        assert_eq!(
            MmSupervisorError::from(PageUpdateError::AlreadyMapped),
            MmSupervisorError::PageUpdate(PageUpdateError::AlreadyMapped)
        );
    }

    #[test]
    fn test_mm_supervisor_error_distinguishes_subsystems() {
        assert_ne!(
            MmSupervisorError::Alloc(AllocError::NotInitialized),
            MmSupervisorError::Alloc(AllocError::PoolTooSmall)
        );
        assert_ne!(
            MmSupervisorError::PolicyGate(PolicyGateError::InvalidVersion),
            MmSupervisorError::PolicyValidation(PolicyValidationError::UnrecognizedHeaderBits)
        );
    }

    #[test]
    fn test_mm_supervisor_error_displays_the_subsystem_and_the_cause() {
        // Every variant names its subsystem and then defers to the wrapped error's own message,
        // so widening never loses the detail the subsystem reported. The table is exhaustive so a
        // new variant added without a Display arm fails here rather than printing the wrong stage.
        let cases: [(MmSupervisorError, &str); 17] = [
            (HobValidationError::NoMmramRegions.into(), "HOB validation: "),
            (CommBufferError::Missing.into(), "Communication buffer: "),
            (PassDownHobError::TooSmall.into(), "PassDown HOB: "),
            (CoreInitError::UserEntryPointMissing.into(), "Core bring-up: "),
            (AllocError::OutOfMemory.into(), "Alloc allocation: "),
            (PolicyGateError::AccessDenied.into(), "Policy gate: "),
            (PolicyValidationError::UnrecognizedHeaderBits.into(), "Policy validation: "),
            (PageTableWalkError::InvalidCr3.into(), "Page table walk: "),
            (SyscallSetupError::NotInitialized.into(), "Syscall setup: "),
            (
                SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 0, smbase: 0x3000 }.into(),
                "Save-state validation: ",
            ),
            (MmramBoundError::NoSmrrRange.into(), "MMRAM bound: "),
            (SmrrError::SmrrUnsupported.into(), "SMRR Programming: "),
            (CallGateError::GdtTooSmall.into(), "Call gate setup: "),
            (SmiHandlerIdtPatchError::EntryTooSmall.into(), "SMI handler IDT patch: "),
            (MailboxError::CommandAlreadyPending { index: 2 }.into(), "AP mailbox: "),
            (UnblockError::OverlapsWithMmram.into(), "Unblock memory: "),
            (PageUpdateError::AlreadyMapped.into(), "Unblock page table update: "),
        ];

        for (error, prefix) in cases {
            let rendered = format!("{error}");
            assert!(rendered.starts_with(prefix), "{rendered:?} should start with {prefix:?}");
            // The wrapped message follows the prefix rather than being replaced by it.
            assert!(rendered.len() > prefix.len(), "{rendered:?} carries no message from the subsystem");
        }

        // The two policy stages are told apart by their prefix, so a reader can see which one
        // rejected the blob without reading the rest of the message.
        let gate = format!("{}", MmSupervisorError::from(PolicyGateError::InvalidVersion));
        let validation = format!("{}", MmSupervisorError::from(PolicyValidationError::UnrecognizedHeaderBits));
        assert!(gate.starts_with("Policy gate: "));
        assert!(validation.starts_with("Policy validation: "));
        assert_ne!(gate, validation);
    }

    #[test]
    fn test_mm_supervisor_error_exposes_the_subsystem_error_as_its_source() {
        use core::error::Error;

        // Every variant wraps a subsystem error, so a source is always reachable.
        let errors = [
            MmSupervisorError::from(HobValidationError::NoMmramRegions),
            MmSupervisorError::from(CommBufferError::Missing),
            MmSupervisorError::from(PassDownHobError::TooSmall),
            MmSupervisorError::from(CoreInitError::UserEntryPointMissing),
            MmSupervisorError::from(AllocError::OutOfMemory),
            MmSupervisorError::from(PolicyGateError::AccessDenied),
            MmSupervisorError::from(PolicyValidationError::UnrecognizedHeaderBits),
            MmSupervisorError::from(PageTableWalkError::InvalidCr3),
            MmSupervisorError::from(SyscallSetupError::NotInitialized),
            MmSupervisorError::from(SaveStateValidationError::UnusableSaveStateRegion { cpu_index: 0, smbase: 0 }),
            MmSupervisorError::from(MmramBoundError::NoSmrrRange),
            MmSupervisorError::from(SmrrError::SmrrUnsupported),
            MmSupervisorError::from(CallGateError::GdtTooSmall),
            MmSupervisorError::from(SmiHandlerIdtPatchError::EntryTooSmall),
            MmSupervisorError::from(MailboxError::CommandAlreadyPending { index: 2 }),
            MmSupervisorError::from(UnblockError::OverlapsWithMmram),
            MmSupervisorError::from(PageUpdateError::AlreadyMapped),
        ];
        for error in errors {
            assert!(error.source().is_some(), "{error} should expose its subsystem error as a source");
        }
    }

    #[test]
    fn test_mm_supervisor_error_separates_policy_from_core_bring_up() {
        // Both are initialization failures, but they stay distinguishable in the aggregate.
        let policy = MmSupervisorError::from(PolicyGateError::MalformedPolicy);
        let core = CoreInitError::UserEntryPointMissing.into();
        assert_ne!(policy, core);

        let MmSupervisorError::CoreInit(inner) = core else {
            panic!("expected a core bring-up error");
        };
        assert_eq!(inner, CoreInitError::UserEntryPointMissing);

        assert_ne!(
            MmSupervisorError::from(CoreInitError::CpuIndexOutOfRange { index: 4, len: 2 }),
            MmSupervisorError::from(CoreInitError::CpuIndexOutOfRange { index: 5, len: 2 })
        );
    }
}
