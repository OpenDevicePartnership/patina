//! UEFI specification definitions and interfaces.
//!
//! Groups the modules that implement or wrap UEFI specification concepts:
//! boot and runtime services, protocols, device paths, decompression, and
//! related base types.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

#[cfg(any(test, feature = "alloc"))]
pub mod boot_services;
pub mod decompress;
#[cfg(any(test, feature = "alloc"))]
pub mod device_path;
#[cfg(any(test, feature = "alloc"))]
pub mod driver_binding;
pub mod event;
pub mod memory;
#[cfg(any(test, feature = "alloc"))]
pub mod memory_map;
#[cfg(any(test, feature = "alloc"))]
pub mod runtime_services;
#[cfg(any(test, feature = "alloc"))]
pub mod tpl_mutex;
