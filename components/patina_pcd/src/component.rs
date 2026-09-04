//! PCD (Platform Configuration Database) Components
//!
//! This module provides the `PcdProvider` component, which locates `PCD_PROTOCOL` and registers
//! the `PcdServices` service so other components can get and set Dynamic and `DynamicEx` PCDs.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
pub mod provider;
