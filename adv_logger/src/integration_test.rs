//! Integration tests for Advanced Logger.
//!
//! These tests are intended to be run on the target hardware. They test the
//! Advanced Logger component and the Advanced Logger protocol are functioning
//! correctly and the the log messages are present in the memory log.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use r_efi::efi;
use uefi_sdk::boot_services::{BootServices, StandardBootServices};
use uefi_test::{u_assert, u_assert_eq, uefi_test};

use crate::{memory_log, protocol};

#[uefi_test]
fn adv_logger_test(bs: StandardBootServices) -> uefi_test::Result {
    const DIRECT_STR: &str = "adv_logger_test: Direct log message!!!";
    const PROTOCOL_STR: &str = "adv_logger_test: Logged through the protocol!!!\n";

    // Get a reference to the advanced logger buffer. The actual transport does
    // not matter so use the NULL implementation as a stand-in.
    let result = unsafe { bs.locate_protocol(&protocol::AdvancedLoggerProtocolRegister, None) };

    u_assert!(result.is_ok(), "adv_logger_test: Failed to locate the advanced logger protocol.");
    let protocol = result.unwrap();

    // Test that directly logging makes it to the memory buffer. Make sure this
    // message gets though by adjusting the max logging temporarily.
    let old_max = log::max_level();
    log::set_max_level(log::LevelFilter::Info);
    log::info!("{}", &DIRECT_STR);
    log::set_max_level(old_max);

    // Log using the protocol.
    let efi_status = (protocol.write_log)(
        protocol,
        memory_log::DEBUG_LEVEL_INFO as usize,
        PROTOCOL_STR.as_bytes().as_ptr(),
        PROTOCOL_STR.as_bytes().len(),
    );

    u_assert_eq!(efi_status, efi::Status::SUCCESS, "adv_logger_test: Failed to write to the advanced logger protocol.");

    // Check that the strings were added to the log.
    let log_info = unsafe { memory_log::AdvLoggerInfo::adopt_memory_log(protocol.log_info) };
    u_assert!(log_info.is_some(), "adv_logger_test: Failed to adopt the memory log.");
    let log_info = log_info.unwrap();
    let mut direct_found = false;
    let mut protocol_found = false;
    for entry in log_info.iter() {
        let log_str = core::str::from_utf8(entry.get_message());
        u_assert!(log_str.is_ok(), "adv_logger_test: Failed to convert log entry to string.");
        let log_str = log_str.unwrap();

        if log_str.contains(DIRECT_STR) {
            direct_found = true;
            u_assert!(
                entry.level == memory_log::DEBUG_LEVEL_INFO,
                "adv_logger_test: Direct log message has incorrect level."
            );
        } else if log_str.contains(PROTOCOL_STR) {
            protocol_found = true;
            u_assert!(direct_found, "adv_logger_test: Protocol log message found before direct log message.");
            u_assert!(
                entry.level == memory_log::DEBUG_LEVEL_INFO,
                "adv_logger_test: Direct log message has incorrect level."
            );
        }
    }

    u_assert!(direct_found, "adv_logger_test: Direct log message not found in the memory log.");
    u_assert!(protocol_found, "adv_logger_test: Protocol log message not found in the memory log.");

    Ok(())
}
