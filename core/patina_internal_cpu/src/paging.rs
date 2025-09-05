//! UEFI Paging Module
//!
//! This module provides implementation for handling paging.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "uefi", target_arch = "x86_64"))] {
        mod x64;
        pub use x64::create_cpu_x64_paging as create_cpu_paging;
    } else if #[cfg(all(target_os = "uefi", target_arch = "aarch64"))] {
        mod aarch64;
        pub use aarch64::create_cpu_aarch64_paging as create_cpu_paging;
    } else {
        mod null;
        pub use null::create_cpu_null_paging as create_cpu_paging;
    }
}
