//! Mailbox Module
//!
//! This module provides the mailbox infrastructure for BSP-AP communication.
//! Each AP has a dedicated mailbox that the BSP uses to send commands and receive responses.
//!
//! ## Architecture
//!
//! The mailbox system uses a simple producer-consumer model:
//! - BSP writes commands to AP mailboxes
//! - APs poll their mailboxes for commands
//! - APs write responses back
//! - BSP reads responses when ready
//!
//! ## Memory Model
//!
//! This module does not perform heap allocation. All structures use fixed-size arrays
//! with compile-time constants provided via const generics.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::{CpuInfo, perf_timer};

/// Commands that can be sent from BSP to APs via the mailbox.
///
/// APs sit in a holding pen polling for commands. When no command is pending
/// the AP simply keeps spinning - there is no explicit "no-op" variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApCommand {
    /// Run a procedure on the AP, with potential demotion to user mode.
    ///
    /// The AP checks buffer ownership and demotes to Ring 3 if the procedure
    /// lives in user-owned memory, otherwise calls it directly in Ring 0.
    RunProcedure {
        /// The procedure function pointer.
        procedure: u64,
        /// The argument to pass to the procedure.
        argument: u64,
    },
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
            ApResponse::Error(code) => 2 | ((code as u64) << 32),
            ApResponse::Busy => 3,
        }
    }
}

impl From<u64> for ApResponse {
    /// Converts a u64 back to a response.
    fn from(value: u64) -> Self {
        let resp_type = value & 0xFF;
        match resp_type {
            0 => ApResponse::None,
            1 => ApResponse::Success,
            2 => {
                let code = (value >> 32) as u32;
                ApResponse::Error(code)
            }
            3 => ApResponse::Busy,
            _ => ApResponse::None,
        }
    }
}

/// Mailbox state flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum MailboxState {
    /// Mailbox is empty, no pending command.
    Empty = 0,
    /// Mailbox has a command waiting to be processed.
    CommandPending = 1,
    /// Command is being processed.
    Processing = 2,
    /// Response is ready for BSP to read.
    ResponseReady = 3,
}

/// A single AP's mailbox for communication with the BSP.
#[repr(align(64))] // Cache-line aligned to avoid false sharing
pub struct ApMailbox {
    /// Current state of the mailbox.
    state: AtomicU32,
    /// The procedure function pointer (for RunProcedure).
    procedure: AtomicU64,
    /// The argument to pass to the procedure (for RunProcedure).
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

