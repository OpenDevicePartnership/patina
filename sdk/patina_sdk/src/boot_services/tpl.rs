//! This module defined every struct related to Tpl in boot services.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use r_efi::efi;

use super::BootServices;

/// This is a structure restore the [`Tpl`] at the end of its scope or when dropped.
///
/// See [`BootServices::raise_tpl_guarded`] for more details.
#[must_use = "if unused the Tpl will immediately restored"]
pub struct TplGuard<'a, T: BootServices + ?Sized> {
    pub(crate) boot_services: &'a T,
    pub(crate) retore_tpl: Tpl,
}

impl<T: BootServices + ?Sized> Drop for TplGuard<'_, T> {
    fn drop(&mut self) {
        self.boot_services.restore_tpl(self.retore_tpl);
    }
}

/// Task Priority Level
#[derive(Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Tpl(pub usize);

impl Tpl {
    /// This is the lowest priority level.
    /// It is the level of execution which occurs when no event notifications are pending and which interacts with the user.
    /// User I/O (and blocking on User I/O) can be performed at this level.
    /// The boot manager executes at this level and passes control to other UEFI applications at this level.
    pub const APPLICATION: Tpl = Tpl(efi::TPL_APPLICATION);

    /// Interrupts code executing below TPL_CALLBACK level.
    /// Long term operations (such as file system operations and disk I/O) can occur at this level.
    pub const CALLBACK: Tpl = Tpl(efi::TPL_CALLBACK);

    /// Interrupts code executing below TPL_NOTIFY level.
    /// Blocking is not allowed at this level.
    /// Code executes to completion and returns.
    /// If code requires more processing, it needs to signal an event to wait to obtain control again at whatever level it requires.
    /// This level is typically used to process low level IO to or from a device.
    pub const NOTIFY: Tpl = Tpl(efi::TPL_NOTIFY);
}

impl From<Tpl> for usize {
    fn from(val: Tpl) -> Self {
        val.0
    }
}

impl From<usize> for Tpl {
    fn from(val: usize) -> Self {
        Tpl(val)
    }
}
