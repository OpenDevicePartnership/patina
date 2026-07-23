//! DXE Core implementation of [`TplServices`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::component::service::{
    IntoService,
    uefi_services::tpl::{PreviousTpl, Tpl, TplServices},
};

use crate::events::{raise_tpl, restore_tpl};
use crate::uefi_services::event::tpl_to_efi;
/// Core implementation of [`TplServices`], delegating to the core TPL primitives.
#[derive(IntoService)]
#[service(dyn TplServices)]
pub(crate) struct CoreTplServices;

impl TplServices for CoreTplServices {
    fn raise_tpl(&self, tpl: Tpl) -> PreviousTpl {
        PreviousTpl::from_raw(raise_tpl(tpl_to_efi(tpl)))
    }

    fn restore_tpl(&self, previous: PreviousTpl) {
        restore_tpl(previous.as_raw());
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina::standard::efi;

    #[test]
    fn test_tpl_to_efi_maps_all_variants() {
        assert_eq!(tpl_to_efi(Tpl::Application), efi::TPL_APPLICATION);
        assert_eq!(tpl_to_efi(Tpl::Callback), efi::TPL_CALLBACK);
        assert_eq!(tpl_to_efi(Tpl::Notify), efi::TPL_NOTIFY);
        assert_eq!(tpl_to_efi(Tpl::HighLevel), efi::TPL_HIGH_LEVEL);
    }

    #[test]
    fn test_core_tpl_services_raise_and_restore_round_trip() {
        crate::test_support::with_global_lock(|| {
            let service = CoreTplServices;

            let previous = service.raise_tpl(Tpl::Callback);
            assert_eq!(previous.as_raw(), efi::TPL_APPLICATION);

            service.restore_tpl(previous);
        })
        .unwrap();
    }

    #[test]
    fn test_core_tpl_services_nested_raise_restore_round_trip() {
        crate::test_support::with_global_lock(|| {
            let service = CoreTplServices;

            let previous_callback = service.raise_tpl(Tpl::Callback);
            let previous_notify = service.raise_tpl(Tpl::Notify);

            // Check that the innermost raise restores first.
            service.restore_tpl(previous_notify);
            service.restore_tpl(previous_callback);
        })
        .unwrap();
    }
}
