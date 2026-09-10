//! Default Font Components
//!
//! This module provides the [`default_font::DefaultFontProvider`] component, which registers the
//! default narrow-glyph "simple font" package with the HII database protocol once it is
//! installed.
//!
//! ```rust,ignore
//! use patina_default_font::component::default_font::DefaultFontProvider;
//!
//! commands.add_component(DefaultFontProvider::new());
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
pub mod default_font;
