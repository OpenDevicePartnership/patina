//! A module containing Macro(s) implementation details for working with components.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::{
    Attribute, Generics, ItemEnum, ItemStruct, Meta, Token,
    parse::{Parse, Parser},
    parse2,
    punctuated::Punctuated,
};

/// A struct responsible for parsing any additional #[...] attributes associated with the main derive attribute.
#[derive(Clone)]
struct AttrConfig {
    /// `#[entry_point = path::to::function]`: Used to override the default `Self::entry_point` entry point.
    entry_point: TokenStream,
}

/// A wrapper for simplifying parsing the supported Struct and Enum types.
enum Component {
    Struct(ItemStruct, AttrConfig),
    Enum(ItemEnum, AttrConfig),
}

impl Component {
    /// The name of the struct or enum
    fn ident(&self) -> Ident {
        match self {
            Component::Struct(item, _) => item.ident.clone(),
            Component::Enum(item, _) => item.ident.clone(),
        }
    }

    /// The attribute configuration for the struct or enum
    fn config(&self) -> AttrConfig {
        match self {
            Component::Struct(_, config) => config.clone(),
            Component::Enum(_, config) => config.clone(),
        }
    }

    /// The generics for the struct or enum
    fn generics(&self) -> Generics {
        match self {
            Component::Struct(item, _) => item.generics.clone(),
            Component::Enum(item, _) => item.generics.clone(),
        }
    }

    /// The left hand side generics for the struct or enum, which can include trait bounds.
    fn lhs_generics(&self) -> Generics {
        self.generics()
    }

    /// The right hand side generics for the struct or enum, which do not include trait bounds.
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

    /// Parses attributes associated with the struct or enum, generating a configuration struct.
    fn parse_attr(attrs: &mut Vec<Attribute>) -> syn::Result<AttrConfig> {
        let mut config = AttrConfig { entry_point: quote!(Self::entry_point) };
        for attr in attrs {
            if attr.path().is_ident("entry_point") {
                config.entry_point = Self::parse_entry_point_attr(attr)?;
            }
        }

        Ok(config)
    }

    /// Parses the `#[entry_point(path = path::to::function)]` attribute to get `path::to::function`.
    fn parse_entry_point_attr(attr: &Attribute) -> syn::Result<TokenStream> {
        // the entry_point attribute must always be a list e.g. entry_point(A, B, C)
        let Meta::List(meta_list) = &attr.meta else {
            return Err(syn::Error::new_spanned(attr, "Expected `#[entry_point(...)]`"));
        };

        let parser = Punctuated::<Meta, Token![,]>::parse_terminated;

        // For now, we only support a single key-value pair in the list so we can just return an error if anything
        // else is found. This makes for less code to change if we add more config.
        #[allow(clippy::never_loop)]
        for meta in parser.parse2(meta_list.tokens.clone())? {
            if let Meta::NameValue(ref nv) = meta
                && nv.path.is_ident("path")
                && let syn::Expr::Path(path) = &nv.value
            {
                return Ok(quote!(#path));
            }
            return Err(syn::Error::new_spanned(meta, "Expected `path = ...`"));
        }
        Err(syn::Error::new_spanned(meta_list, "Expected `entry_point()` to not be empty"))
    }
}

impl TryFrom<ItemStruct> for Component {
    type Error = syn::Error;
    fn try_from(mut item: ItemStruct) -> syn::Result<Self> {
        let config = Self::parse_attr(&mut item.attrs)?;
        let x = Component::Struct(item, config);
        Ok(x)
    }
}

impl TryFrom<ItemEnum> for Component {
    type Error = syn::Error;
    fn try_from(mut item: ItemEnum) -> syn::Result<Self> {
        let config = Self::parse_attr(&mut item.attrs)?;
        Ok(Component::Enum(item, config))
    }
}

impl Parse for Component {
    fn parse(stream: syn::parse::ParseStream) -> syn::Result<Self> {
        if stream.fork().parse::<ItemStruct>().is_ok() {
            Component::try_from(stream.parse::<ItemStruct>()?)
        } else if stream.fork().parse::<ItemEnum>().is_ok() {
            Component::try_from(stream.parse::<ItemEnum>()?)
        } else {
            Err(syn::Error::new(stream.span(), "Union types are not currently supported."))
        }
    }
}

/// The testable version of the `component` macro that uses proc_macro2::Tokenstreams.
pub(crate) fn component2(item: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let component = match parse2::<Component>(item) {
        Ok(component) => component,
        Err(e) => return e.to_compile_error(),
    };

    let AttrConfig { entry_point, .. } = component.config();

    let lhs = component.lhs_generics();
    let rhs = component.rhs_generics();
    let where_clause = component.generics().where_clause;

    let name = component.ident();
    let alloc_name = format_ident!("__alloc_component_{name}");

    quote! {
        extern crate alloc as #alloc_name;
        impl #lhs patina_sdk::component::params::ComponentInput for #name #rhs #where_clause {}
        impl #lhs patina_sdk::component::IntoComponent<fn(#name #rhs)-> patina_sdk::error::Result<()>> for #name #rhs #where_clause {
            fn into_component(self) -> #alloc_name::boxed::Box<dyn patina_sdk::component::Component> {
                #alloc_name::boxed::Box::new(
                    patina_sdk::component::StructComponent::new(
                        #entry_point,
                        self
                    )
                )
            }
        }

