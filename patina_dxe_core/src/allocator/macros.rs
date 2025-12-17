//! Macros for interacting with allocators.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

/// Macro to iterate over all static allocators and execute an expression for each.
/// Returns `true` if any allocator returns `true` from the expression.
/// The variable `$alloc` is available in the expression and represents each allocator.
#[macro_export]
macro_rules! for_each_static_allocator {
    ($alloc:ident => $action:expr) => {{
        $crate::allocator::STATIC_ALLOCATORS.iter().any(|(alloc_ref, _)| {
            let $alloc = *alloc_ref;
            $action
        })
    }};
}

/// Macro to try an operation on each static allocator and return the first success.
/// Sets a mutable variable to the memory type if successful.
/// The variable `$alloc` is available in the expression and represents each allocator.
///
/// # Example
/// ```ignore
/// let mut memory_type = efi::BOOT_SERVICES_DATA;
/// if try_each_static_allocator!(memory_type, alloc => alloc.allocate_pages(pages)) {
///     // memory_type now contains the type of the allocator that succeeded
/// }
/// ```
#[macro_export]
macro_rules! try_each_static_allocator {
    ($memory_type_var:ident, $alloc:ident => $action:expr) => {{
        let mut found = false;
        for (alloc_ref, mem_type) in $crate::allocator::STATIC_ALLOCATORS.iter() {
            let $alloc = *alloc_ref;
            if $action.is_ok() {
                $memory_type_var = *mem_type;
                found = true;
                break;
            }
        }
        found
    }};
}

/// Macro to match a memory type and execute an action on the corresponding static allocator.
/// Falls back to a default expression if the memory type doesn't match any static allocator.
///
/// # Example
/// ```ignore
/// match_static_allocator!(memory_type, alloc => alloc.get_memory_ranges().collect(), {
///     // Fallback for non-static allocators
///     Vec::new()
/// })
/// ```
#[macro_export]
macro_rules! match_static_allocator {
    ($memory_type:expr, $alloc:ident => $action:expr, $fallback:expr) => {{
        let mut result = None;
        for (alloc_ref, mem_type) in $crate::allocator::STATIC_ALLOCATORS.iter() {
            if *mem_type == $memory_type {
                let $alloc = *alloc_ref;
                result = Some($action);
                break;
            }
        }
        match result {
            Some(value) => value,
            None => $fallback,
        }
    }};
}
