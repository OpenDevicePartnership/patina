//! ## MM Technology Background (x86 Architecture)
//!
//! System Management Mode (SMM) or Management Mode (MM) is a special-purpose operating mode in x86 architecture
//! with high execution privilege that is used to monitor and manage various system resources. MM code is often
//! written similarly to non-MM UEFI Code, built with the same toolset and included alongside non-MM UEFI code in
//! the same firmware image. However, MM code executes in a special region of memory that is isolated from the rest
//! of the system, and it is not directly accessible to the operating system or other software running on the system.
//!
//! This region is called System Management RAM (SMRAM) or Management Mode RAM (MMRAM). Since this region is isolated,
//! constructs from the DXE environment like boot services, runtime services, and the DXE protocol database are not
//! available in MM. Instead, MM code uses its own services table and protocol data entirely managed in MMRAM.
//!
//! MM is entered on a system by triggering a System Management Interrupt (SMI) also called a Management Mode
//! Interrupt (MMI). The MMI may be either triggered by software (synchronous) or a hardware (asynchronous) event. A
//! MMI is a high priority, non-maskable interrupt. On receipt of the interrupt, the processor saves the current state
//! of the system and switches to MM. Within MM, the code must set up its own execution environment such as applying
//! an interupt descriptor table (IDT), creating page tables, etc. It must also identify the source of the MMI to
//! determine what MMI handler to invoke in response.
//!
//! Recently, there has been an effort to reduce and even eliminate the use of MM in modern systems. MM represents a
//! large attack surface because of its pervasiveness throughout the system lifetime. It is especially impactful if
//! compromised due to its ubiquity and system access privilege. A vulnerability in a given MM implementation could
//! further be used to compromise or circumvent OS protections such as Virtualization-based Security (VBS). Based on
//! the current use cases for MM and available alternatives, it is not possible to completely eliminate MM from
//! modern systems.
//!
//! ## Examples and Usage
//!
//! This example demonstrates how to use the `MmCommunication` service to communicate with the
//! [MM Supervisor](https://github.com/microsoft/mu_feature_mm_supv). The MM Supervisor has a MMI handler that will
//! respond to requests for information about the MM Supervisor itself, such as its version and capabilities.
//!
//! ```rust
//! use patina_mm::service::MmCommunication;
//! use patina::component::{IntoComponent, service::Service};
//! use r_efi::efi;
//!
//! /// MM Supervisor Request Header
//! ///
//! /// Used to request information from the MM Supervisor.
//! ///
//! /// ## Notes
//! ///
//! /// - This structure is only defined here for test purposes.
//! #[repr(C, packed(1))]
//! struct MmSupervisorRequestHeader {
//!     signature: u32,
//!     revision: u32,
//!     request: u32,
//!     reserved: u32,
//!     result: u64,
//! }
//!
//! /// MM Supervisor Version Info
//! ///
//! /// Populated by the MM Supervisor in response to a version request.
//! ///
//! /// ## Notes
//! ///
//! /// - This structure is only defined here for test purposes.
//! #[repr(C, packed(1))]
//! struct MmSupervisorVersionInfo {
//!     version: u32,
//!     patch_level: u32,
//!     max_supervisor_request_level: u64,
//! }
//!
//! /// QEMU Q35 MM Test Component
//! ///
//! /// Responsible for testing the MM communication interface on the QEMU Q35 platform.
//! #[derive(Default, IntoComponent)]
//! pub struct MmSupervisorDemo;
//!
//! impl MmSupervisorDemo {
//!     pub fn new() -> Self {
//!         Self
//!     }
//!
//!     /// Entry point for the MM Test component.
//!     ///
//!     /// Uses the `MmCommunication` service to send a request version information from the MM Supervisor. The MM
//!     /// Supervisor is expected to be the Standalone MM environment used on the QEMU Q35 platform.
//!     pub fn entry_point(self, mm_comm: Service<dyn MmCommunication>) -> patina::error::Result<()> {
//!         let mm_supv_req_header = MmSupervisorRequestHeader {
//!             signature: u32::from_le_bytes([b'M', b'S', b'U', b'P']),
//!             revision: 1,
//!             request: 0x0003, // Request Version Info
//!             reserved: 0,
//!             result: 0,
//!         };
//!
//!         let result = unsafe {
//!             mm_comm
//!                 .communicate(
//!                     0,
//!                     core::slice::from_raw_parts(
//!                         &mm_supv_req_header as *const _ as *const u8,
//!                         core::mem::size_of::<MmSupervisorRequestHeader>(),
//!                     ),
//!                     efi::Guid::from_fields(
//!                         0x8c633b23,
//!                         0x1260,
//!                         0x4ea6,
//!                         0x83,
//!                         0x0F,
//!                         &[0x7d, 0xdc, 0x97, 0x38, 0x21, 0x11],
//!                     ),
//!                 )
//!                 .map_err(|_| {
//!                     log::error!("MM Communication failed");
//!                     patina::error::EfiError::DeviceError // Todo: Map actual codes
//!                 })?
//!         };
//!
//!         let mm_supv_ver_info = unsafe {
//!             &*(result[core::mem::size_of::<MmSupervisorRequestHeader>()..].as_ptr() as *const MmSupervisorVersionInfo)
//!         };
//!         let version = mm_supv_ver_info.version;
//!         let patch_level = mm_supv_ver_info.patch_level;
//!         let max_request_level = mm_supv_ver_info.max_supervisor_request_level;
//!         log::info!(
//!             "MM Supervisor Version: {:#X}, Patch Level: {:#X}, Max Request Level: {:#X}",
//!             version,
//!             patch_level,
//!             max_request_level
//!         );
//!
//!         Ok(())
//!     }
//! }
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
#![cfg_attr(all(not(feature = "std"), not(test), not(feature = "mockall")), no_std)]
#![feature(coverage_attribute)]

pub mod component;
pub mod config;
pub mod service;
