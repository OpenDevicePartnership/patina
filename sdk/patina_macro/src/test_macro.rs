//! This crate provides a procedural macro for creating UEFI tests.
//!
//! The macro is used as an attribute on a function and will generate a test case that is automatically
//! discovered and run by the UEFI test runner.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use std::collections::HashMap;

use quote::{ToTokens, format_ident, quote};
use syn::{Attribute, ItemFn, Meta, Token, parse::Parser, punctuated::Punctuated, spanned::Spanned};

const KEY_SHOULD_FAIL: &str = "should_fail";
const KEY_FAIL_MSG: &str = "fail_msg";
const KEY_SKIP: &str = "skip";
const KEY_TRIGGER: &str = "trigger";

pub fn patina_test2(stream: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let mut item = match syn::parse2::<ItemFn>(stream) {
        Ok(i) => i,
        Err(e) => return e.to_compile_error(),
    };

    let test_case_config = match process_attributes(&mut item) {
        Ok(cfg) => cfg,
        Err(e) => return e.to_compile_error(),
    };

    // Wait until we filter out or custom attributes so that we don't confuse the compiler
    // with attributes it does not expect.
    if cfg!(not(feature = "enable_patina_tests")) {
        return handle_feature_off(item);
    }

    generate_expanded_test_case(&item, &test_case_config)
}

/// Consumes any attributes owned by `patina_test` and returns a map of the configuration.
fn process_attributes(item: &mut ItemFn) -> syn::Result<HashMap<&'static str, proc_macro2::TokenStream>> {
    let mut map = HashMap::new();

    map.insert(KEY_SHOULD_FAIL, quote! {false});
    map.insert(KEY_FAIL_MSG, quote! {None});
    map.insert(KEY_SKIP, quote! {false});
    map.insert(KEY_TRIGGER, quote! { patina::test::__private_api::TestTrigger::Immediate });

    let mut result = Ok(());
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
        if attr.path().is_ident("on") {
            match parse_on_attr(attr) {
                Ok(tokens) => {
                    map.insert(KEY_TRIGGER, tokens);
                    return false;
                }
                Err(e) => {
                    result = Err(e);
                    return false;
                }
            };
        }
        true
    });

    result.map(|_| map)
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

