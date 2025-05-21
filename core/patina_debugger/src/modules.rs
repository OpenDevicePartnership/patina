//! Implements module related structures and functions.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use alloc::{string::String, vec::Vec};

/// Information about a loaded module.
pub(crate) struct ModuleInfo {
    pub name: String,
    pub base: usize,
    pub size: usize,
}

/// Manages loaded modules and module breakpoints.
pub(crate) struct Modules {
    modules: Vec<ModuleInfo>,
    module_breakpoints: Vec<String>,
    break_all: bool,
}

impl Modules {
    pub const fn new() -> Self {
        Modules { modules: Vec::new(), module_breakpoints: Vec::new(), break_all: false }
    }

    pub fn add_module(&mut self, name: &str, base: usize, size: usize) {
        self.modules.push(ModuleInfo { name: String::from(name), base, size });
    }

    pub fn check_module_breakpoints(&self, name: &str) -> bool {
        if self.break_all {
            return true;
        }

        for module in &self.module_breakpoints {
            let trimmed = name.trim_end_matches(".efi");
            if module.eq_ignore_ascii_case(trimmed) {
                return true;
            }
        }

        false
    }

    #[cfg(feature = "alloc")]
    pub fn add_module_breakpoint(&mut self, name: &str) {
        let trimmed = name.trim().trim_end_matches(".efi");
        if !trimmed.is_empty() {
            self.module_breakpoints.push(String::from(trimmed));
        }
    }

    pub fn break_on_all(&mut self) {
        self.break_all = true;
    }

    pub fn clear_module_breakpoints(&mut self) {
        self.module_breakpoints.clear();
        self.break_all = false;
    }

    pub fn get_modules(&self) -> &Vec<ModuleInfo> {
        &self.modules
    }

    #[cfg(feature = "alloc")]
    pub fn get_module_breakpoints(&self) -> &Vec<String> {
        &self.module_breakpoints
    }
}

#[cfg(feature = "alloc")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_module() {
        let mut modules = Modules::new();
        modules.add_module("test_module", 0x1000, 0x2000);
        assert_eq!(modules.get_modules().len(), 1);
        assert_eq!(modules.get_modules()[0].name, "test_module");
        assert_eq!(modules.get_modules()[0].base, 0x1000);
        assert_eq!(modules.get_modules()[0].size, 0x2000);
    }

    #[test]
    fn test_check_module_breakpoints() {
        let mut modules = Modules::new();
        modules.add_module_breakpoint("test_module");
        assert!(modules.check_module_breakpoints("test_module"));
        assert!(!modules.check_module_breakpoints("other_module"));
    }

    #[test]
    fn test_break_on_all() {
        let mut modules = Modules::new();
        modules.break_on_all();
        assert!(modules.check_module_breakpoints("any_module"));
    }

    #[test]
    fn test_clear_module_breakpoints() {
        let mut modules = Modules::new();
        modules.add_module_breakpoint("test_module");
        modules.break_on_all();
        modules.clear_module_breakpoints();
        assert!(!modules.check_module_breakpoints("test_module"));
        assert!(!modules.check_module_breakpoints("any_module"));
    }

    #[test]
    fn test_add_module_breakpoint() {
        let mut modules = Modules::new();
        modules.add_module_breakpoint("test_module");
        assert_eq!(modules.get_module_breakpoints().len(), 1);
        assert_eq!(modules.get_module_breakpoints()[0], "test_module");
    }
}
