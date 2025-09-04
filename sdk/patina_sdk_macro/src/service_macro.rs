//! A module containing Macro(s) implementation details for creating a IntoService implementation.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{Attribute, Generics, ItemEnum, ItemStruct, Meta, parse::Parse, spanned::Spanned};

/// A struct responsible for parsing any additional #[...] attributes associated with the main derive macro.
#[derive(Clone)]
struct AttrConfig {
    pub services: Vec<TokenStream>,
}

/// A struct containing the parsed struct and its attribute configs.
struct Service {
    item: ItemStruct,
    config: AttrConfig,
}

impl Service {
    /// Parses all attributes of the struct
    fn parse_attr(attrs: &mut Vec<Attribute>) -> syn::Result<AttrConfig> {
        let mut config = AttrConfig { services: vec![] };
        for attr in attrs {
            if attr.path().is_ident("service") {
                config.services = Self::parse_service_attr(attr)?;
            }
        }

        Ok(config)
    }

    /// Splits a token stream by commas.
    fn split_by_comma(ts: TokenStream) -> Vec<TokenStream> {
        let mut streams = Vec::new();
        let mut stream = TokenStream::new();

        for tt in ts.into_iter() {
            if tt.to_string() == "," {
                streams.push(stream);
                stream = TokenStream::new();
                continue;
            }
            stream.extend(tt.to_token_stream());
        }

        streams.push(stream);
        streams
    }

