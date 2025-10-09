use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::{Attribute, Generics, ItemEnum, ItemStruct, Meta, parse::Parse, spanned::Spanned};

struct AttrConfig {
    hob_guid: TokenStream,
}

struct HobConfig {
    item: ItemStruct,
    config: AttrConfig,
}

impl HobConfig {
    fn parse_attr(attrs: &mut Vec<Attribute>) -> syn::Result<AttrConfig> {
        let mut config = AttrConfig { hob_guid: TokenStream::new() };
        for attr in attrs {
            if attr.path().is_ident("hob") {
                config.hob_guid = Self::parse_hob_attr(attr)?;
            }
        }

        Ok(config)
    }

    fn parse_hob_attr(attr: &Attribute) -> syn::Result<TokenStream> {
        let Meta::NameValue(nv) = &attr.meta else {
            return Err(syn::Error::new(attr.span(), "Expected #[hob = \"GUID\"]"));
        };

        let id = match uuid::Uuid::parse_str(&nv.value.to_token_stream().to_string().replace("\"", "")) {
            Err(_) => return Err(syn::Error::new(attr.span(), "Invalid GUID format")),
            Ok(id) => id,
        };

        let fields = id.as_fields();
        let node: &[u8; 6] =
            &fields.3[2..].try_into().map_err(|_| syn::Error::new(attr.span(), "Invalid GUID format"))?;
        let (a, b, c) = (fields.0, fields.1, fields.2);
        let (d0, d1) = (fields.3[0], fields.3[1]);
        let [d2, d3, d4, d5, d6, d7] = *node;

        Ok(quote! {
            patina::OwnedGuid::from_fields(#a, #b, #c, #d0, #d1, [#d2, #d3, #d4, #d5, #d6, #d7])
        })
    }

    /// Returns the name [Ident](syn::Ident) of the struct
    fn ident(&self) -> &syn::Ident {
        &self.item.ident
    }

    /// Returns the parsed attribute configuration.
    fn config(&self) -> &AttrConfig {
        &self.config
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

impl TryFrom<ItemStruct> for HobConfig {
    type Error = syn::Error;

    fn try_from(mut item: ItemStruct) -> syn::Result<Self> {
        let config = Self::parse_attr(&mut item.attrs)?;
        if config.hob_guid.is_empty() {
            return Err(syn::Error::new(
                item.span(),
                "Missing required attribute `#[hob = \"GUID\"]` for HobConfig derive macro.",
            ));
        }
        Ok(HobConfig { item, config })
    }
}

impl Parse for HobConfig {
    fn parse(stream: syn::parse::ParseStream) -> syn::Result<Self> {
        if stream.fork().parse::<ItemStruct>().is_ok() {
            Ok(stream.parse::<ItemStruct>().and_then(HobConfig::try_from)?)
        } else if stream.fork().parse::<ItemEnum>().is_ok() {
            Err(syn::Error::new(stream.span(), "Enum types are not currently supported."))
        } else {
            Err(syn::Error::new(stream.span(), "Union types are not currently supported."))
        }
    }
}

pub fn hob_config2(item: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let config = match syn::parse2::<HobConfig>(item) {
        Ok(config) => config,
        Err(err) => return err.to_compile_error(),
    };

    let name = config.ident();
    let lhs = config.lhs_generics();
    let rhs = config.rhs_generics();
    let where_clause = config.generics().where_clause;
    let hob_guid = &config.config().hob_guid;

    quote! {
        impl #lhs patina::component::hob::FromHob for #name #rhs #where_clause {
            const HOB_GUID: patina::OwnedGuid = #hob_guid;

            fn parse(bytes: &[u8]) -> Self {
                assert!(
                    bytes.len() >= core::mem::size_of::<Self>(),
                    "Guided Hob [{:#?}] parse failed. Buffer to small for type {}", Self::HOB_GUID, core::any::type_name::<Self>()
                );
                unsafe { *(bytes.as_ptr() as *const Self) }
            }
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use proc_macro2::TokenStream;
    use quote::quote;
    extern crate alloc;
    use alloc::format;

    #[test]
    fn test_config_basic() {
        let input: TokenStream = quote! {
            #[derive(HobConfig)]
            #[hob = "8be4df61-93ca-11d2-aa0d-00e098032b8c"]
            struct MyStruct(u32);
        };

        const TEST_HOB_GUID: patina::OwnedGuid = patina::OwnedGuid::from_fields(
            2347032417u32,
            37834u16,
            4562u16,
            170u8,
            13u8,
            [0u8, 224u8, 152u8, 3u8, 43u8, 140u8],
        );
        let expected = quote! {
            impl patina::component::hob::FromHob for MyStruct {
                const HOB_GUID: patina::OwnedGuid = patina::OwnedGuid::from_fields(2347032417u32, 37834u16, 4562u16, 170u8, 13u8, [0u8, 224u8, 152u8, 3u8, 43u8, 140u8]);
                fn parse(bytes: &[u8]) -> Self {
                    assert!(
                        bytes.len() >= core::mem::size_of::<Self>(),
                        "Guided Hob [{:#?}] parse failed. Buffer to small for type {}", Self::HOB_GUID, core::any::type_name::<Self>()
                    );
                    unsafe { *(bytes.as_ptr() as *const Self) }
                }
            }
        };

        let output = hob_config2(input);
        assert_eq!(output.to_string(), expected.to_string());

        let (f0, f1, f2, f3, f4, &[f5, f6, f7, f8, f9, f10]) = TEST_HOB_GUID.as_fields();
        let name =
            format!("{f0:08x}-{f1:04x}-{f2:04x}-{f3:02x}{f4:02x}-{f5:02x}{f6:02x}{f7:02x}{f8:02x}{f9:02x}{f10:02x}");
        assert_eq!(name, "8be4df61-93ca-11d2-aa0d-00e098032b8c");
    }

    #[test]
    fn test_config_basic_pcd_db_hob_guid() {
        let input: TokenStream = quote! {
            #[derive(HobConfig)]
            #[hob = "ea296d92-0b69-423c-8c28-33b4e0a91268"]
            struct MyStruct(u32);
        };

        const TEST_HOB_GUID: patina::OwnedGuid = patina::OwnedGuid::from_fields(
            0xea296d92u32,
            0x0b69u16,
            0x423cu16,
            0x8cu8,
            0x28u8,
            [0x33u8, 0xb4u8, 0xe0u8, 0xa9u8, 0x12u8, 0x68u8],
        );
        let expected = quote! {
            impl patina::component::hob::FromHob for MyStruct {

                const HOB_GUID: patina::OwnedGuid = patina::OwnedGuid::from_fields(3928583570u32, 2921u16, 16956u16, 140u8, 40u8, [51u8, 180u8, 224u8, 169u8, 18u8, 104u8]);
                fn parse(bytes: &[u8]) -> Self {
                    assert!(
                        bytes.len() >= core::mem::size_of::<Self>(),
                        "Guided Hob [{:#?}] parse failed. Buffer to small for type {}", Self::HOB_GUID, core::any::type_name::<Self>()
                    );
                    unsafe { *(bytes.as_ptr() as *const Self) }
                }
            }
        };

        let output = hob_config2(input);
        assert_eq!(output.to_string(), expected.to_string());

        // Test that the hex string provided in the attribute matches the well known PCD Database HOB GUID
        let (f0, f1, f2, f3, f4, &[f5, f6, f7, f8, f9, f10]) = TEST_HOB_GUID.as_fields();
        let name = alloc::format!(
            "{f0:08x}-{f1:04x}-{f2:04x}-{f3:02x}{f4:02x}-{f5:02x}{f6:02x}{f7:02x}{f8:02x}{f9:02x}{f10:02x}"
        );
        assert_eq!(name, "ea296d92-0b69-423c-8c28-33b4e0a91268");
    }

    #[test]
    fn test_config_with_generics() {
        let input: TokenStream = quote! {
            #[derive(HobConfig)]
            #[hob = "8be4df61-93ca-11d2-aa0d-00e098032b8c"]
            struct MyStruct<T>(u32, T);
        };
        let expected = quote! {
            impl<T> patina::component::hob::FromHob for MyStruct<T> {
                const HOB_GUID: patina::OwnedGuid = patina::OwnedGuid::from_fields(2347032417u32, 37834u16, 4562u16, 170u8, 13u8, [0u8, 224u8, 152u8, 3u8, 43u8, 140u8]);
                fn parse(bytes: &[u8]) -> Self {
                    assert!(
                        bytes.len() >= core::mem::size_of::<Self>(),
                        "Guided Hob [{:#?}] parse failed. Buffer to small for type {}", Self::HOB_GUID, core::any::type_name::<Self>()
                    );
                    unsafe { *(bytes.as_ptr() as *const Self) }
                }
            }
        };

        let output = hob_config2(input);
        assert_eq!(output.to_string(), expected.to_string());
    }

    #[test]
    fn test_config_with_missing_hob() {
        let input: TokenStream = quote! {
            #[derive(HobConfig)]
            struct MyStruct(u32);
        };
        let expected = quote! {
            :: core :: compile_error ! { "Missing required attribute `#[hob = \"GUID\"]` for HobConfig derive macro." }
        };

        let output = hob_config2(input);
        assert_eq!(output.to_string(), expected.to_string());
    }

    #[test]
    fn test_config_with_bad_hob() {
        let input: TokenStream = quote! {
            #[derive(HobConfig)]
            #[hob = "invalid-guid"]
            struct MyStruct(u32);
        };
        let expected = quote! {
            :: core :: compile_error ! { "Invalid GUID format" }
        };

        let output = hob_config2(input);
        assert_eq!(output.to_string(), expected.to_string());
    }

    #[test]
    fn test_bad_hob_attr_usage() {
        let input: TokenStream = quote! {
            #[derive(HobConfig)]
            #[hob("8be4df61-93ca-11d2-aa0d-00e098032b8c")]
            struct MyStruct(u32);
        };
        let expected = quote! {
            :: core :: compile_error ! { "Expected #[hob = \"GUID\"]" }
        };

        let output = hob_config2(input);
        assert_eq!(output.to_string(), expected.to_string());
    }

    #[test]
    fn test_on_enum_type() {
        let input: TokenStream = quote! {
            #[derive(HobConfig)]
            #[hob = "8be4df61-93ca-11d2-aa0d-00e098032b8c"]
            enum MyEnum {
                Variant1,
                Variant2,
            }
        };
        let expected = quote! {
            :: core :: compile_error ! { "Enum types are not currently supported." }
        };

        let output = hob_config2(input);
        assert_eq!(output.to_string(), expected.to_string());
    }

    #[test]
    fn test_on_union_type() {
        let input: TokenStream = quote! {
            #[derive(HobConfig)]
            #[hob = "8be4df61-93ca-11d2-aa0d-00e098032b8c"]
            union MyUnion {
                field1: u32,
                field2: u32,
            }
        };
        let expected = quote! {
            :: core :: compile_error ! { "Union types are not currently supported." }
        };

        let output = hob_config2(input);
        assert_eq!(output.to_string(), expected.to_string());
    }
}