    /// Gets the pending command (called by AP).
    ///
    /// Returns `Some(command)` if a command is pending, `None` otherwise.
    /// This also transitions the mailbox to the Processing state.
    pub fn take_command(&self) -> Option<ApCommand> {
        // Try to transition from CommandPending to Processing
        let result = self.state.compare_exchange(
            MailboxState::CommandPending as u32,
            MailboxState::Processing as u32,
            Ordering::AcqRel,
            Ordering::Acquire,
        );

        if result.is_ok() {
            let procedure = self.procedure.load(Ordering::Acquire);
            let argument = self.argument.load(Ordering::Acquire);
            Some(ApCommand::RunProcedure { procedure, argument })
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
    /// The payload (procedure and argument) is written first with `Relaxed`
    /// ordering, then `state` is set to `CommandPending` with `Release` ordering.
    /// The AP acquires `state`, which guarantees it sees the fully-written payload.
    pub fn send_command(&self, command: ApCommand) -> bool {
        // Only allow sending if mailbox is empty
        let result = self.state.compare_exchange(
            MailboxState::Empty as u32,
            MailboxState::Empty as u32, // keep Empty while we fill the payload
            Ordering::AcqRel,
            Ordering::Acquire,
        );

        if result.is_ok() {
            // Write all payload fields before publishing.
            // Relaxed is fine here — the Release store to `state` below
            // will fence all prior writes.
            let ApCommand::RunProcedure { procedure, argument } = command;
            self.procedure.store(procedure, Ordering::Relaxed);
            self.argument.store(argument, Ordering::Relaxed);

            // Publish: the AP polls on `state` with Acquire, so this
            // Release ensures it sees the payload written above.
            self.state.store(MailboxState::CommandPending as u32, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Gets the response from this mailbox (called by BSP).
    ///
    /// Returns the response and clears the mailbox if a response is ready.
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
    ///
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

/// Manager for all AP mailboxes.
///
/// Uses fixed-size arrays with const generic for maximum AP count.
///
/// ## Const Generic Parameters
///
/// * `MAX_APS` - The maximum number of APs that can be managed.
pub struct MailboxManager<const MAX_APS: usize, C: CpuInfo> {
    /// Mailboxes - fixed size array.
    mailboxes: [ApMailbox; MAX_APS],
    /// Phantom data for the CpuInfo type.
    _cpu_info: core::marker::PhantomData<fn() -> C>,
}

impl<const MAX_APS: usize, C: CpuInfo> MailboxManager<MAX_APS, C> {
    /// Creates a new mailbox manager.
    ///
    /// This is a const fn and performs no heap allocation.
    pub const fn new() -> Self {
        Self { mailboxes: [const { ApMailbox::new() }; MAX_APS], _cpu_info: core::marker::PhantomData }
    }

    /// Sends a command to a specific AP.
    pub fn send_command(&self, cpu_index: usize, command: ApCommand) -> Result<(), ()> {
        let mailbox = self.mailboxes.get(cpu_index).ok_or(())?;
        if mailbox.send_command(command) { Ok(()) } else { Err(()) }
    }

    /// Checks for a pending command (called by AP).
    pub fn check_mailbox(&self, cpu_index: usize) -> Option<ApCommand> {
        self.mailboxes.get(cpu_index)?.take_command()
    }

    /// Posts a response (called by AP).
    pub fn post_response(&self, cpu_index: usize, response: ApResponse) {
        if let Some(mailbox) = self.mailboxes.get(cpu_index) {
            mailbox.post_response(response);
        }
    }

    /// Resets every assigned mailbox back to the empty state.
    ///
    pub fn reset_all(&self) {
        for mailbox in &self.mailboxes {
            mailbox.reset();
        }
    }

    /// Waits for a response from an AP with timeout.
    ///
    /// Returns the response, or `None` if timeout.
    pub fn wait_response(&self, cpu_index: usize, timeout_us: u64) -> Option<ApResponse> {
        let mailbox = self.mailboxes.get(cpu_index)?;
        let mut result = None;

        perf_timer::spin_until::<C, _>(timeout_us, || {
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

impl<const MAX_APS: usize, C: CpuInfo> Default for MailboxManager<MAX_APS, C> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal `CpuInfo` implementation for tests (all methods use defaults).
    struct TestCpuInfo;
    impl CpuInfo for TestCpuInfo {}

    #[test]
    fn test_mailbox_creation() {
        let mailbox = ApMailbox::new();
        // A fresh mailbox has nothing to take or read back.
        assert_eq!(mailbox.take_command(), None);
        assert_eq!(mailbox.get_response(), None);
    }

    #[test]
    fn test_mailbox_is_const() {
        // Verify we can create a static mailbox
        static _MAILBOX: ApMailbox = ApMailbox::new();
    }

    #[test]
    fn test_command_send_receive() {
        let mailbox = ApMailbox::new();
        let cmd = ApCommand::RunProcedure { procedure: 0x1234, argument: 0x5678 };

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
    fn test_run_procedure_command() {
        let mailbox = ApMailbox::new();

        let cmd = ApCommand::RunProcedure { procedure: 0xDEAD_BEEF, argument: 0x12345678 };

        assert!(mailbox.send_command(cmd));
        let received = mailbox.take_command();
        assert!(matches!(received, Some(ApCommand::RunProcedure { procedure: 0xDEAD_BEEF, argument: 0x12345678 })));
    }

    #[test]
    fn test_mailbox_manager_is_const() {
        // Verify we can create a static manager
        static _MANAGER: MailboxManager<8, TestCpuInfo> = MailboxManager::new();
    }

    #[test]
    fn test_mailbox_manager() {
        let manager: MailboxManager<4, TestCpuInfo> = MailboxManager::new();

        // Send a command to the AP occupying slot index 1.
        let command = ApCommand::RunProcedure { procedure: 0x1000, argument: 0x2000 };
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
        assert!(mailbox.send_command(ApCommand::RunProcedure { procedure: 0x1000, argument: 7 }));
        let _ = mailbox.take_command();

        // A fresh command cannot be sent while stuck.
        assert!(!mailbox.send_command(ApCommand::RunProcedure { procedure: 0x2000, argument: 8 }));

        // Reset forces the mailbox back to empty, so a new command now succeeds.
        mailbox.reset();
        assert!(mailbox.send_command(ApCommand::RunProcedure { procedure: 0x2000, argument: 8 }));
    }

    #[test]
    fn test_manager_reset_all_scrubs_every_mailbox() {
        let manager: MailboxManager<4, TestCpuInfo> = MailboxManager::new();

        // Leave two different slots' mailboxes stuck in non-empty states.
        assert!(manager.send_command(1, ApCommand::RunProcedure { procedure: 0x10, argument: 0 }).is_ok());
        let _ = manager.check_mailbox(1); // -> Processing
        assert!(manager.send_command(2, ApCommand::RunProcedure { procedure: 0x20, argument: 0 }).is_ok()); // -> CommandPending

        // Neither can accept a new command while stuck.
        assert!(manager.send_command(1, ApCommand::RunProcedure { procedure: 0x11, argument: 0 }).is_err());
        assert!(manager.send_command(2, ApCommand::RunProcedure { procedure: 0x21, argument: 0 }).is_err());

        // The BSP's leave-time sweep clears every mailbox at once.
        manager.reset_all();

        // Both slots are dispatchable again.
        assert!(manager.send_command(1, ApCommand::RunProcedure { procedure: 0x11, argument: 0 }).is_ok());
        assert!(manager.send_command(2, ApCommand::RunProcedure { procedure: 0x21, argument: 0 }).is_ok());
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
