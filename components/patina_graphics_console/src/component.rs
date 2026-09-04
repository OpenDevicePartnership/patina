//! Graphics console components.
//!
//! - [`graphics_console::GraphicsConsoleProvider`] installs an EFI Driver Binding instance that
//!   binds to every Graphics Output Protocol (GOP) controller in the system. Each controller it
//!   starts gets its own EFI Simple Text Output protocol instance that renders text onto that
//!   controller's framebuffer, using EFI HII Font protocol for glyph rasterization.
//!
//! A platform registers it like any other component:
//!
//! ```ignore
//! commands.add_component(GraphicsConsoleProvider::new());
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

pub mod graphics_console;
