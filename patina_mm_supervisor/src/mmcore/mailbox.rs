//! Mailbox Module
//!
//! Per-AP mailboxes the BSP uses to send commands to the APs and collect their responses.
//! The BSP posts a command, the AP polls its mailbox and takes it, and the AP posts a response
//! that the BSP reads.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

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
    /// The AP's mailbox still holds a command it has not taken yet.
    ///
    /// Distinct from [`MailboxError::NoMailboxForCpuIndex`]: the slot exists, but overwriting it
    /// would drop a command the AP was still going to run.
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
                write!(f, "the mailbox for CPU index {index} still holds a command the AP has not taken")
            }
        }
    }
}

/// A procedure the BSP asks an AP to run through its mailbox.
///
/// The AP demotes to Ring 3 if the procedure lives in user-owned memory, and calls it directly
/// in Ring 0 otherwise. An AP with no pending command keeps polling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApCommand {
    /// The procedure function pointer.
    pub procedure: u64,
    /// The argument to pass to the procedure.
    pub argument: u64,
}

/// Responses from APs to the BSP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApResponse {
    /// No response yet (mailbox empty).
    None,
    /// Command executed successfully.
    Success,
    /// Command failed with an error code.
    Error(u32),
    /// AP is busy and cannot accept commands.
    Busy,
}

impl From<ApResponse> for u64 {
    /// Converts the response to a u64 for atomic storage.
    fn from(response: ApResponse) -> Self {
        match response {
            ApResponse::None => 0,
            ApResponse::Success => 1,
            ApResponse::Error(code) => 2 | (u64::from(code) << 32),
            ApResponse::Busy => 3,
        }
    }
}

impl From<u64> for ApResponse {
    /// Converts a u64 back to a response.
    fn from(value: u64) -> Self {
        let resp_type = value & 0xFF;
        match resp_type {
            1 => ApResponse::Success,
            2 => {
                let code = (value >> 32) as u32;
                ApResponse::Error(code)
            }
            3 => ApResponse::Busy,
            0 | 4..=u64::MAX => ApResponse::None,
        }
    }
}

/// The state of an AP mailbox, in the order a command moves through it.
///
/// Each state has one owner, and only that side may change the mailbox.
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

/// A single AP's mailbox for communication with the BSP.
#[repr(align(64))] // Cache-line aligned to avoid false sharing
pub struct ApMailbox {
    /// Current state of the mailbox.
    state: AtomicU32,
    /// The procedure function pointer of the pending [`ApCommand`].
    procedure: AtomicU64,
    /// The argument of the pending [`ApCommand`].
    argument: AtomicU64,
    /// The response data (packed into u64).
    response: AtomicU64,
}

impl ApMailbox {
    /// Creates a new empty mailbox.
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(MailboxState::Empty as u32),
            procedure: AtomicU64::new(0),
            argument: AtomicU64::new(0),
            response: AtomicU64::new(0),
        }
    }

    /// Takes the ready command and moves the mailbox to `CommandRunning`. Called by the AP.
    ///
    /// Returns `None` if no command is ready.
    pub fn take_command(&self) -> Option<ApCommand> {
        // Try to transition from CommandReady to CommandRunning
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

    /// Posts a response (called by AP).
    pub fn post_response(&self, response: ApResponse) {
        self.response.store(response.into(), Ordering::Release);
        self.state.store(MailboxState::ResponseReady as u32, Ordering::Release);
    }

    /// Sends a command to this mailbox (called by BSP).
    ///
    /// Returns `true` if the command was successfully posted, `false` if the mailbox is busy.
    ///
    /// The sender first claims the mailbox by moving it to [`MailboxState::CommandWriting`], then writes
    /// the payload with `Relaxed` ordering, then publishes [`MailboxState::CommandReady`] with
    /// `Release`. The AP acquires `state`, which guarantees it sees the fully-written payload.
    pub fn send_command(&self, command: ApCommand) -> bool {
        // Claim the mailbox. The state has to change here: a compare-exchange that leaves it
        // `Empty` is a check rather than a claim, so concurrent senders would all pass it and
        // interleave their payload writes, leaving the AP a command assembled from both.
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

        // Publish. The AP reads `state` with Acquire, so it sees the payload written above.
        self.state.store(MailboxState::CommandReady as u32, Ordering::Release);
        true
    }

    /// Takes the response and empties the mailbox, if a response is ready. Called by the BSP.
    pub fn get_response(&self) -> Option<ApResponse> {
        // Only read if response is ready
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

    /// Forcibly resets the mailbox to the [`MailboxState::Empty`] state,
    /// discarding any pending command, in-flight processing, or unread response.
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

/// Holds one mailbox per CPU index, sized from the core count the MP Information HOB reports.
///
/// The storage is published once by [`MailboxManager::init`] during BSP initialization. Before
/// that the manager has no mailboxes, so every lookup finds nothing.
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
    /// Returns [`MailboxError::AlreadyInitialized`] if mailboxes were already published. The new
    /// ones are not used.
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

    /// Returns the published mailboxes, or none before [`MailboxManager::init`].
    fn mailboxes(&self) -> &'static [ApMailbox] {
        self.mailboxes.get().copied().unwrap_or(&[])
    }

    /// Sends a command to a specific AP.
    ///
    /// Reports [`MailboxError::NotInitialized`] before the storage is published,
    /// [`MailboxError::NoMailboxForCpuIndex`] when `cpu_index` has no slot, and
    /// [`MailboxError::CommandAlreadyPending`] when the AP has not taken its previous command.
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
            log::error!("CPU index {cpu_index} already has a command pending");
            Err(MailboxError::CommandAlreadyPending { index: cpu_index }.into())
        }
    }

    /// Checks for a pending command (called by AP).
    pub fn check_mailbox(&self, cpu_index: usize) -> Option<ApCommand> {
        self.mailboxes().get(cpu_index)?.take_command()
    }

    /// Posts a response (called by AP).
    pub fn post_response(&self, cpu_index: usize, response: ApResponse) {
        if let Some(mailbox) = self.mailboxes().get(cpu_index) {
            mailbox.post_response(response);
        }
    }

    /// Resets every assigned mailbox back to the empty state.
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
