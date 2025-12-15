//! A crate containing macros to be re-exported in the `patina` crate.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#![feature(coverage_attribute)]

mod hob_macro;
mod service_macro;
mod smbios_record_macro;
mod test_macro;
mod validate_params_macro;

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
/// use patina::{
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
/// This macro uses the [zerocopy::FromBytes](https://docs.rs/zerocopy/latest/zerocopy/trait.FromBytes.html)
/// implementation to safely create an instance of the type from a byte slice. If FromBytes is not implemented on the
/// type, a compile time error will be produced.
///
/// ## Macro Attribute
///
/// - `guid`: The guid to associate with the type.
///
/// ## Examples
///
/// ```rust, ignore
/// use patina::component::FromHob;
///
/// #[derive(FromHob, zerocopy::FromBytes)]
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
/// use patina::test::*;
/// use patina::boot_services::StandardBootServices;
/// use patina::test::patina_test;
/// use patina::{u_assert, u_assert_eq};
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
///
/// #[patina_test]
/// #[on(timer = 1000000)]
/// #[on(event = patina::guids::EVENT_GROUP_END_OF_DXE)]
/// fn multi_triggered_test_case() -> Result {
///  todo!()
/// }
/// ```
#[proc_macro_attribute]
pub fn patina_test(_: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    if cfg!(feature = "enable_patina_tests") {
        test_macro::patina_test2(item.into()).into()
    } else {
        test_macro::patina_test_feature_off(item.into()).into()
    }
}

/// Derive Macro for implementing the `SmbiosRecordStructure` trait.
///
/// This macro automatically generates a complete `SmbiosRecordStructure` trait
/// implementation, eliminating the need for manual boilerplate code.
///
/// ## Macro Attributes
///
/// - `#[smbios(record_type = N)]`: **Required**. Specifies the SMBIOS type number (0-255).
///
/// ## Member Attributes
///
/// - `#[string_pool]`: Marks a field as the string pool (must be `Vec<String>`).
///   Only one field per struct can have this attribute.
///
/// ## Examples
///
/// ```rust, ignore
/// use patina_macro::SmbiosRecord;
/// use patina_smbios::{SmbiosTableHeader, SmbiosRecordStructure};
/// use alloc::{string::String, vec::Vec};
///
/// // Vendor-specific OEM record (Type 0x80-0xFF)
/// #[derive(SmbiosRecord)]
/// #[smbios(record_type = 0x80)]
/// pub struct VendorOemRecord {
///     pub header: SmbiosTableHeader,
///     pub oem_field: u32,
///     #[string_pool]
///     pub string_pool: Vec<String>,
/// }
///
/// // Custom record without strings
/// #[derive(SmbiosRecord)]
/// #[smbios(record_type = 0x81)]
/// pub struct CustomData {
///     pub header: SmbiosTableHeader,
///     pub value1: u16,
///     pub value2: u32,
/// }
/// ```
///
/// The macro generates:
/// - `const RECORD_TYPE: u8`
/// - `fn to_bytes(&self) -> Vec<u8>` - Complete serialization
/// - `fn validate(&self) -> Result<(), SmbiosError>` - String validation
/// - `fn string_pool(&self) -> &[String]` - String pool accessor
/// - `fn string_pool_mut(&mut self) -> &mut Vec<String>` - Mutable accessor
#[proc_macro_derive(SmbiosRecord, attributes(smbios, string_pool))]
pub fn smbios_record(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    smbios_record_macro::smbios_record_derive(item.into()).into()
}

/// Attribute macro for component impl blocks with automatic parameter validation.
///
/// This is the primary macro for defining components in Patina. It must be applied to impl blocks
/// containing a component's `entry_point` method.
///
/// The macro automatically:
/// - Verifies an `entry_point` method exists
/// - Validates the entry_point parameters at compile time
/// - Generates the `IntoComponent` trait implementation
///
/// ## Usage
///
/// ```rust, ignore
/// use patina::component::component;
///
/// pub struct MyComponent {
///     data: u32,
/// }
///
/// #[component]
/// impl MyComponent {
///     fn entry_point(self, config: Config<u32>) -> Result<()> {
///         Ok(())
///     }
/// }
/// ```
///
/// ## Generic Types
///
/// ```rust, ignore
/// pub struct MyComponent<T> {
///     data: T,
/// }
///
/// #[component]
/// impl<T> MyComponent<T> {
///     fn entry_point(self, config: Config<T>) -> Result<()> {
///         Ok(())
///     }
/// }
/// ```
///
/// ## Validation Rules
///
/// - Impl block must contain an `entry_point` method
/// - Entry point must have `self`, `mut self``, `&self`, or `&mut self` as the first parameter
/// - No duplicate `ConfigMut<T>` parameters with the same type T
/// - Cannot have both `Config<T>` and `ConfigMut<T>` for the same type T
/// - Cannot use `&mut Storage` with `Config<T>` or `ConfigMut<T>`
/// - Cannot use `&Storage` with `ConfigMut<T>`
/// - Cannot have multiple `Commands` parameters or multiple service table parameters
#[proc_macro_attribute]
pub fn component(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    validate_params_macro::component_entry_point(attr.into(), item.into()).into()
}
