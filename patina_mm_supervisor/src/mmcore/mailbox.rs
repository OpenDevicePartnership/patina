//! Mailbox Module
//!
//! Per-AP mailboxes the BSP uses to send a command to an AP and collect its response.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use patina::standard::efi;
use spin::Once;

use crate::error::MmSupervisorResult;

use super::perf_timer;

/// Errors produced by the mailbox manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxError {
    /// The mailbox storage has not been published yet.
    NotInitialized,
    /// The mailbox storage was already published.
    AlreadyInitialized,
    /// The CPU index has no mailbox slot.
    NoMailboxForCpuIndex {
        /// The index the caller asked for.
        index: usize,
        /// Number of mailbox slots the manager holds.
        len: usize,
    },
    /// The AP's mailbox is not empty: a command is being written, waiting, or running, or its
    /// response has not been read.
    CommandAlreadyPending {
        /// The index whose mailbox is occupied.
        index: usize,
    },
}

impl core::error::Error for MailboxError {}

impl core::fmt::Display for MailboxError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotInitialized => write!(f, "the mailbox storage has not been published yet"),
            Self::AlreadyInitialized => write!(f, "the mailbox storage was already published"),
            Self::NoMailboxForCpuIndex { index, len } => {
                write!(f, "CPU index {index} has no mailbox slot among the {len} available")
            }
            Self::CommandAlreadyPending { index } => {
                write!(f, "the mailbox for CPU index {index} is busy with an earlier command or its unread response")
            }
        }
    }
}

/// A procedure the BSP asks an AP to run through its mailbox.
///
/// The AP demotes to Ring 3 if the procedure lives in user-owned memory, and calls it directly
/// in Ring 0 otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApCommand {
    /// Address of the procedure.
    pub procedure: u64,
    /// The argument to pass to the procedure.
    pub argument: u64,
}

/// Responses from APs to the BSP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApResponse {
    /// No response yet.
    None,
    /// Command executed successfully.
    Success,
    /// Command failed with an error status.
    Error(efi::Status),
    /// AP is busy and cannot accept commands.
    Busy,
}

/// The error bit of an `efi::Status`, its highest bit.
const EFI_ERROR_BIT: usize = 1 << (usize::BITS - 1);

impl From<ApResponse> for u64 {
    /// Packs the response for atomic storage: the kind in the low byte and, for an error, the
    /// low 32 bits of its status in the high half. The error bit is restored on unpacking.
    fn from(response: ApResponse) -> Self {
        match response {
            ApResponse::None => 0,
            ApResponse::Success => 1,
            ApResponse::Error(status) => 2 | (u64::from(status.as_usize() as u32) << 32),
            ApResponse::Busy => 3,
        }
    }
}

impl From<u64> for ApResponse {
    /// Unpacks a response stored by `From<ApResponse> for u64`. Unknown kinds read as `None`.
    fn from(value: u64) -> Self {
        let resp_type = value & 0xFF;
        match resp_type {
            1 => ApResponse::Success,
            2 => {
                let code = (value >> 32) as usize;
                ApResponse::Error(efi::Status::from_usize(EFI_ERROR_BIT | code))
            }
            3 => ApResponse::Busy,
            0 | 4..=u64::MAX => ApResponse::None,
        }
    }
}

/// The state of an AP mailbox, in the order a command moves through it. Only the side that owns
/// the current state may change the mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum MailboxState {
    /// Nothing is pending. The BSP may claim it.
    Empty = 0,
    /// The BSP has claimed the mailbox and is writing the command.
    CommandWriting = 1,
    /// The command is written and waiting for the AP.
    CommandReady = 2,
    /// The AP has taken the command and is running it.
    CommandRunning = 3,
    /// The AP has posted its response for the BSP to read.
    ResponseReady = 4,
}

/// A single AP's mailbox.
#[repr(align(64))] // Cache-line aligned to avoid false sharing
pub struct ApMailbox {
    /// The [`MailboxState`].
    state: AtomicU32,
    /// The procedure of the pending [`ApCommand`].
    procedure: AtomicU64,
    /// The argument of the pending [`ApCommand`].
    argument: AtomicU64,
    /// The packed [`ApResponse`].
    response: AtomicU64,
}

