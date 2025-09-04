//! This crate provides a procedural macro for creating UEFI tests.
//!
//! The macro is used as an attribute on a function and will generate a test case that is automatically
//! discovered and run by the UEFI test runner.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use std::collections::HashMap;

use quote::{ToTokens, format_ident, quote};
use syn::{Attribute, ItemFn, Meta};

const KEY_SHOULD_FAIL: &str = "should_fail";
const KEY_FAIL_MSG: &str = "fail_msg";
const KEY_SKIP: &str = "skip";

pub fn patina_test2(stream: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let mut item =
        syn::parse2::<ItemFn>(stream).expect("The #[patina_test] attribute can only be applied to functions");
    let test_case_config = process_attributes(&mut item);

    // Wait until we filter out or custom attributes so that we don't confuse the compiler
    // with attributes it does not expect.
    if cfg!(not(feature = "enable_patina_tests")) {
        return handle_feature_off(item);
    }

    generate_expanded_test_case(&item, &test_case_config)
}

/// Consumes any attributes owned by `patina_test` and returns a map of the configuration.
fn process_attributes(item: &mut ItemFn) -> HashMap<&'static str, proc_macro2::TokenStream> {
    let mut map = HashMap::new();

    map.insert(KEY_SHOULD_FAIL, quote! {false});
    map.insert(KEY_FAIL_MSG, quote! {None});
    map.insert(KEY_SKIP, quote! {false});

    item.attrs.retain(|attr| {
        if attr.path().is_ident("patina_test") {
            return false;
        }
        if attr.path().is_ident("should_fail") {
            let (should_fail, fail_msg) = parse_should_fail_attr(attr);
            map.insert(KEY_SHOULD_FAIL, should_fail);
            map.insert(KEY_FAIL_MSG, fail_msg);
            return false;
        }
        if attr.path().is_ident("skip") {
            let skip = parse_skip_attr(attr);
            map.insert(KEY_SKIP, skip);
            return false;
        }
        true
    });

    map
}

