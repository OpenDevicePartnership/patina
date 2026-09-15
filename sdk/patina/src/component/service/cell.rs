//! A wait-free, write-once cell for sharing a [`Service<T>`](super::Service) with code that cannot receive it
//! through dependency injection.
//!
//! [`Service<T>`](super::Service) and other [`Param`](crate::component::params::Param) types are retrieved through
//! the component dispatcher, which requires a `Storage` to resolve from. Some code has no such context to work
//! with. Two cases come up in practice:
//!
//! - An `extern "efiapi"` function backing an EDK II protocol, whose signature is fixed by the protocol definition
//!   and carries no context/`this` parameter to route a service through.
//! - A `struct` or `static` that must exist before the dependency injection system does, such as one built with a
//!   `const fn` constructor so it can be a top-level `static`. The service it needs is only available later, once
//!   a component's entry point runs and resolves it.
//!
//! [`ServiceCell<T>`] handles both cases. A component's entry point (or an init method it calls) publishes its
//! injected [`Service<T>`](super::Service) into a `static ServiceCell`, or a `ServiceCell` field, and the
//! context-free function or not-yet-ready struct reads it back later through [`ServiceCell::get`], which returns
//! `None` until the service is published.
//!
//! ## Avoiding `spin::Once`, `TplMutex`, or a plain `Mutex`
//!
//! See [Synchronization](https://github.com/OpenDevicePartnership/patina/blob/main/docs/src/dxe_core/synchronization.md)
//! for Patina's main synchronization design principles. The explanation below briefly states why `ServiceCell<T>` is
//! designed to be "wait-free".
//!
//! - Non-TPL-aware spinning primitives (`spin::mutex`, `spin::rwlock`, and `spin::Once`) are prone to TPL-inversion
//!   deadlocks in UEFI code. If a lower-TPL context is interrupted by a higher-TPL context that spins waiting for it,
//!   the lower-TPL context can never resume to unblock it. `spin::Once::call_once` blocks the caller if another call
//!   is in progress. [`ServiceCell::publish`] never blocks or spins and returns the rejected service immediately.
//! - `TplMutex` requires choosing a fixed TPL ceiling that no caller may exceed. While some services have TPL caller
//!   restrictions defined in the UEFI Specification, many do not. And because many protocols are invoked by C code
//!   outside of Patina, a TPL ceiling is difficult to enforce in practice.
//!
//! [`ServiceCell<T>`] avoids these problems by being "wait-free". Every operation completes in a bounded number of
//! steps using atomic operations.
//!
//! ## Example
//!
//! ```rust
//! use patina::component::service::{Service, cell::ServiceCell};
//!
//! # use std::boxed::Box;
//! trait MyService {
//!     fn value(&self) -> u32;
//! }
//!
//! struct MyServiceImpl;
//!
//! impl MyService for MyServiceImpl {
//!     fn value(&self) -> u32 {
//!         42
//!     }
//! }
//!
//! static CURRENT: ServiceCell<dyn MyService> = ServiceCell::new();
//!
//! // From a component's entry point, once the service is available:
//! # let service: Service<dyn MyService> = Service::mock(Box::new(MyServiceImpl));
//! CURRENT.publish(service).expect("published exactly once");
//!
//! // From a context-free callback, later:
//! assert_eq!(CURRENT.get().map(|s| s.value()), Some(42));
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{
    cell::OnceCell,
    sync::atomic::{AtomicBool, Ordering},
};

use super::Service;

/// A wait-free, write-once-then-read-many cell for a [`Service<T>`](super::Service).
///
/// See the [module documentation](self) for the motivation and the reasoning behind its design.
pub struct ServiceCell<T: ?Sized + 'static> {
    value: OnceCell<Service<T>>,
    writing: AtomicBool,
}

// SAFETY: All writes to `value` go through `publish`, which is serialized by a `compare_exchange` on `writing` so
// that at most one writer ever calls `OnceCell::set`. Reads through `get` only observe `value` after confirming (with
// an `Acquire` load of `writing`) that no write is in progress, which paired with the `Release` store at the end of
// `publish`, establishes a happens-before relationship with the completed write. `Service<T>` is `Send` and `Sync`
// unconditionally regardless of `T`, so no further bound on `T` is needed here.
unsafe impl<T: ?Sized + 'static> Sync for ServiceCell<T> {}

