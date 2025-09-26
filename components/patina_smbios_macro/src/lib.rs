use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    parse_macro_input, Attribute, Data, DeriveInput, Fields, Ident, Type, Meta, MetaNameValue, Expr, Lit
};

/// Derive macro for SMBIOS record structures
/// 
/// Usage:
/// ```rust
/// #[derive(SmbiosRecord)]
/// #[smbios(record_type = 2)]
/// pub struct Type2BaseboardInformation {
///     pub manufacturer: u8,
///     pub product: u8,
///     // ... other fields
///     pub string_pool: Vec<String>,  // or pub strings: Vec<String>
/// }
/// ```
#[proc_macro_derive(SmbiosRecord, attributes(smbios))]
pub fn derive_smbios_record(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    
    let name = &input.ident;
    
    // Parse the #[smbios(record_type = N)] attribute
    let record_type = extract_record_type(&input.attrs)
        .expect("Missing #[smbios(record_type = N)] attribute");
    
    // Parse the struct fields
    let fields = match &input.data {
        Data::Struct(data_struct) => match &data_struct.fields {
            Fields::Named(fields_named) => &fields_named.named,
            _ => panic!("SmbiosRecord derive only supports named fields"),
        },
        _ => panic!("SmbiosRecord derive only supports structs"),
    };
    
    // Generate field layout implementation using the existing macro
    let field_layout_impl = generate_field_layout_impl_macro(name, fields);
    
    // Generate SmbiosRecordStructure implementation
    let record_structure_impl = generate_record_structure_impl(name, record_type, fields);
    
    let expanded = quote! {
        #field_layout_impl
        #record_structure_impl
    };
    
    TokenStream::from(expanded)
}

fn extract_record_type(attrs: &[Attribute]) -> Option<u8> {
    for attr in attrs {
        if attr.path().is_ident("smbios") {
            if let Meta::List(meta_list) = &attr.meta {
                // Parse the tokens manually
                let tokens = &meta_list.tokens;
                let parsed: syn::Result<MetaNameValue> = syn::parse2(tokens.clone());
                if let Ok(name_value) = parsed {
                    if name_value.path.is_ident("record_type") {
                        if let Expr::Lit(expr_lit) = &name_value.value {
                            if let Lit::Int(lit_int) = &expr_lit.lit {
                                return lit_int.base10_parse().ok();
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn generate_field_layout_impl_macro(name: &Ident, fields: &syn::punctuated::Punctuated<syn::Field, syn::token::Comma>) -> TokenStream2 {
    let mut field_specs = Vec::new();
    
    for field in fields {
        if let Some(field_name) = &field.ident {
            // Skip header and string_pool fields
            if field_name == "header" || field_name == "string_pool" || field_name == "strings" {
                continue;
            }
            
            let field_type_ident = get_field_type_ident(&field.ty);
            field_specs.push(quote! { #field_name: #field_type_ident });
        }
    }
    
    quote! {
        impl_smbios_field_layout!(#name,
            #(#field_specs),*
        );
    }
}

fn get_field_type_ident(ty: &Type) -> proc_macro2::Ident {
    match ty {
        Type::Path(type_path) => {
            let last_segment = type_path.path.segments.last().unwrap();
            let type_name = &last_segment.ident;
            
            match type_name.to_string().as_str() {
                "u8" => proc_macro2::Ident::new("u8", proc_macro2::Span::call_site()),
                "u16" => proc_macro2::Ident::new("u16", proc_macro2::Span::call_site()),
                "u32" => proc_macro2::Ident::new("u32", proc_macro2::Span::call_site()),
                "u64" => proc_macro2::Ident::new("u64", proc_macro2::Span::call_site()),
                _ => proc_macro2::Ident::new("u8", proc_macro2::Span::call_site()), // Default to u8
            }
        }
        _ => proc_macro2::Ident::new("u8", proc_macro2::Span::call_site()), // Default to u8
    }
}

fn generate_record_structure_impl(name: &Ident, record_type: u8, fields: &syn::punctuated::Punctuated<syn::Field, syn::token::Comma>) -> TokenStream2 {
    // Find the string pool field name
    let string_pool_field = fields.iter()
        .find(|f| {
            if let Some(field_name) = &f.ident {
                field_name == "string_pool" || field_name == "strings"
            } else {
                false
            }
        })
        .and_then(|f| f.ident.as_ref());
    
    let string_pool_field = string_pool_field.expect("No string_pool or strings field found");
    
    quote! {
        impl SmbiosRecordStructure for #name {
            const RECORD_TYPE: u8 = #record_type;
            
            fn validate(&self) -> Result<(), crate::smbios_derive::SmbiosError> {
                // Basic validation for strings
                for string in &self.#string_pool_field {
                    if string.len() > crate::smbios_derive::SMBIOS_STRING_MAX_LENGTH {
                        return Err(crate::smbios_derive::SmbiosError::StringTooLong);
                    }
                }
                Ok(())
            }
            
            fn string_pool(&self) -> &[alloc::string::String] {
                &self.#string_pool_field
            }
            
            fn string_pool_mut(&mut self) -> &mut alloc::vec::Vec<alloc::string::String> {
                &mut self.#string_pool_field
            }
        }
    }
}