// returns a token stream for the trigger struct field
fn parse_on_attr(attr: &Attribute) -> syn::Result<proc_macro2::TokenStream> {
    // Attribute starts with "on". Lets make sure its of the format "on(...)".
    if let Meta::List(ml) = &attr.meta {
        let parser = Punctuated::<Meta, Token![,]>::parse_terminated;

        // For now, we only support a single key-value pair in the list so we can just return an error if anything
        // else is found. This makes for less code to change if we add more config.
        #[allow(clippy::never_loop)]
        for meta in parser.parse2(ml.tokens.clone())? {
            match meta {
                // CASE1: $[on(event = module_path_to_guid)]
                Meta::NameValue(nv) if nv.path.is_ident("event") => {
                    let value = &nv.value;
                    return Ok(quote! {
                        patina::test::__private_api::TestTrigger::Event(&#value)
                    });
                }
                // CASE2: $[on(timer = interval_in_100ns_units)]
                Meta::NameValue(nv) if nv.path.is_ident("timer") => {
                    let value = &nv.value;
                    return Ok(quote! {
                        patina::test::__private_api::TestTrigger::Timer(#value)
                    });
                }
                // No other cases are supported right now.
                _ => {
                    return Err(syn::Error::new(
                        meta.span(),
                        "Unsupported attribute key. See patina::test::__private_api::TestTrigger for supported keys.",
                    ));
                }
            }
        }
    }
    Err(syn::Error::new(attr.span(), "Expected valid attribute format. e.g. #[on(...)]"))
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
    let trigger = test_case_config.get(KEY_TRIGGER).expect("All configuration should have a default value set.");

    let expanded = quote! {
        #[patina::test::linkme::distributed_slice(patina::test::__private_api::TEST_CASES)]
        #[linkme(crate = patina::test::linkme)]
        #[allow(non_upper_case_globals)]
        static #struct_name: patina::test::__private_api::TestCase =
        patina::test::__private_api::TestCase {
            name: concat!(module_path!(), "::", stringify!(#fn_name)),
            trigger: #trigger,
            skip: #skip,
            should_fail: #should_fail,
            fail_msg: #fail_msg,
            func: |storage| patina::test::__private_api::FunctionTest::new(#fn_name).run(storage.into()),
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

        let expected = quote! {
            ::core::compile_error ! { "expected `fn`" }
        };

        assert_eq!(patina_test2(stream).to_string(), expected.to_string(),);
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
                #[patina::test::linkme::distributed_slice(patina::test::__private_api::TEST_CASES)]
                #[linkme(crate = patina::test::linkme)]
                #[allow(non_upper_case_globals)]
                static __my_test_case_TestCase: patina::test::__private_api::TestCase = patina::test::__private_api::TestCase {
                    name: concat!(module_path!(), "::", stringify!(my_test_case)),
                    trigger: patina::test::__private_api::TestTrigger::Immediate,
                    skip: false,
                    should_fail: false,
                    fail_msg: None,
                    func: |storage| patina::test::__private_api::FunctionTest::new(my_test_case).run(storage.into()),
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
                #[patina::test::linkme::distributed_slice(patina::test::__private_api::TEST_CASES)]
                #[linkme(crate = patina::test::linkme)]
                #[allow(non_upper_case_globals)]
                static __my_test_case_TestCase: patina::test::__private_api::TestCase =
                patina::test::__private_api::TestCase {
                    name: concat!(module_path!(), "::", stringify!(my_test_case)),
                    trigger: patina::test::__private_api::TestTrigger::Immediate,
                    skip: true,
                    should_fail: false,
                    fail_msg: None,
                    func: |storage| patina::test::__private_api::FunctionTest::new(my_test_case).run(storage.into()),
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
    fn test_process_on_event_attribute() {
        let attr = syn::parse_quote! { #[on(event = patina::guids::EVENT_GROUP_END_OF_DXE)] };
        let tokens = parse_on_attr(&attr).unwrap();

        let expected = quote! {
            patina::test::__private_api::TestTrigger::Event(&patina::guids::EVENT_GROUP_END_OF_DXE)
        };
        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn test_improper_on_event_attribute() {
        let attr = syn::parse_quote! { #[on(event = )] };
        assert!(parse_on_attr(&attr).is_err());

        let attr = syn::parse_quote! { #[on(junk = patina::guids::EVENT_GROUP_END_OF_DXE)] };
        assert!(parse_on_attr(&attr).is_err());

        let attr = syn::parse_quote! { #[on()] };
        assert!(parse_on_attr(&attr).is_err());

        let attr = syn::parse_quote! { #[on] };
        assert!(parse_on_attr(&attr).is_err());
    }

    #[test]
    fn test_process_on_timer_attribute() {
        let attr = syn::parse_quote! { #[on(timer = 1000000)] };
        let tokens = parse_on_attr(&attr).unwrap();

        let expected = quote! {
            patina::test::__private_api::TestTrigger::Timer(1000000)
        };
        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn test_improper_on_timer_attribute() {
        let attr = syn::parse_quote! { #[on(timer = )] };
        assert!(parse_on_attr(&attr).is_err());

        let attr = syn::parse_quote! { #[on(junk = 1000000)] };
        assert!(parse_on_attr(&attr).is_err());

        let attr = syn::parse_quote! { #[on()] };
        assert!(parse_on_attr(&attr).is_err());

        let attr = syn::parse_quote! { #[on] };
        assert!(parse_on_attr(&attr).is_err());
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
        let tc_cfg = process_attributes(&mut test_fn).unwrap();

        // Our attributes are consumed, Others are ignored.
        assert_eq!(test_fn.attrs.len(), 1);

        // Test proper configuration
        assert_eq!(tc_cfg.len(), 4); // If we add more attributes, this breaks, and we know to add more to the test.

        assert_eq!(tc_cfg.get(KEY_SHOULD_FAIL).unwrap().to_string(), "true");
        assert_eq!(tc_cfg.get(KEY_FAIL_MSG).unwrap().to_string(), "Some (\"Expected Error\")");
        assert_eq!(tc_cfg.get(KEY_SKIP).unwrap().to_string(), "true");
        assert_eq!(
            tc_cfg.get(KEY_TRIGGER).unwrap().to_string(),
            "patina :: test :: __private_api :: TestTrigger :: Immediate"
        );
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

    #[test]
    fn patina_test2_bad_attributes() {
        let stream = quote! {
            #[patina_test]
            #[on(bad_thing)]
            fn my_test_case() -> Result {
                assert!(true);
            }
        };

        let expanded = patina_test2(stream);
        let expected = quote! {
            ::core::compile_error ! { "Unsupported attribute key. See patina::test::__private_api::TestTrigger for supported keys." }
        };
        assert_eq!(expanded.to_string(), expected.to_string());
    }

    #[test]
    fn patina_test2_good() {
        let stream = quote! {
            #[patina_test]
            #[should_fail = "Expected Error"]
            #[skip]
            #[on(event = patina::guids::EVENT_GROUP_END_OF_DXE)]
            fn my_test_case() -> Result {
                assert!(true);
            }
        };

        let expanded = patina_test2(stream);
        let expected = if cfg!(feature = "enable_patina_tests") {
            quote! {
                #[patina::test::linkme::distributed_slice(patina::test::__private_api::TEST_CASES)]
                #[linkme(crate = patina::test::linkme)]
                #[allow(non_upper_case_globals)]
                static __my_test_case_TestCase: patina::test::__private_api::TestCase =
                patina::test::__private_api::TestCase {
                    name: concat!(module_path!(), "::", stringify!(my_test_case)),
                    trigger: patina::test::__private_api::TestTrigger::Event(&patina::guids::EVENT_GROUP_END_OF_DXE),
                    skip: true,
                    should_fail: true,
                    fail_msg: Some("Expected Error"),
                    func: |storage| patina::test::__private_api::FunctionTest::new(my_test_case).run(storage.into()),
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
    fn test_generate_expanded_test_case() {
        let quoted_fn = quote! {
            fn my_test_case() -> Result {
                assert!(true);
            }
        };

        let item = syn::parse2::<ItemFn>(quoted_fn).unwrap();

        let mut config = HashMap::new();
        config.insert(KEY_SHOULD_FAIL, quote! {true});
        config.insert(KEY_FAIL_MSG, quote! {Some("Expected Error")});
        config.insert(KEY_SKIP, quote! {false});
        config.insert(KEY_TRIGGER, quote! { patina::test::__private_api::TestTrigger::Immediate });

        let expanded = generate_expanded_test_case(&item, &config);

        let expected = quote! {
            #[patina::test::linkme::distributed_slice(patina::test::__private_api::TEST_CASES)]
            #[linkme(crate = patina::test::linkme)]
            #[allow(non_upper_case_globals)]
            static __my_test_case_TestCase: patina::test::__private_api::TestCase =
            patina::test::__private_api::TestCase {
                name: concat!(module_path!(), "::", stringify!(my_test_case)),
                trigger: patina::test::__private_api::TestTrigger::Immediate,
                skip: false,
                should_fail: true,
                fail_msg: Some("Expected Error"),
                func: |storage| patina::test::__private_api::FunctionTest::new(my_test_case).run(storage.into()),
            };
            fn my_test_case() -> Result {
                assert!(true);
            }
        };

        assert_eq!(expanded.to_string(), expected.to_string());
    }
}
