//! Component metadata.
//!
//! The metadata is used by the scheduler for multiple purposes including, but not limited to:
//! - Managing access requirements for components.
//! - Logging and debugging.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
use core::fmt;
use fixedbitset::FixedBitSet;

/// Metadata for a component. Not used for execution, but referenced by the scheduler.
#[derive(Default, Debug)]
pub struct MetaData {
    /// The read/write parameter access requirements for the component.
    access: Access,
    /// The name of the component.
    name: &'static str,
    /// the name of the last param that failed to be set.
    last_failed_param: Option<&'static str>,
}

impl MetaData {
    /// Creates a new metadata object for a component.
    pub fn new<S>() -> Self {
        Self { access: Access::new(), name: core::any::type_name::<S>(), last_failed_param: None }
    }

    #[inline(always)]
    pub fn name(&self) -> &'static str {
        self.name
    }

    #[inline(always)]
    pub fn set_failed_param(&mut self, param: &'static str) {
        self.last_failed_param = Some(param);
    }

    #[inline(always)]
    pub fn failed_param(&self) -> Option<&'static str> {
        self.last_failed_param
    }

    #[inline(always)]
    pub(crate) fn access_mut(&mut self) -> &mut Access {
        &mut self.access
    }

    #[inline(always)]
    pub(crate) fn access(&self) -> &Access {
        &self.access
    }
}

/// Access requirements for a component.
#[derive(Default)]
pub struct Access {
    /// Write accesses to a config resource.
    config_writes: FixedBitSet,
    /// All accesses to a config resource.
    config_read_and_writes: FixedBitSet,
    /// is `true` if the component has access to all config resources.
    reads_all_configs: bool,
    /// is `true` if the component has mutable access to all config resources.
    writes_all_configs: bool,
    /// is `true` if the component accesses the deferred queue.
    has_deferred: bool,
}

impl Access {
    pub const fn new() -> Self {
        Self {
            config_writes: FixedBitSet::new(),
            config_read_and_writes: FixedBitSet::new(),
            reads_all_configs: false,
            writes_all_configs: false,
            has_deferred: false,
        }
    }
}

impl Access {
    /// Registers a write access to the specified config resource.
    pub fn add_config_write(&mut self, id: usize) {
        self.config_writes.grow_and_insert(id);
        self.config_read_and_writes.grow_and_insert(id);
    }

    /// Registers a read access to a config resource.
    pub fn add_config_read(&mut self, id: usize) {
        self.config_read_and_writes.grow_and_insert(id);
    }

    /// Returns whether the component needs write access to the config resources denoted by `id`.
    pub fn has_config_write(&self, id: usize) -> bool {
        self.writes_all_configs | self.config_writes.contains(id)
    }

    /// Returns whether the component needs read access to the config resources denoted by `id`.
    pub fn has_config_read(&self, id: usize) -> bool {
        self.reads_all_configs | self.config_read_and_writes.contains(id)
    }

    pub fn has_any_config_write(&self) -> bool {
        self.writes_all_configs | (self.config_writes.count_ones(..) > 0)
    }

    pub fn has_any_config_read(&self) -> bool {
        self.reads_all_configs | (self.config_read_and_writes.count_ones(..) > 0)
    }

    pub fn has_writes_all_configs(&self) -> bool {
        self.writes_all_configs
    }

    pub fn has_deferred(&self) -> bool {
        self.has_deferred
    }

    pub fn reads_all_configs(&mut self) {
        self.reads_all_configs = true;
    }

    pub fn writes_all_configs(&mut self) {
        self.writes_all_configs = true;
        self.reads_all_configs = true;
    }

    pub fn deferred(&mut self) {
        self.has_deferred = true;
    }
}

impl fmt::Debug for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Access")
            .field("reads_all_configs", &self.reads_all_configs)
            .field("writes_all_configs", &self.writes_all_configs)
            .field("config_writes", &PrettyFixedBitSet(&self.config_writes))
            .field(
                "config_reads",
                &PrettyFixedBitSet(&self.config_read_and_writes.difference(&self.config_writes).collect()),
            )
            .finish()
    }
}

/// A type redefinition of [FixedBitSet] to allow a custom [Debug] implementation.
pub struct PrettyFixedBitSet<'a>(&'a FixedBitSet);

impl fmt::Debug for PrettyFixedBitSet<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.ones().map(|i| i as u32)).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn test_debug_view_calculates_config_reads_correctly() {
        let mut access = Access::new();
        access.add_config_write(0);
        access.add_config_read(1);
        access.reads_all_configs();
        access.writes_all_configs();

        assert_eq!(
            std::format!("{:?}", access),
            "Access { reads_all_configs: true, writes_all_configs: true, config_writes: [0], config_reads: [1] }"
        );
    }

    #[test]
    fn test_write_config_marks_as_read_also() {
        let mut access = Access::new();

        access.add_config_write(0);

        assert!(access.has_config_read(0));
        assert!(access.has_config_write(0));

        assert!(!access.has_config_read(1));
        assert!(!access.has_config_write(1));

        assert!(access.has_any_config_read());
        assert!(access.has_any_config_write());
    }

    #[test]
    fn test_read_config_does_not_mark_config_write() {
        let mut access = Access::new();

        access.add_config_read(0);

        assert!(access.has_any_config_read());
        assert!(access.has_config_read(0));

        assert!(!access.has_any_config_write());
        assert!(!access.has_config_write(0));

        assert!(!access.has_config_read(1));
        assert!(!access.has_config_write(1));
    }

    #[test]
    fn test_reads_all_configs_does_not_mark_config_write() {
        let mut access = Access::new();
        access.reads_all_configs();

        for i in 0..10 {
            assert!(access.has_config_read(i));
            assert!(!access.has_config_write(i));
        }

        assert!(access.has_any_config_read());
        assert!(!access.has_any_config_write());
    }

    #[test]
    fn test_writes_all_configs_marks_config_read_also() {
        let mut access = Access::new();
        access.writes_all_configs();

        for i in 0..10 {
            assert!(access.has_config_read(i));
            assert!(access.has_config_write(i));
        }

        assert!(access.has_any_config_read());
        assert!(access.has_any_config_write());
    }
}
