//! Aggregate MM Supervisor error type.
//!
//! This module provides [`MmSupervisorError`], a single enum that groups the
//! error types declared across the supervisor's subsystems. Each variant names
//! a subsystem and carries that subsystem's existing error enum unchanged, so
//! no detail is lost when an error is widened into the aggregate.
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
    privilege_mgmt::save_state::SaveStateValidationError,
    privilege_mgmt::{call_gate::CallGateError, syscall_setup::SyscallSetupError},
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