impl<T: ?Sized + 'static> ServiceCell<T> {
    /// Creates a new, empty cell.
    pub const fn new() -> Self {
        Self { value: OnceCell::new(), writing: AtomicBool::new(false) }
    }

    /// Publishes `service` into the cell.
    ///
    /// Never blocks. If another call to `publish` is in progress, or the cell is already published, `service` is
    /// returned back to the caller immediately rather than waiting.
    ///
    /// # Errors
    ///
    /// Returns `Err(service)` if the cell was already published, or another call to `publish` is currently in
    /// progress.
    pub fn publish(&self, service: Service<T>) -> Result<(), Service<T>> {
        if self.writing.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            let result = self.value.set(service);
            self.writing.store(false, Ordering::Release);
            return result;
        }
        Err(service)
    }

    /// Returns the published service, if any.
    ///
    /// Never blocks. Returns `None` both when nothing has been published yet and while a concurrent call to
    /// `publish` is in progress.
    pub fn get(&self) -> Option<Service<T>> {
        if self.writing.load(Ordering::Acquire) { None } else { self.value.get().copied() }
    }

    /// Returns the provided default result if nothing has been published yet, or applies `f` to the published
    /// service.
    ///
    /// Shorthand for `self.get().map_or(default, f)`.
    ///
    /// Arguments passed to `map_or` are eagerly evaluated. If you are passing the result of a function call,
    /// consider `map_or_else` instead, which is lazily evaluated.
    pub fn map_or<U>(&self, default: U, f: impl FnOnce(Service<T>) -> U) -> U {
        self.get().map_or(default, f)
    }

    /// Returns the result of applying `f` to the published service, or the result of `default` if nothing has been
    /// published yet.
    ///
    /// Shorthand for `self.get().map_or_else(default, f)`.
    pub fn map_or_else<U>(&self, default: impl FnOnce() -> U, f: impl FnOnce(Service<T>) -> U) -> U {
        self.get().map_or_else(default, f)
    }

    /// Returns the result of applying `f` to the published service, or `U::default()` if nothing has been published
    /// yet.
    pub fn map_or_default<U: Default>(&self, f: impl FnOnce(Service<T>) -> U) -> U {
        self.get().map(f).unwrap_or_default()
    }

    /// Returns `true` if a service has been published.
    pub fn is_published(&self) -> bool {
        self.get().is_some()
    }
}

impl<T: ?Sized + 'static> Default for ServiceCell<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;

    trait TestService {
        fn value(&self) -> u32;
    }

    struct TestServiceImpl(u32);

    impl TestService for TestServiceImpl {
        fn value(&self) -> u32 {
            self.0
        }
    }

    fn mock_service(value: u32) -> Service<dyn TestService> {
        Service::mock(Box::new(TestServiceImpl(value)))
    }

    #[test]
    fn test_service_cell_get_before_publish_returns_none() {
        let cell: ServiceCell<dyn TestService> = ServiceCell::new();
        assert!(cell.get().is_none());
        assert!(!cell.is_published());
    }

    #[test]
    fn test_service_cell_publish_then_get() {
        let cell: ServiceCell<dyn TestService> = ServiceCell::new();
        assert!(cell.publish(mock_service(7)).is_ok());
        assert_eq!(cell.get().map(|s| s.value()), Some(7));
        assert!(cell.is_published());
    }

    #[test]
    fn test_service_cell_double_publish_returns_service() {
        let cell: ServiceCell<dyn TestService> = ServiceCell::new();
        assert!(cell.publish(mock_service(1)).is_ok());
        let Err(rejected) = cell.publish(mock_service(2)) else {
            panic!("expected the second publish to be rejected");
        };
        assert_eq!(rejected.value(), 2);
        // The first published service is unchanged.
        assert_eq!(cell.get().map(|s| s.value()), Some(1));
    }

    #[test]
    fn test_service_cell_get_returns_none_while_write_in_progress() {
        let cell: ServiceCell<dyn TestService> = ServiceCell::new();
        // Simulate another context that has won the publish race but hasn't finished yet.
        cell.writing.store(true, Ordering::Release);
        assert!(cell.get().is_none());
        assert!(!cell.is_published());
    }

    #[test]
    fn test_service_cell_publish_returns_service_while_write_in_progress() {
        let cell: ServiceCell<dyn TestService> = ServiceCell::new();
        cell.writing.store(true, Ordering::Release);
        let Err(rejected) = cell.publish(mock_service(5)) else { panic!("expected publish to be rejected") };
        assert_eq!(rejected.value(), 5);
    }

    #[test]
    fn test_service_cell_default_is_empty() {
        let cell: ServiceCell<dyn TestService> = ServiceCell::default();
        assert!(cell.get().is_none());
    }

    #[test]
    fn test_service_cell_map_or_returns_default_before_publish() {
        let cell: ServiceCell<dyn TestService> = ServiceCell::new();
        assert_eq!(cell.map_or(42, |s| s.value()), 42);
    }

    #[test]
    fn test_service_cell_map_or_returns_mapped_value_after_publish() {
        let cell: ServiceCell<dyn TestService> = ServiceCell::new();
        cell.publish(mock_service(7)).expect("published exactly once");
        assert_eq!(cell.map_or(42, |s| s.value()), 7);
    }

    #[test]
    fn test_service_cell_map_or_else_calls_default_before_publish() {
        let cell: ServiceCell<dyn TestService> = ServiceCell::new();
        assert_eq!(cell.map_or_else(|| 42, |s| s.value()), 42);
    }

    #[test]
    fn test_service_cell_map_or_else_returns_mapped_value_after_publish() {
        let cell: ServiceCell<dyn TestService> = ServiceCell::new();
        cell.publish(mock_service(7)).expect("published exactly once");
        assert_eq!(cell.map_or_else(|| 42, |s| s.value()), 7);
    }

    #[test]
    fn test_service_cell_map_or_default_returns_default_before_publish() {
        let cell: ServiceCell<dyn TestService> = ServiceCell::new();
        assert_eq!(cell.map_or_default(|s| s.value()), 0);
    }

    #[test]
    fn test_service_cell_map_or_default_returns_mapped_value_after_publish() {
        let cell: ServiceCell<dyn TestService> = ServiceCell::new();
        cell.publish(mock_service(7)).expect("published exactly once");
        assert_eq!(cell.map_or_default(|s| s.value()), 7);
    }
}
