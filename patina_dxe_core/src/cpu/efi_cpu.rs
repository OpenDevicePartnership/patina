//! DXE Core CPU initialization types.
//!
//! This module provides the [EfiCpu] type used for boot-time CPU initialization. The [EfiCpu]
//! struct is architecture specific and selected at compile time based on the target architecture.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#[cfg(target_arch = "aarch64")]
pub(crate) mod aarch64;
#[cfg(not(target_os = "uefi"))]
mod stub;
#[cfg(target_arch = "x86_64")]
pub(crate) mod x64;

cfg_if::cfg_if! {
    if #[cfg(not(target_os = "uefi"))] {
        /// A stand in implementation of the CPU struct. This will be architecture structure defined by the platform
        /// compilation.
        pub type EfiCpu = stub::EfiCpuStub;
    } else if #[cfg(target_arch = "x86_64")] {
        pub type EfiCpu = x64::EfiCpuX64;
    } else if #[cfg(target_arch = "aarch64")] {
        pub type EfiCpu = aarch64::EfiCpuAarch64;
    }
}