    /// Parses the `#[service(...)]` attribute.
    fn parse_service_attr(attr: &Attribute) -> syn::Result<Vec<TokenStream>> {
        let Meta::List(meta_list) = &attr.meta else {
            return Err(syn::Error::new(attr.span(), "Expected #[service(...)]"));
        };

        let mut services = Vec::new();
        for s in Self::split_by_comma(meta_list.tokens.clone()) {
            services.push(quote!(#s));
        }

        Ok(services)
    }

    /// Returns the name [Ident](syn::Ident) of the struct
    fn ident(&self) -> &syn::Ident {
        &self.item.ident
    }

    /// Returns the parsed attribute configuration.
    fn config(&self) -> &AttrConfig {
        &self.config
    }

    /// Returns a unique identifier for the allocator.
    fn alloc_name(&self) -> proc_macro2::Ident {
        format_ident!("__alloc_service_{}", self.ident())
    }

    /// Returns a token stream containing code to register 0..N services.
    fn service_register(&self) -> TokenStream {
        let mut tokens = TokenStream::new();
        let alloc_name = self.alloc_name();

        for service in &self.config.services {
            tokens.extend(quote! {
                let ref_service: &'static #service = leaked;
                let any: &'static dyn core::any::Any = #alloc_name::boxed::Box::leak(#alloc_name::boxed::Box::new(ref_service));
                Self::register_service::<#service>(storage, any);
            })
        }
        tokens
    }

    /// The generics for the struct
    fn generics(&self) -> Generics {
        self.item.generics.clone()
    }

    /// The left hand side generics for the struct, which can include trait bounds.
    fn lhs_generics(&self) -> Generics {
        self.generics()
    }

    /// The right hand side generics for the struct, which do not include trait bounds.
    ///
    /// valid: `impl<T: Debug> SomeTrait for MyStruct<T> {}`
    /// invalid: `impl SomeTrait for MyStruct<T: Debug> {}`
    fn rhs_generics(&self) -> Generics {
        let mut generics = self.generics();
        for param in generics.params.iter_mut() {
            if let syn::GenericParam::Type(param) = param {
                param.bounds.clear();
            }
        }
        generics.where_clause = None;
        generics
    }
}

impl TryFrom<ItemStruct> for Service {
    type Error = syn::Error;

    fn try_from(mut item: ItemStruct) -> Result<Self, Self::Error> {
        let config = Self::parse_attr(&mut item.attrs)?;
        Ok(Service { item, config })
    }
}

impl Parse for Service {
    fn parse(stream: syn::parse::ParseStream) -> syn::Result<Self> {
        if stream.fork().parse::<ItemStruct>().is_ok() {
            Ok(stream.parse::<ItemStruct>().and_then(Service::try_from)?)
        } else if stream.fork().parse::<ItemEnum>().is_ok() {
            Err(syn::Error::new(stream.span(), "Enum types are not currently supported."))
        } else {
            Err(syn::Error::new(stream.span(), "Union types are not currently supported."))
        }
    }
}

/// The testable version of the `service` macro that uses proc_macro2::Tokenstreams.
pub(crate) fn service2(item: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let service = match syn::parse2::<Service>(item) {
        Ok(service) => service,
        Err(e) => return e.to_compile_error(),
    };

    let config = service.config();

    if config.services.is_empty() {
        return syn::Error::new(
            service.ident().span(),
            "At least one Service #[service(Service1, Service2, ...)] must be specified.",
        )
        .to_compile_error();
    }

    // Tokens for expanding the IntoService trait implementation.
    let name = service.ident();
    let lhs = service.lhs_generics();
    let rhs = service.rhs_generics();
    let where_clause = &service.generics().where_clause;
    let alloc_name = service.alloc_name();
    let service_register = service.service_register();

    quote! {
        extern crate alloc as #alloc_name;
        impl #lhs patina_sdk::component::service::IntoService for #name #rhs #where_clause {

            fn register(self, storage: &mut patina_sdk::component::Storage) {
                let leaked: &'static Self = #alloc_name::boxed::Box::leak(#alloc_name::boxed::Box::new(self));
                #service_register
            }
        }

        impl #lhs patina_sdk::component::service::IntoService for &'static #name #rhs #where_clause {

            fn register(self, storage: &mut patina_sdk::component::Storage) {
                let leaked: Self = self;
                #service_register
            }
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn test_basic_struct_parse() {
        let input = quote! {
            #[service(dyn MyService)]
            struct MyStruct;
        };

        let expected = quote! {
            extern crate alloc as __alloc_service_MyStruct;
            impl patina_sdk::component::service::IntoService for MyStruct {
                fn register(self, storage: &mut patina_sdk::component::Storage) {
                    let leaked: &'static Self = __alloc_service_MyStruct::boxed::Box::leak(__alloc_service_MyStruct::boxed::Box::new(self));
                    let ref_service: &'static dyn MyService = leaked;
                    let any: &'static dyn core::any::Any = __alloc_service_MyStruct::boxed::Box::leak(__alloc_service_MyStruct::boxed::Box::new(ref_service));
                    Self::register_service::<dyn MyService>(storage, any);
                }
            }

            impl patina_sdk::component::service::IntoService for &'static MyStruct {
                fn register(self, storage: &mut patina_sdk::component::Storage) {
                    let leaked: Self = self;
                    let ref_service: &'static dyn MyService = leaked;
                    let any: &'static dyn core::any::Any = __alloc_service_MyStruct::boxed::Box::leak(__alloc_service_MyStruct::boxed::Box::new(ref_service));
                    Self::register_service::<dyn MyService>(storage, any);
                }
            }
        };

        assert_eq!(expected.to_string(), service2(input).to_string());
    }

    #[test]
    fn test_struct_with_multiple_services() {
        let input = quote! {
            #[service(dyn MyService, dyn MyService2)]
            struct MyStruct;
        };

        let expected = quote! {
            extern crate alloc as __alloc_service_MyStruct;
            impl patina_sdk::component::service::IntoService for MyStruct {
                fn register(self, storage: &mut patina_sdk::component::Storage) {
                    let leaked: &'static Self = __alloc_service_MyStruct::boxed::Box::leak(__alloc_service_MyStruct::boxed::Box::new(self));
                    let ref_service: &'static dyn MyService = leaked;
                    let any: &'static dyn core::any::Any = __alloc_service_MyStruct::boxed::Box::leak(__alloc_service_MyStruct::boxed::Box::new(ref_service));
                    Self::register_service::<dyn MyService>(storage, any);
                    let ref_service: &'static dyn MyService2 = leaked;
                    let any: &'static dyn core::any::Any = __alloc_service_MyStruct::boxed::Box::leak(__alloc_service_MyStruct::boxed::Box::new(ref_service));
                    Self::register_service::<dyn MyService2>(storage, any);
                }
            }

            impl patina_sdk::component::service::IntoService for &'static MyStruct {
                fn register(self, storage: &mut patina_sdk::component::Storage) {
                    let leaked: Self = self;
                    let ref_service: &'static dyn MyService = leaked;
                    let any: &'static dyn core::any::Any = __alloc_service_MyStruct::boxed::Box::leak(__alloc_service_MyStruct::boxed::Box::new(ref_service));
                    Self::register_service::<dyn MyService>(storage, any);
                    let ref_service: &'static dyn MyService2 = leaked;
                    let any: &'static dyn core::any::Any = __alloc_service_MyStruct::boxed::Box::leak(__alloc_service_MyStruct::boxed::Box::new(ref_service));
                    Self::register_service::<dyn MyService2>(storage, any);
                }
            }
        };

        assert_eq!(expected.to_string(), service2(input).to_string());
    }