impl ApMailbox {
    /// Creates an empty mailbox.
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(MailboxState::Empty as u32),
            procedure: AtomicU64::new(0),
            argument: AtomicU64::new(0),
            response: AtomicU64::new(0),
        }
    }

    /// Takes the ready command and moves the mailbox to `CommandRunning`, or returns `None` if no
    /// command is ready. Called by the AP.
    pub fn take_command(&self) -> Option<ApCommand> {
        let result = self.state.compare_exchange(
            MailboxState::CommandReady as u32,
            MailboxState::CommandRunning as u32,
            Ordering::AcqRel,
            Ordering::Acquire,
        );

        if result.is_ok() {
            let procedure = self.procedure.load(Ordering::Acquire);
            let argument = self.argument.load(Ordering::Acquire);
            Some(ApCommand { procedure, argument })
        } else {
            None
        }
    }

    /// Posts a response. Called by the AP.
    pub fn post_response(&self, response: ApResponse) {
        self.response.store(response.into(), Ordering::Release);
        self.state.store(MailboxState::ResponseReady as u32, Ordering::Release);
    }

    /// Posts a command, returning `false` if the mailbox is not empty. Called by the BSP.
    ///
    /// The sender claims the mailbox with [`MailboxState::CommandWriting`], writes the payload,
    /// then publishes [`MailboxState::CommandReady`] with `Release`, so an AP that acquires
    /// `state` sees the whole payload.
    pub fn send_command(&self, command: ApCommand) -> bool {
        // The claim must change the state, or concurrent senders could all pass it and
        // interleave their payload writes.
        let claimed = self.state.compare_exchange(
            MailboxState::Empty as u32,
            MailboxState::CommandWriting as u32,
            Ordering::AcqRel,
            Ordering::Acquire,
        );

        if claimed.is_err() {
            return false;
        }

        // Relaxed is enough: the Release store to `state` below orders these writes before it.
        let ApCommand { procedure, argument } = command;
        self.procedure.store(procedure, Ordering::Relaxed);
        self.argument.store(argument, Ordering::Relaxed);

        self.state.store(MailboxState::CommandReady as u32, Ordering::Release);
        true
    }

    /// Takes the response and empties the mailbox, if a response is ready. Called by the BSP.
    pub fn get_response(&self) -> Option<ApResponse> {
        let result = self.state.compare_exchange(
            MailboxState::ResponseReady as u32,
            MailboxState::Empty as u32,
            Ordering::AcqRel,
            Ordering::Acquire,
        );

        if result.is_ok() {
            let resp = self.response.load(Ordering::Acquire);
            Some(ApResponse::from(resp))
        } else {
            None
        }
    }

    /// Resets the mailbox to [`MailboxState::Empty`], discarding any command or response.
    pub fn reset(&self) {
        self.procedure.store(0, Ordering::Relaxed);
        self.argument.store(0, Ordering::Relaxed);
        self.response.store(ApResponse::None.into(), Ordering::Relaxed);
        self.state.store(MailboxState::Empty as u32, Ordering::Release);
    }
}

impl Default for ApMailbox {
    fn default() -> Self {
        Self::new()
    }
}

/// Holds one mailbox per CPU index.
///
/// [`MailboxManager::init`] publishes the storage once during BSP initialization. Before that
/// every lookup finds nothing.
pub struct MailboxManager {
    /// One mailbox per CPU index.
    mailboxes: Once<&'static [ApMailbox]>,
}

impl MailboxManager {
    /// Creates a manager with no mailbox storage.
    pub const fn new() -> Self {
        Self { mailboxes: Once::new() }
    }

    /// Publishes `mailboxes`, one per CPU index. Each must be empty, as `ApMailbox::default`
    /// creates it.
    ///
    /// ## Errors
    ///
    /// Returns [`MailboxError::AlreadyInitialized`] if mailboxes were already published.
    pub(crate) fn init(&self, mailboxes: &'static [ApMailbox]) -> MmSupervisorResult<()> {
        let mut published = false;
        self.mailboxes.call_once(|| {
            published = true;
            mailboxes
        });
        if !published {
            log::error!("Mailbox storage was already published");
            return Err(MailboxError::AlreadyInitialized.into());
        }
        Ok(())
    }

    /// Returns the published mailboxes, or an empty slice before [`MailboxManager::init`].
    fn mailboxes(&self) -> &'static [ApMailbox] {
        self.mailboxes.get().copied().unwrap_or(&[])
    }

    /// Sends a command to the AP at `cpu_index`.
    ///
    /// ## Errors
    ///
    /// Returns [`MailboxError::NotInitialized`] before the storage is published,
    /// [`MailboxError::NoMailboxForCpuIndex`] if `cpu_index` has no slot, and
    /// [`MailboxError::CommandAlreadyPending`] if the mailbox is not empty.
    pub fn send_command(&self, cpu_index: usize, command: ApCommand) -> MmSupervisorResult<()> {
        let Some(mailboxes) = self.mailboxes.get() else {
            log::error!("No mailbox storage for CPU index {cpu_index}");
            return Err(MailboxError::NotInitialized.into());
        };
        let Some(mailbox) = mailboxes.get(cpu_index) else {
            let len = mailboxes.len();
            log::error!("No mailbox for CPU index {cpu_index} ({len} slots)");
            return Err(MailboxError::NoMailboxForCpuIndex { index: cpu_index, len }.into());
        };
        if mailbox.send_command(command) {
            Ok(())
        } else {
            log::error!("CPU index {cpu_index} mailbox is busy with an earlier command or its response");
            Err(MailboxError::CommandAlreadyPending { index: cpu_index }.into())
        }
    }

    /// Takes the pending command for `cpu_index`, if any. Called by the AP.
    pub fn check_mailbox(&self, cpu_index: usize) -> Option<ApCommand> {
        self.mailboxes().get(cpu_index)?.take_command()
    }

    /// Posts a response for `cpu_index`. Called by the AP.
    pub fn post_response(&self, cpu_index: usize, response: ApResponse) {
        if let Some(mailbox) = self.mailboxes().get(cpu_index) {
            mailbox.post_response(response);
        }
    }

    /// Resets every mailbox to the empty state.
    pub fn reset_all(&self) {
        for mailbox in self.mailboxes() {
            mailbox.reset();
        }
    }

    /// Waits up to `timeout_us` microseconds for a response from an AP.
    ///
    /// Returns `None` on timeout or if `cpu_index` has no mailbox.
    pub fn wait_response(&self, cpu_index: usize, timeout_us: u64) -> Option<ApResponse> {
        let mailbox = self.mailboxes().get(cpu_index)?;
        let mut result = None;

        perf_timer::spin_until(timeout_us, || {
            if let Some(response) = mailbox.get_response() {
                result = Some(response);
                true
            } else {
                false
            }
        });

        result
    }
}

impl Default for MailboxManager {
    fn default() -> Self {
        Self::new()
    }
}
