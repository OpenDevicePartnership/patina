//! DXE Core Dispatch Service
//!
//! Provides the [`CoreDxeDispatch`] service implementation, which exposes
//! the PI dispatcher to components via dependency injection. This allows
//! components to trigger DXE driver dispatch passes (e.g., to interleave
//! controller connection with driver dispatch during boot).
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina::{
    component::service::{IntoService, dxe_dispatch::DxeDispatch},
    error::Result,
};

use crate::{Core, PlatformInfo};

/// DXE dispatch service backed by the PI dispatcher.
#[derive(IntoService)]
#[service(dyn DxeDispatch)]
pub(crate) struct CoreDxeDispatch<P: PlatformInfo>(&'static Core<P>);

#[coverage(off)]
impl<P: PlatformInfo> CoreDxeDispatch<P> {
    pub(crate) fn new(core: &'static Core<P>) -> Self {
        Self(core)
    }
}

#[coverage(off)]
impl<P: PlatformInfo> DxeDispatch for CoreDxeDispatch<P> {
    fn dispatch(&self) -> Result<bool> {
        self.0.pi_dispatcher.dispatch()
    }
}