        //#component
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn test_basic_struct() {
        let input = quote! {
            #[entry_point(path = MyStruct::entry)]
            struct MyStruct;
        };

        let expected = quote! {
            extern crate alloc as __alloc_component_MyStruct;
            impl patina_sdk::component::params::ComponentInput for MyStruct {}
            impl patina_sdk::component::IntoComponent<fn(MyStruct)-> patina_sdk::error::Result<()>> for MyStruct {
                fn into_component(self) -> __alloc_component_MyStruct::boxed::Box<dyn patina_sdk::component::Component> {
                    __alloc_component_MyStruct::boxed::Box::new(
                        patina_sdk::component::StructComponent::new(
                            MyStruct::entry,
                            self
                        )
                    )
                }
            }
        };

        assert_eq!(expected.to_string(), component2(input).to_string());
    }

    #[test]
    fn test_basic_enum() {
        let input = quote! {
            enum MyEnum {
                A,
                B,
            }
        };

        let expected = quote! {
            extern crate alloc as __alloc_component_MyEnum;
            impl patina_sdk::component::params::ComponentInput for MyEnum {}
            impl patina_sdk::component::IntoComponent<fn(MyEnum)-> patina_sdk::error::Result<()>> for MyEnum {
                fn into_component(self) -> __alloc_component_MyEnum::boxed::Box<dyn patina_sdk::component::Component> {
                    __alloc_component_MyEnum::boxed::Box::new(
                        patina_sdk::component::StructComponent::new(
                            Self::entry_point,
                            self
                        )
                    )
                }
            }
        };

        assert_eq!(expected.to_string(), component2(input).to_string());
    }

    #[test]
    fn test_entry_point_not_a_path() {
        let input = quote! {
            #[entry_point(path = "MyStruct::entry")]
            struct MyStruct;
        };

        let expected = quote! {
            :: core :: compile_error ! { "Expected `path = ...`" }
        };

        assert_eq!(component2(input).to_string(), expected.to_string());
    }

    #[test]
    fn test_other_attributes_are_ignored() {
        let input = quote! {
            #[entry_point(path = MyStruct::entry)]
            #[other = "value"]
            struct MyStruct;
        };

        let expected = quote! {
            extern crate alloc as __alloc_component_MyStruct;
            impl patina_sdk::component::params::ComponentInput for MyStruct {}
            impl patina_sdk::component::IntoComponent<fn(MyStruct)-> patina_sdk::error::Result<()>> for MyStruct {
                fn into_component(self) -> __alloc_component_MyStruct::boxed::Box<dyn patina_sdk::component::Component> {
                    __alloc_component_MyStruct::boxed::Box::new(
                        patina_sdk::component::StructComponent::new(
                            MyStruct::entry,
                            self
                        )
                    )
                }
            }
        };

        assert_eq!(expected.to_string(), component2(input).to_string());
    }