    #[test]
    fn test_struct_with_generic_and_where_clause() {
        let input = quote! {
            #[service(dyn MyService)]
            struct MyStruct<T: Debug> where T: Clone;
        };

        let expected = quote! {
            extern crate alloc as __alloc_service_MyStruct;
            impl<T: Debug> patina_sdk::component::service::IntoService for MyStruct<T> where T: Clone {
                fn register(self, storage: &mut patina_sdk::component::Storage) {
                    let leaked: &'static Self = __alloc_service_MyStruct::boxed::Box::leak(__alloc_service_MyStruct::boxed::Box::new(self));
                    let ref_service: &'static dyn MyService = leaked;
                    let any: &'static dyn core::any::Any = __alloc_service_MyStruct::boxed::Box::leak(__alloc_service_MyStruct::boxed::Box::new(ref_service));
                    Self::register_service::<dyn MyService>(storage, any);
                }
            }

            impl<T: Debug> patina_sdk::component::service::IntoService for &'static MyStruct<T> where T: Clone {
                fn register(self, storage: &mut patina_sdk::component::Storage) {
                    let leaked: Self = self;
                    let ref_service: &'static dyn MyService = leaked;
                    let any: &'static dyn core::any::Any = __alloc_service_MyStruct::boxed::Box::leak(__alloc_service_MyStruct::boxed::Box::new(ref_service));
                    Self::register_service::<dyn MyService>(storage, any);
                }
            }
        };

        assert_eq!(expected.to_string(), service2(input).to_string());
    }

    #[test]
    fn test_enum_gives_good_error() {
        let input = quote! {
            #[service(dyn MyService)]
            enum MyUnion {
                A,
                B,
            }
        };

        let expected = quote! {
            :: core :: compile_error ! { "Enum types are not currently supported." }
        };

        assert_eq!(expected.to_string(), service2(input).to_string());
    }

    #[test]
    fn test_union_gives_good_error() {
        let input = quote! {
            #[service(dyn MyService)]
            union MyUnion {
                a: u32,
                b: u32,
            }
        };

        let expected = quote! {
            :: core :: compile_error ! { "Union types are not currently supported." }
        };

        assert_eq!(expected.to_string(), service2(input).to_string());
    }

    #[test]
    fn test_bad_service_format() {
        let input = quote! {
            #[service = MyService]
            struct MyStruct;
        };

        let expected = quote! {
            :: core :: compile_error ! { "Expected #[service(...)]" }
        };

        assert_eq!(expected.to_string(), service2(input).to_string());
    }

    #[test]
    fn test_no_service_or_protocol_gives_error() {
        let input = quote! {
            struct MyStruct;
        };

        let expected = quote! {
            :: core :: compile_error ! { "At least one Service #[service(Service1, Service2, ...)] must be specified." }
        };

        assert_eq!(expected.to_string(), service2(input).to_string());
    }
}
