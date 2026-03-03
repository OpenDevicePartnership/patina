//! UEFI HID - Human Interface Device support as a Patina component.
//!
//! This crate provides a Patina component that consumes the HidIo protocol and
//! produces UEFI input protocols (SimpleTextInput, SimpleTextInputEx,
//! AbsolutePointer) for keyboard and pointer HID devices.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod hid;
pub mod hid_io;
pub mod keyboard;
pub mod pointer;
