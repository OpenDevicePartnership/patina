//! TPL Critical Section Sample Component
//!
//! This component demonstrates [`TplServices`] for serializing access to state shared with an
//! asynchronous event notification. It raises the task priority level (TPL) in `Notify` blocks.
//!
//! The recommended form to use is
//! [`with_raised_tpl`](patina::component::service::uefi_services::tpl::TplServicesExt::with_raised_tpl),
//! which raises the TPL, runs a closure, and restores the previous level even if the closure
//! returns early. [`raise`](patina::component::service::uefi_services::tpl::TplServicesExt::raise)
//! returns a guard for cases where the critical section spans a whole block and TPL should be
//! restored when the block ends.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::sync::atomic::{AtomicU32, Ordering};

use patina::{
    component::{
        component,
        service::{
            Service,
            uefi_services::tpl::{Tpl, TplServices, TplServicesExt},
        },
    },
    error::Result,
};

/// A pair of counters that must always be kept in sync. An interrupting notification that observed
/// them mid-update would see an inconsistent pair, so updates happen inside a raised-TPL section.
static PRIMARY_COUNT: AtomicU32 = AtomicU32::new(0);
static MIRROR_COUNT: AtomicU32 = AtomicU32::new(0);

/// Updates two related counters atomically with respect to notifications by raising the TPL.
#[derive(Default)]
pub struct TplCriticalSectionSample;

#[component]
impl TplCriticalSectionSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, tpl: Service<dyn TplServices>) -> Result<()> {
        // Option 1: The closure runs at TPL_NOTIFY, and the previous level is restored
        // automatically when it returns.
        tpl.with_raised_tpl(Tpl::Notify, || {
            let next = PRIMARY_COUNT.load(Ordering::Relaxed) + 1;
            PRIMARY_COUNT.store(next, Ordering::Relaxed);
            MIRROR_COUNT.store(next, Ordering::Relaxed);
        });

        // Option 2: Equivalent guard-based form for a critical section that spans a block.
        // The TPL stays raised until `_guard` is dropped at the end of the scope.
        {
            let _guard = tpl.raise(Tpl::Notify);
            let next = PRIMARY_COUNT.load(Ordering::Relaxed) + 1;
            PRIMARY_COUNT.store(next, Ordering::Relaxed);
            MIRROR_COUNT.store(next, Ordering::Relaxed);
        } // previous TPL is restored here

        debug_assert_eq!(PRIMARY_COUNT.load(Ordering::Relaxed), MIRROR_COUNT.load(Ordering::Relaxed));
        log::info!("Counters were kept consistent under raised TPL");

        Ok(())
    }
}
