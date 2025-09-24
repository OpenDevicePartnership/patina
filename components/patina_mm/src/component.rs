//! Management Mode (MM) Components
//!
//! This module provides components for interacting with MM from the DXE environment. These components ultimately do
//! so through the `SwmMmiTrigger` service which is installed by the `SwMmiManager` component. The `Communicator`
//! component leverages the `SwmMmiTrigger` service to exchange messages with MM.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
pub mod communicator;
pub mod sw_mmi_manager;
