//! A crate containing macros to be re-exported in the `patina_sdk` crate.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#![feature(coverage_attribute)]

mod component_macro;
mod hob_macro;
mod service_macro;
mod test_macro;

/// Derive Macro for implementing the `IntoComponent` trait for a type.
///
/// This macro automatically implements the necessary traits for the provided type implementation to be used as a
/// `Component`. By default, the component attribute macro will assume a function, `Self::entry_point`, exists on the
/// type, but that can be overridden with the `entry_point` attribute.
///
/// ## Supported types
///
/// - Struct
/// - Enum
///
/// ## Macro Attribute
///
/// - `entry_point`: The function to be called when the component is executed.
///
/// ## Examples
///
/// ```rust, ignore
/// use patina_sdk::{
///     error::Result,
///     component::{
///         IntoComponent,
///         params::Config,
///     },
/// };
///
/// #[derive(IntoComponent)]
/// struct MyStruct(u32);
///
/// impl MyStruct {
///
///     fn entry_point(self, _cfg: Config<String>) -> Result<()> {
///         Ok(())
///     }
/// }
///
/// #[derive(IntoComponent)]
/// #[entry_point(path = driver)]
/// struct MyStruct2(u32);
///
/// fn driver(s: MyStruct2, _cfg: Config<String>) -> Result<()> {
///    Ok(())
/// }
///
/// #[derive(IntoComponent)]
/// #[entry_point(path = MyEnum::run_me)]
/// enum MyEnum {
///    A,
///    B,
/// }
///
/// impl MyEnum {
///    fn run_me(self, _cfg: Config<String>) -> Result<()> {
///       Ok(())
///   }
/// }
/// ```
#[proc_macro_derive(IntoComponent, attributes(entry_point, protocol))]
pub fn component(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    component_macro::component2(item.into()).into()
}

/// Derive Macro for implementing the `IntoService` trait for a type.
///
/// This macro automatically implements the necessary traits for the provided type implementation to be used as a
/// `Service`. By default the derive macro assumes the service is the same as the deriver, but that can be overridden
/// with the `service` attribute to specify that the service is actually a dyn \<Trait\> that the underlying type
/// implements.
///
/// ## Macro Attribute
///
/// - `service`: The service trait(s) that the type implements.
/// - `protocol`: Publishes the entire struct as a protocol with the given GUID.
///
/// ## Member Attributes
///
/// - `protocol`: Publishes the field as a protocol with the given GUID.
///
/// ## Pure Rust Example
///
/// ```rust, ignore
/// use patina_sdk::{
///    error::Result,
///    component::{
///      IntoService,
///      params::Service,
///    },
/// };
///
/// trait MyService {
///   fn do_something(&self) -> Result<()>;
/// }
///
/// #[derive(IntoService)]
/// #[service(MyService)]
/// struct MyStruct;
///
/// impl MyService for MyStruct {
///   fn do_something(&self) -> Result<()> {
///    Ok(())
///   }
/// }
/// ```
#[proc_macro_derive(IntoService, attributes(service))]
pub fn service(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    service_macro::service2(item.into()).into()
}

/// Derive Macro for implementing the `HobConfig` trait for a type.
///
/// This macro automatically implements the `HobConfig` trait for the provided type
/// by casting the passed in bytes (`&[u8]`) to the type. and cloning the struct.
///
/// This macro is inherently unsafe it it casts the pointer to the bytes to the type.
/// It is the responsibility of the developer to ensure that the type is properly formatted
/// and that the bytes are valid for the type.
///
/// The User must also implement the `Copy` trait for the type so that the bytes can be
/// copied to the new instance of the type. Due to the requirements of the `IntoConfig` trait,
/// the type must also implement the `Default` trait.
///
/// ## Macro Attribute
///
/// - `guid`: The guid to associate with the type.
///
/// ## Examples
///
/// ```rust, ignore
/// use patina_sdk::component::FromHob;
///
/// #[derive(FromHob, Copy, Clone, Default)]
/// #[guid = "8be4df61-93ca-11d2-aa0d-00e098032b8c"]
/// struct MyConfig {
///   field1: u32,
///   field2: u32,
/// }
/// ```
#[proc_macro_derive(FromHob, attributes(hob))]
pub fn hob_config(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    hob_macro::hob_config2(item.into()).into()
}

/// A proc-macro that registers the annotated function as a test case to be run by patina_test component.
///
/// There is a distinct difference between doing a #[cfg_attr(..., skip)] and a
/// #[cfg_attr(..., patina_test)]. The first still compiles the test case, but skips it at runtime. The second does not
/// compile the test case at all.
///
/// ## Attributes
///
/// - `#[should_fail]`: Indicates that the test is expected to fail. If the test passes, the test runner will log an
///   error.
/// - `#[should_fail = "message"]`: Indicates that the test is expected to fail with the given message. If the test
///   passes or fails with a different message, the test runner will log an error.
/// - `#[skip]`: Indicates that the test should be skipped.
///
/// ## Example
///
/// ```ignore
/// use patina_sdk::test::*;
/// use patina_sdk::boot_services::StandardBootServices;
/// use patina_sdk::test::patina_test;
/// use patina_sdk::{u_assert, u_assert_eq};
///
/// #[patina_test]
/// fn test_case() -> Result {
///     todo!()
/// }
///
/// #[patina_test]
/// #[should_fail]
/// fn failing_test_case() -> Result {
///     u_assert_eq!(1, 2);
///     Ok(())
/// }
///
/// #[patina_test]
/// #[should_fail = "This test failed"]
/// fn failing_test_case_with_msg() -> Result {
///    u_assert_eq!(1, 2, "This test failed");
///    Ok(())
/// }
///
/// #[patina_test]
/// #[skip]
/// fn skipped_test_case() -> Result {
///    todo!()
/// }
///
/// #[patina_test]
/// #[cfg_attr(not(target_arch = "x86_64"), skip)]
/// fn x86_64_only_test_case(bs: StandardBootServices) -> Result {
///   todo!()
/// }
/// ```
#[proc_macro_attribute]
pub fn patina_test(_: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    test_macro::patina_test2(item.into()).into()
}
