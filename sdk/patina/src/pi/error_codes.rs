//! Error Code related definitions in PI.
//!
//! These error codes are defined in UEFI Platform Initialization Specification 1.9,
//! Volume 2: Driver Execution Environment Core Interface, Appendix A: Error Codes.
//!
//! See <https://uefi.org/specs/PI/1.9/V2_DXE_Appendix_A.html>.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// Base value for DXE error codes.
pub const DXE_ERROR: usize = 0xA0 << ((core::mem::size_of::<usize>() * 8) - 8);
/// If this value is returned by an EFI image, then the image should be unloaded.
pub const EFI_REQUEST_UNLOAD_IMAGE: usize = DXE_ERROR | 0x01;
/// If this value is returned by an API, it means the capability is not yet installed/available/ready to use.
pub const EFI_NOT_AVAILABLE_YET: usize = DXE_ERROR | 0x02;