/// Adds an `#[allow(dead_code)]` attribute to the function to prevent warnings.
fn handle_feature_off(mut item: ItemFn) -> proc_macro2::TokenStream {
    let allow_dead_code: Attribute = syn::parse_quote! {#[allow(dead_code)]};
    item.attrs.push(allow_dead_code);
    item.to_token_stream()
}

// Returns (`should_fail`, `fail_msg`) as a token stream for placement in the expanded code
fn parse_should_fail_attr(attr: &Attribute) -> (proc_macro2::TokenStream, proc_macro2::TokenStream) {
    // CASE1: #[should_fail = "message"]
    if let Meta::NameValue(nv) = &attr.meta
        && let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value
    {
        return (quote! {true}, quote! {Some(#s)});
    }
    // CASE2: #[should_fail]
    if let Meta::Path(_) = &attr.meta {
        return (quote! {true}, quote! {None});
    }
    panic!("#[should_fail] attribute must be a string literal. e.g. #[should_fail] or #[should_fail = \"message\"]");
}

// Returns `skip` as a token stream for placement in the expanded code
fn parse_skip_attr(attr: &Attribute) -> proc_macro2::TokenStream {
    // CASE1: #[skip]
    if let Meta::Path(_) = &attr.meta {
        return quote! {true};
    }
    panic!("#[skip] attribute must be empty. e.g. #[skip]");
}

fn generate_expanded_test_case(
    item: &ItemFn,
    test_case_config: &HashMap<&'static str, proc_macro2::TokenStream>,
) -> proc_macro2::TokenStream {
    let fn_name = &item.sig.ident; // The Component function's name
    let struct_name = format_ident!("__{}_TestCase", fn_name);

    // Extract the configuration
    let should_fail =
        test_case_config.get(KEY_SHOULD_FAIL).expect("All configuration should have a default value set.");
    let fail_msg = test_case_config.get(KEY_FAIL_MSG).expect("All configuration should have a default value set.");
    let skip = test_case_config.get(KEY_SKIP).expect("All configuration should have a default value set.");

    let expanded = quote! {
        #[patina_sdk::test::linkme::distributed_slice(patina_sdk::test::__private_api::TEST_CASES)]
        #[linkme(crate = patina_sdk::test::linkme)]
        #[allow(non_upper_case_globals)]
        static #struct_name: patina_sdk::test::__private_api::TestCase =
        patina_sdk::test::__private_api::TestCase {
            name: concat!(module_path!(), "::", stringify!(#fn_name)),
            skip: #skip,
            should_fail: #should_fail,
            fail_msg: #fail_msg,
            func: |storage| patina_sdk::test::__private_api::FunctionTest::new(#fn_name).run(storage.into()),
        };
        #item
    };

    expanded
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_attr_on_non_fn() {
        let stream = quote! {
            #[patina_test]
            struct MyStruct;
        };
        assert!(::std::panic::catch_unwind(|| patina_test2(stream)).is_err());
    }

    #[test]
    fn test_standard_use_case() {
        let stream = quote! {
            #[patina_test]
            fn my_test_case() -> Result {
                assert!(true);
            }
        };

        let expanded = patina_test2(stream);
        let expected = if cfg!(feature = "enable_patina_tests") {
            quote! {
                #[patina_sdk::test::linkme::distributed_slice(patina_sdk::test::__private_api::TEST_CASES)]
                #[linkme(crate = patina_sdk::test::linkme)]
                #[allow(non_upper_case_globals)]
                static __my_test_case_TestCase: patina_sdk::test::__private_api::TestCase = patina_sdk::test::__private_api::TestCase {
                    name: concat!(module_path!(), "::", stringify!(my_test_case)),
                    skip: false,
                    should_fail: false,
                    fail_msg: None,
                    func: |storage| patina_sdk::test::__private_api::FunctionTest::new(my_test_case).run(storage.into()),
                };
                fn my_test_case() -> Result {
                    assert!(true);
                }
            }
        } else {
            quote! {
                #[allow(dead_code)]
                fn my_test_case() -> Result {
                    assert!(true);
                }
            }
        };

        assert_eq!(expanded.to_string(), expected.to_string());
    }

    #[test]
    fn test_with_skip_functionality() {
        let stream = quote! {
            #[patina_test]
            #[skip]
            fn my_test_case() -> Result {
                assert!(true);
            }
        };

        let expanded = patina_test2(stream);

        let expected = if cfg!(feature = "enable_patina_tests") {
            quote! {
                #[patina_sdk::test::linkme::distributed_slice(patina_sdk::test::__private_api::TEST_CASES)]
                #[linkme(crate = patina_sdk::test::linkme)]
                #[allow(non_upper_case_globals)]
                static __my_test_case_TestCase: patina_sdk::test::__private_api::TestCase =
                patina_sdk::test::__private_api::TestCase {
                    name: concat!(module_path!(), "::", stringify!(my_test_case)),
                    skip: true,
                    should_fail: false,
                    fail_msg: None,
                    func: |storage| patina_sdk::test::__private_api::FunctionTest::new(my_test_case).run(storage.into()),
                };
                fn my_test_case() -> Result {
                    assert!(true);
                }
            }
        } else {
            quote! {
                #[allow(dead_code)]
                fn my_test_case() -> Result {
                    assert!(true);
                }
            }
        };

        assert_eq!(expanded.to_string(), expected.to_string());
    }

    #[test]
    fn test_parse_should_fail_attr() {
        let attr = syn::parse_quote! { #[should_fail] };
        let (should_fail, fail_msg) = parse_should_fail_attr(&attr);
        assert_eq!(should_fail.to_string(), "true");
        assert_eq!(fail_msg.to_string(), "None");

        let attr = syn::parse_quote! { #[should_fail = "message"] };
        let (should_fail, fail_msg) = parse_should_fail_attr(&attr);
        assert_eq!(should_fail.to_string(), "true");
        assert_eq!(fail_msg.to_string(), "Some (\"message\")");

        let attr = syn::parse_quote! { #[should_fail = 42] };
        assert!(::std::panic::catch_unwind(|| parse_should_fail_attr(&attr)).is_err());

        let attr = syn::parse_quote! { #[should_fail("message")] };
        assert!(::std::panic::catch_unwind(|| parse_should_fail_attr(&attr)).is_err());

        let attr = syn::parse_quote! { #[should_fail("message", "junk")] };
        assert!(::std::panic::catch_unwind(|| parse_should_fail_attr(&attr)).is_err());
    }

    #[test]
    fn test_parse_skip_attr() {
        let attr = syn::parse_quote! { #[skip] };
        let skip = parse_skip_attr(&attr);
        assert_eq!(skip.to_string(), "true");

        let attr = syn::parse_quote! { #[skip = "message"] };
        assert!(::std::panic::catch_unwind(|| parse_skip_attr(&attr)).is_err());

        let attr = syn::parse_quote! { #[skip("message")] };
        assert!(::std::panic::catch_unwind(|| parse_skip_attr(&attr)).is_err());

        let attr = syn::parse_quote! { #[skip("message", "junk")] };
        assert!(::std::panic::catch_unwind(|| parse_skip_attr(&attr)).is_err());
    }

    #[test]
    fn test_process_multiple_attributes() {
        let stream = quote! {
            #[patina_test]
            #[should_fail = "Expected Error"]
            #[skip]
            #[not_our_attr]
            fn my_test_case() -> Result {
                assert!(true);
            }
        };

        let mut test_fn = syn::parse2::<ItemFn>(stream).unwrap();
        let tc_cfg = process_attributes(&mut test_fn);

        // Our attributes are consumed, Others are ignored.
        assert_eq!(test_fn.attrs.len(), 1);

        // Test proper configuration
        assert_eq!(tc_cfg.len(), 3); // If we add more attributes, this breaks, and we know to add more to the test.

        assert_eq!(tc_cfg.get(KEY_SHOULD_FAIL).unwrap().to_string(), "true");
        assert_eq!(tc_cfg.get(KEY_FAIL_MSG).unwrap().to_string(), "Some (\"Expected Error\")");
        assert_eq!(tc_cfg.get(KEY_SKIP).unwrap().to_string(), "true");
    }

    #[test]
    fn test_handle_feature_off() {
        let stream = quote! {
            fn my_test_case(&interface: &dyn DxeComponentInterface) -> Result {
                assert!(true);
            }
        };

        let expanded = handle_feature_off(syn::parse2(stream).unwrap());

        let expected = quote! {
            #[allow(dead_code)]
            fn my_test_case(&interface: &dyn DxeComponentInterface) -> Result {
                assert!(true);
            }
        };

        assert_eq!(expanded.to_string(), expected.to_string());
    }
}
