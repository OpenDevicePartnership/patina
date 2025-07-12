//! A blanket [Component] implementation for functions whose parameters implement [Param].
//!
//! Everything in this module does not need to be used directly by the developer. It exists to provide a blanket
//! implementation for all functions whose parameters implement [Param]. This ranges from functions with no
//! parameters (such as `fn my_system()`) to functions with  multiple parameters (such as
//! `fn my_system(data: Config<i32>, data2: Config<f32>)`) and even anonymous functions (such as `fn ()`).
//!
//! Review [Param] implementations for all types that can be used as parameters to these functions.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
extern crate alloc;

use core::marker::PhantomData;

use super::{
    metadata::MetaData,
    params::{Param, ParamFunction},
    storage::{Storage, UnsafeStorageCell},
    Component, IntoComponent,
};

use crate::error::Result;

/// A [Component] implementation for a function whose parameters all implement [Param].
pub struct FunctionComponent<Marker, Func>
where
    Func: ParamFunction<Marker>,
{
    func: Func,
    param_state: Option<<Func::Param as Param>::State>,
    metadata: MetaData,
    _marker: PhantomData<fn() -> Marker>,
}

impl<Marker, Func> Component for FunctionComponent<Marker, Func>
where
    Marker: 'static,
    Func: ParamFunction<Marker, In = (), Out = Result<()>>,
{
    /// Runs the Component if all parameters are retrievable from storage.
    ///
    /// ## Safety
    ///
    /// - Each parameter must properly register its access type.
    /// - Each parameter must properly validate its access ability.
    unsafe fn run_unsafe(&mut self, storage: UnsafeStorageCell) -> Result<bool> {
        let param_state = self.param_state.as_mut().expect("Param state created on initialize.");

        if let Err(bad_param) = Func::Param::try_validate(param_state, storage) {
            self.metadata.set_failed_param(bad_param);
            return Ok(false);
        }

        let param_value = unsafe { Func::Param::get_param(param_state, storage) };

        self.func.run((), param_value).map(|_| true)
    }

    /// Returns the metadata of the Component.
    fn metadata(&self) -> &MetaData {
        &self.metadata
    }

    /// One-time initialization of the Component. Should set [Access](super::metadata::Access) requirements.
    fn initialize(&mut self, _storage: &mut Storage) {
        self.param_state = Some(Func::Param::init_state(_storage, &mut self.metadata));
    }
}

impl<Marker, F> IntoComponent<Marker> for F
where
    Marker: 'static,
    F: ParamFunction<Marker, In = (), Out = Result<()>>,
{
    fn into_component(self) -> alloc::boxed::Box<dyn Component> {
        alloc::boxed::Box::new(FunctionComponent {
            func: self,
            param_state: None,
            metadata: MetaData::new::<F>(),
            _marker: PhantomData,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::params::{Config, ConfigMut};

    #[test]
    fn test_metadata_returns_correct_metadata() {
        let func1 = |_: Config<i32>| Ok(());
        let mut component1 = func1.into_component();

        let func2 = |_: ConfigMut<i32>| Ok(());
        let mut component2 = func2.into_component();

        assert!(component1.metadata().name().ends_with("{{closure}}"));
        assert_eq!(component2.metadata().failed_param(), None);

        let mut storage = Storage::new();
        component1.initialize(&mut storage);
        component2.initialize(&mut storage);

        // The config datum is now marked as unlocked in storage, so if we run component1, it should fail because
        // `Config` is only retrieveable if the datum is locked.
        assert_eq!(unsafe { component1.run_unsafe((&storage).into()) }, Ok(false));
        assert_eq!(component1.metadata().failed_param(), Some("patina_sdk::component::params::Config<i32>"));
    }
}