    #[test]
    fn test_basic_generic_struct() {
        let input = quote! {
            struct MyStruct<T>(T);
        };

        let expected = quote! {
            extern crate alloc as __alloc_component_MyStruct;
            impl<T> patina_sdk::component::params::ComponentInput for MyStruct<T> {}
            impl<T> patina_sdk::component::IntoComponent<fn(MyStruct<T>)-> patina_sdk::error::Result<()>> for MyStruct<T> {
                fn into_component(self) -> __alloc_component_MyStruct::boxed::Box<dyn patina_sdk::component::Component> {
                    __alloc_component_MyStruct::boxed::Box::new(
                        patina_sdk::component::StructComponent::new(
                            Self::entry_point,
                            self
                        )
                    )
                }
            }
        };

        assert_eq!(expected.to_string(), component2(input).to_string());
    }

    #[test]
    fn test_basic_generic_enum() {
        let input = quote! {
            enum MyEnum<T> {
                A(T),
                B,
            }
        };

        let expected = quote! {
            extern crate alloc as __alloc_component_MyEnum;
            impl<T> patina_sdk::component::params::ComponentInput for MyEnum<T> {}
            impl<T> patina_sdk::component::IntoComponent<fn(MyEnum<T>) -> patina_sdk::error::Result<()>> for MyEnum<T> {
                fn into_component(self) -> __alloc_component_MyEnum::boxed::Box<dyn patina_sdk::component::Component> {
                    __alloc_component_MyEnum::boxed::Box::new(
                        patina_sdk::component::StructComponent::new(
                            Self::entry_point,
                            self
                        )
                    )
                }
            }
        };

        assert_eq!(expected.to_string(), component2(input).to_string());
    }

    #[test]
    fn test_generic_with_where_clause() {
        let input = quote! {
            struct MyStruct<T>
            where T: Debug
            {
                x: T,
            }
        };

        let expected = quote! {
            extern crate alloc as __alloc_component_MyStruct;
            impl<T> patina_sdk::component::params::ComponentInput for MyStruct<T> where T: Debug {}
            impl<T> patina_sdk::component::IntoComponent<fn(MyStruct<T>)-> patina_sdk::error::Result<()>> for MyStruct<T> where T: Debug {
                fn into_component(self) -> __alloc_component_MyStruct::boxed::Box<dyn patina_sdk::component::Component> {
                    __alloc_component_MyStruct::boxed::Box::new(
                        patina_sdk::component::StructComponent::new(
                            Self::entry_point,
                            self
                        )
                    )
                }
            }
        };

        assert_eq!(expected.to_string(), component2(input).to_string());
    }

    #[test]
    fn test_generic_where_restriction_with_generic() {
        let input = quote! {
            struct MyStruct<T: Debug>(T);
        };

        let expected = quote! {
            extern crate alloc as __alloc_component_MyStruct;
            impl<T: Debug> patina_sdk::component::params::ComponentInput for MyStruct<T> {}
            impl<T: Debug> patina_sdk::component::IntoComponent<fn(MyStruct<T>)-> patina_sdk::error::Result<()>> for MyStruct<T> {
                fn into_component(self) -> __alloc_component_MyStruct::boxed::Box<dyn patina_sdk::component::Component> {
                    __alloc_component_MyStruct::boxed::Box::new(
                        patina_sdk::component::StructComponent::new(
                            Self::entry_point,
                            self
                        )
                    )
                }
            }
        };

        assert_eq!(expected.to_string(), component2(input).to_string());
    }

    #[test]
    fn test_parse_attr_not_list() {
        let input = quote! {
            #[entry_point]
            struct MyStruct;
        };

        let expected = quote! {
            :: core :: compile_error ! { "Expected `#[entry_point(...)]`" }
        };

        assert_eq!(component2(input).to_string(), expected.to_string());
    }

    #[test]
    fn test_parse_attr_empty_list() {
        let input = quote! {
            #[entry_point()]
            struct MyStruct;
        };

        let expected = quote! {
            :: core :: compile_error ! { "Expected `entry_point()` to not be empty" }
        };

        assert_eq!(component2(input).to_string(), expected.to_string());
    }

    #[test]
    fn test_parse_union_not_supported() {
        let input = quote! {
            union MyUnion {
                a: u32,
                b: u64,
            }
        };

        let expected = quote! {
            :: core :: compile_error ! { "Union types are not currently supported." }
        };

        assert_eq!(component2(input).to_string(), expected.to_string());
    }
}
