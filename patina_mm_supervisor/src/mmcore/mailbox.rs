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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::test_support::leaked_storage;

    /// A manager with `slots` published mailboxes.
    fn manager_with(slots: usize) -> MailboxManager {
        let manager = MailboxManager::new();
        manager.init(leaked_storage(slots)).expect("fresh storage is published");
        manager
    }

    #[test]
    fn test_mailbox_creation() {
        let mailbox = ApMailbox::new();
        // A fresh mailbox has nothing to take or read back.
        assert_eq!(mailbox.take_command(), None);
        assert_eq!(mailbox.get_response(), None);
    }

    #[test]
    fn test_mailbox_defaults_match_new() {
        // The manager is held in a static, so Default has to produce the same empty manager that
        // new() does: no mailbox can be reached before storage is published.
        let mailbox = ApMailbox::default();
        assert_eq!(mailbox.take_command(), None);
        assert_eq!(mailbox.get_response(), None);

        let manager = MailboxManager::default();
        assert_eq!(manager.check_mailbox(0), None);
    }

    #[test]
    fn test_mailbox_is_const() {
        // Verify we can create a static mailbox
        static _MAILBOX: ApMailbox = ApMailbox::new();
    }

    #[test]
    fn test_command_send_receive() {
        let mailbox = ApMailbox::new();
        let cmd = ApCommand { procedure: 0x1234, argument: 0x5678 };

        // Send a command; a second send is rejected while one is pending.
        assert!(mailbox.send_command(cmd));
        assert!(!mailbox.send_command(cmd));

        // Take the command, then post and read back the response.
        assert_eq!(mailbox.take_command(), Some(cmd));
        mailbox.post_response(ApResponse::Success);
        assert_eq!(mailbox.get_response(), Some(ApResponse::Success));

        // The mailbox is empty again: a fresh command can be sent.
        assert!(mailbox.send_command(cmd));
    }

    #[test]
    fn test_only_one_concurrent_sender_claims_the_mailbox() {
        use std::sync::{Arc, Barrier};

        const SENDERS: u64 = 4;

        // Each sender's payload carries the same value in both fields, so a command assembled
        // from two senders' writes is visible as a mismatched pair.
        for _ in 0..64 {
            let mailbox = Arc::new(ApMailbox::new());
            let accepted = Arc::new(AtomicU32::new(0));
            // Thread spawn alone lets each sender finish before the next starts, which hides the
            // race entirely. Hold them until every one is ready.
            let start = Arc::new(Barrier::new(SENDERS as usize));

            let senders: Vec<_> = (1..=SENDERS)
                .map(|sender_number| {
                    let mailbox = Arc::clone(&mailbox);
                    let accepted = Arc::clone(&accepted);
                    let start = Arc::clone(&start);
                    std::thread::spawn(move || {
                        let value = sender_number * 0x1111_1111;
                        start.wait();
                        if mailbox.send_command(ApCommand { procedure: value, argument: value }) {
                            accepted.fetch_add(1, Ordering::SeqCst);
                        }
                    })
                })
                .collect();
            for sender in senders {
                sender.join().expect("sender thread completes");
            }

            assert_eq!(accepted.load(Ordering::SeqCst), 1, "more than one sender claimed the mailbox");

            let ApCommand { procedure, argument } = mailbox.take_command().expect("the accepted command is pending");
            assert_eq!(procedure, argument, "payload was interleaved between senders");
        }
    }

    #[test]
    fn test_run_procedure_command() {
        let mailbox = ApMailbox::new();

        let cmd = ApCommand { procedure: 0xDEAD_BEEF, argument: 0x12345678 };

        assert!(mailbox.send_command(cmd));
        let received = mailbox.take_command();
        assert!(matches!(received, Some(ApCommand { procedure: 0xDEAD_BEEF, argument: 0x12345678 })));
    }

    #[test]
    fn test_mailbox_manager_is_const() {
        // Verify we can create a static manager
        static _MANAGER: MailboxManager = MailboxManager::new();
    }

    #[test]
    fn test_mailbox_manager_has_no_mailboxes_before_init() {
        let manager = MailboxManager::new();
        let command = ApCommand { procedure: 0x1000, argument: 0x2000 };

        assert_eq!(manager.send_command(0, command), Err(MailboxError::NotInitialized.into()));
        assert_eq!(manager.check_mailbox(0), None);
        assert_eq!(manager.wait_response(0, 0), None);
    }

    #[test]
    fn test_mailbox_manager_publishes_storage_once() {
        let manager = manager_with(2);
        let command = ApCommand { procedure: 0x1000, argument: 0x2000 };
        assert!(manager.send_command(1, command).is_ok());

        // A second publication is refused, and the first storage, with its pending command, stays.
        assert_eq!(manager.init(leaked_storage(8)), Err(MailboxError::AlreadyInitialized.into()));
        assert_eq!(manager.check_mailbox(1), Some(command));
        assert_eq!(
            manager.send_command(2, command),
            Err(MailboxError::NoMailboxForCpuIndex { index: 2, len: 2 }.into())
        );
    }

    #[test]
    fn test_mailbox_manager_rejects_an_unknown_cpu_index() {
        let manager = manager_with(4);
        let command = ApCommand { procedure: 0x1000, argument: 0x2000 };

        // Slot 4 is past the end of a four slot manager.
        assert_eq!(
            manager.send_command(4, command),
            Err(MailboxError::NoMailboxForCpuIndex { index: 4, len: 4 }.into())
        );
        assert_eq!(manager.check_mailbox(4), None);
    }

    #[test]
    fn test_mailbox_manager_rejects_a_second_pending_command() {
        let manager = manager_with(4);
        let command = ApCommand { procedure: 0x1000, argument: 0x2000 };

        assert!(manager.send_command(2, command).is_ok());
        // The slot already holds a command that the AP has not taken yet.
        assert_eq!(manager.send_command(2, command), Err(MailboxError::CommandAlreadyPending { index: 2 }.into()));

        // A busy slot and a missing slot are different failures.
        assert_ne!(
            manager.send_command(2, command),
            manager.send_command(4, command),
            "a pending command and a missing mailbox must stay distinguishable"
        );
    }

    #[test]
    fn test_mailbox_error_displays_each_variant() {
        assert_eq!(format!("{}", MailboxError::NotInitialized), "the mailbox storage has not been published yet");
        assert_eq!(format!("{}", MailboxError::AlreadyInitialized), "the mailbox storage was already published");
        assert_eq!(
            format!("{}", MailboxError::NoMailboxForCpuIndex { index: 4, len: 4 }),
            "CPU index 4 has no mailbox slot among the 4 available"
        );
        assert_eq!(
            format!("{}", MailboxError::CommandAlreadyPending { index: 2 }),
            "the mailbox for CPU index 2 still holds a command the AP has not taken"
        );
    }

    #[test]
    fn test_mailbox_manager() {
        let manager = manager_with(4);

        // Send a command to the AP occupying slot index 1.
        let command = ApCommand { procedure: 0x1000, argument: 0x2000 };
        assert!(manager.send_command(1, command).is_ok());

        // Check mailbox
        let cmd = manager.check_mailbox(1);
        assert_eq!(cmd, Some(command));

        // Post response
        manager.post_response(1, ApResponse::Success);

        // Wait for response
        let resp = manager.wait_response(1, 1000);
        assert_eq!(resp, Some(ApResponse::Success));
    }

    #[test]
    fn test_mailbox_reset_forces_empty() {
        let mailbox = ApMailbox::new();

        // Put the mailbox into a stuck, non-empty state: a command was sent and
        // picked up for processing, but no response was ever posted (hung AP).
        assert!(mailbox.send_command(ApCommand { procedure: 0x1000, argument: 7 }));
        let _ = mailbox.take_command();

        // A fresh command cannot be sent while stuck.
        assert!(!mailbox.send_command(ApCommand { procedure: 0x2000, argument: 8 }));

        // Reset forces the mailbox back to empty, so a new command succeeds.
        mailbox.reset();
        assert!(mailbox.send_command(ApCommand { procedure: 0x2000, argument: 8 }));
    }

    #[test]
    fn test_manager_reset_all_scrubs_every_mailbox() {
        let manager = manager_with(4);

        // Leave two different slots' mailboxes stuck in non-empty states.
        assert!(manager.send_command(1, ApCommand { procedure: 0x10, argument: 0 }).is_ok());
        let _ = manager.check_mailbox(1); // -> CommandRunning
        assert!(manager.send_command(2, ApCommand { procedure: 0x20, argument: 0 }).is_ok()); // -> CommandReady

        // Neither can accept a new command while stuck.
        assert!(manager.send_command(1, ApCommand { procedure: 0x11, argument: 0 }).is_err());
        assert!(manager.send_command(2, ApCommand { procedure: 0x21, argument: 0 }).is_err());

        // The BSP's leave-time sweep clears every mailbox at once.
        manager.reset_all();

        // Both slots are dispatchable again.
        assert!(manager.send_command(1, ApCommand { procedure: 0x11, argument: 0 }).is_ok());
        assert!(manager.send_command(2, ApCommand { procedure: 0x21, argument: 0 }).is_ok());
    }

    #[test]
    fn test_response_encoding() {
        let responses = [ApResponse::None, ApResponse::Success, ApResponse::Error(42), ApResponse::Busy];

        for resp in responses {
            let encoded = u64::from(resp);
            let decoded = ApResponse::from(encoded);
            assert_eq!(resp, decoded);
        }
    }
}
