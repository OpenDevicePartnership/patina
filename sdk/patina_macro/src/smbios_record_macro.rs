//! Derive macro for SMBIOS record structures.
//!
//! This macro generates a complete `SmbiosRecordStructure` trait implementation,
//! eliminating the need for manual boilerplate code.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use patina_macro::SmbiosRecord;
//!
//! #[derive(SmbiosRecord)]
//! #[smbios(record_type = 0x80)]  // Vendor-specific type
//! pub struct VendorOemRecord {
//!     pub header: SmbiosTableHeader,
//!     pub oem_field: u32,
//!     #[string_pool]
//!     pub string_pool: Vec<String>,
//! }
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, ItemStruct, Lit, Meta, parse::Parse, spanned::Spanned};

struct SmbiosRecord {
    item: ItemStruct,
    string_pool_field: Option<syn::Ident>,
    record_type: Option<u8>,
}

impl Parse for SmbiosRecord {
    fn parse(stream: syn::parse::ParseStream) -> syn::Result<Self> {
        let item: ItemStruct = stream.parse()?;

        // Find the #[smbios(record_type = ...)] attribute
        let mut record_type = None;
        for attr in &item.attrs {
            if attr.path().is_ident("smbios")
                && let Meta::List(meta_list) = &attr.meta
            {
                // Parse record_type = value
                let nested: syn::punctuated::Punctuated<Meta, syn::Token![,]> =
                    meta_list.parse_args_with(syn::punctuated::Punctuated::parse_terminated)?;

                for meta in nested {
                    if let Meta::NameValue(nv) = meta
                        && nv.path.is_ident("record_type")
                        && let syn::Expr::Lit(expr_lit) = &nv.value
                        && let Lit::Int(lit_int) = &expr_lit.lit
                    {
                        record_type = Some(lit_int.base10_parse()?);
                    }
                }
            }
        }

        // Find the field marked with #[string_pool]
        let mut string_pool_field = None;
        let mut string_pool_count = 0;

        if let Fields::Named(fields) = &item.fields {
            for field in fields.named.iter() {
                let has_string_pool = field.attrs.iter().any(|attr| attr.path().is_ident("string_pool"));

                if has_string_pool {
                    string_pool_count += 1;
                    string_pool_field = field.ident.clone();
                }
            }
        }

        if string_pool_count > 1 {
            return Err(syn::Error::new(
                item.span(),
                "Only one field can be marked with #[string_pool] - SMBIOS records have a single string pool",
            ));
        }

        Ok(SmbiosRecord { item, string_pool_field, record_type })
    }
}

/// Generate the SmbiosRecordStructure trait implementation
///
/// This macro generates a complete SmbiosRecordStructure implementation including:
/// - RECORD_TYPE constant
/// - to_bytes() serialization
/// - validate() string length checking
/// - string_pool() and string_pool_mut() accessors
pub(crate) fn smbios_record_derive(item: TokenStream) -> TokenStream {
    let record = match syn::parse2::<SmbiosRecord>(item) {
        Ok(r) => r,
        Err(e) => return e.to_compile_error(),
    };

    let name = &record.item.ident;
    let (impl_generics, ty_generics, where_clause) = record.item.generics.split_for_impl();

    // Detect if we're inside patina_smbios crate or using it externally
    // Use crate:: for internal, ::patina_smbios:: for external
    let crate_path = quote! { ::patina_smbios };

    // Get the record type - required for trait implementation
    let record_type = match record.record_type {
        Some(rt) => rt,
        None => {
            return syn::Error::new(
                record.item.span(),
                "Missing #[smbios(record_type = ...)] attribute. Example: #[smbios(record_type = 0x80)]",
            )
            .to_compile_error();
        }
    };

    // Collect all non-string-pool fields for serialization
    let mut field_serializations = Vec::new();
    let mut structured_size_calc = quote! { core::mem::size_of::<SmbiosTableHeader>() };

    if let Fields::Named(fields) = &record.item.fields {
        for field in fields.named.iter() {
            let field_name = field.ident.as_ref().unwrap();
            let field_ty = &field.ty;

            // Skip the string_pool field and header field
            let is_string_pool = field.attrs.iter().any(|attr| attr.path().is_ident("string_pool"));
            if is_string_pool || field_name == "header" {
                continue;
            }

            // Validate field type - must be a primitive integer type or byte array
            let is_valid_type = match field_ty {
                syn::Type::Path(type_path) => {
                    let path = &type_path.path;
                    path.segments.len() == 1 && {
                        let segment = &path.segments[0];
                        matches!(
                            segment.ident.to_string().as_str(),
                            "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64"
                        )
                    }
                }
                syn::Type::Array(type_array) => {
                    matches!(&*type_array.elem,
                        syn::Type::Path(elem_path)
                            if elem_path.path.segments.len() == 1 &&
                               elem_path.path.segments[0].ident == "u8")
                }
                _ => false,
            };

            if !is_valid_type {
                return syn::Error::new(
                    field.span(),
                    format!(
                        "Field '{}' has unsupported type. SMBIOS record fields must be primitive integer types (u8, u16, u32, u64, i8, i16, i32, i64) or byte arrays ([u8; N]). Found: {}",
                        field_name,
                        quote!(#field_ty)
                    ),
                )
                .to_compile_error();
            }

            // Add field size to structured size calculation
            structured_size_calc = quote! {
                #structured_size_calc + core::mem::size_of::<#field_ty>()
            };

            // Generate serialization for this field based on type
            // Special case for byte arrays (like UUID) - copy directly without to_le_bytes()
            let serialization = match field_ty {
                syn::Type::Array(_) => {
                    quote! {
                        bytes.extend_from_slice(&self.#field_name);
                    }
                }
                _ => {
                    quote! {
                        bytes.extend_from_slice(&self.#field_name.to_le_bytes());
                    }
                }
            };

            field_serializations.push(serialization);
        }
    }

    // String pool methods
    let (string_pool_impl, validate_impl) = if let Some(pool_field) = &record.string_pool_field {
        (
            quote! {
                fn string_pool(&self) -> &[alloc::string::String] {
                    &self.#pool_field
                }

                fn string_pool_mut(&mut self) -> &mut alloc::vec::Vec<alloc::string::String> {
                    &mut self.#pool_field
                }
            },
            quote! {
                fn validate(&self) -> core::result::Result<(), #crate_path::error::SmbiosError> {
                    for string in &self.#pool_field {
                        if string.len() > #crate_path::service::SMBIOS_STRING_MAX_LENGTH {
                            return Err(#crate_path::error::SmbiosError::StringTooLong);
                        }
                    }
                    Ok(())
                }
            },
        )
    } else {
        (
            quote! {
                fn string_pool(&self) -> &[alloc::string::String] {
                    &[]
                }

                fn string_pool_mut(&mut self) -> &mut alloc::vec::Vec<alloc::string::String> {
                    // Return a dummy mutable reference - this should never be used
                    static mut EMPTY: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
                    unsafe { &mut EMPTY }
                }
            },
            quote! {
                fn validate(&self) -> core::result::Result<(), #crate_path::error::SmbiosError> {
                    Ok(())
                }
            },
        )
    };

    // Generate serialize_strings helper
    let serialize_strings = if let Some(pool_field) = record.string_pool_field.as_ref() {
        quote! {
            let mut string_bytes = alloc::vec::Vec::new();
            if self.#pool_field.is_empty() {
                string_bytes.extend_from_slice(&[0, 0]);
            } else {
                for string in &self.#pool_field {
                    if !string.is_empty() {
                        string_bytes.extend_from_slice(string.as_bytes());
                    }
                    string_bytes.push(0);
                }
                string_bytes.push(0);
            }
            bytes.extend_from_slice(&string_bytes);
        }
    } else {
        quote! {
            bytes.extend_from_slice(&[0, 0]);
        }
    };

    quote! {
        impl #impl_generics #crate_path::smbios_record::SmbiosRecordStructure for #name #ty_generics #where_clause {
            const RECORD_TYPE: u8 = #record_type;

            fn to_bytes(&self) -> alloc::vec::Vec<u8> {
                let mut bytes = alloc::vec::Vec::new();

                // Calculate structured data size (header + all fields except string_pool)
                let structured_size = #structured_size_calc;

                // Serialize header (type, length, handle)
                bytes.push(Self::RECORD_TYPE);
                bytes.push(structured_size as u8);
                bytes.extend_from_slice(&self.header.handle.to_le_bytes());

                // Serialize all other fields
                #(#field_serializations)*

                // Serialize string pool
                #serialize_strings

                bytes
            }

            #validate_impl

            #string_pool_impl
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn test_single_string_pool_only() {
        let input = quote! {
            #[derive(SmbiosRecord)]
            #[repr(C, packed)]
            pub struct TestRecord {
                pub header: SmbiosTableHeader,
                #[string_pool]
                pub strings1: Vec<String>,
                #[string_pool]
                pub strings2: Vec<String>,
            }
        };

        let output = smbios_record_derive(input);
        let output_str = output.to_string();
        // Should error about multiple string pools
        assert!(output_str.contains("compile_error") || output_str.contains("Only one"));
    }

    #[test]
    fn test_unsupported_field_type_string() {
        let input = quote! {
            #[derive(SmbiosRecord)]
            #[smbios(record_type = 0x80)]
            pub struct TestRecord {
                pub header: SmbiosTableHeader,
                pub invalid_field: String,
                #[string_pool]
                pub strings: Vec<String>,
            }
        };

        let output = smbios_record_derive(input);
        let output_str = output.to_string();
        // Should error about unsupported type for invalid_field
        assert!(output_str.contains("compile_error"));
        assert!(output_str.contains("invalid_field"));
        assert!(output_str.contains("unsupported type"));
    }

    #[test]
    fn test_unsupported_field_type_vec() {
        let input = quote! {
            #[derive(SmbiosRecord)]
            #[smbios(record_type = 0x80)]
            pub struct TestRecord {
                pub header: SmbiosTableHeader,
                pub data: Vec<u8>,
                #[string_pool]
                pub strings: Vec<String>,
            }
        };

        let output = smbios_record_derive(input);
        let output_str = output.to_string();
        // Should error about unsupported Vec type
        assert!(output_str.contains("compile_error"));
        assert!(output_str.contains("data"));
        assert!(output_str.contains("unsupported type"));
    }

    #[test]
    fn test_unsupported_field_type_custom_struct() {
        let input = quote! {
            #[derive(SmbiosRecord)]
            #[smbios(record_type = 0x80)]
            pub struct TestRecord {
                pub header: SmbiosTableHeader,
                pub custom: CustomStruct,
                #[string_pool]
                pub strings: Vec<String>,
            }
        };

        let output = smbios_record_derive(input);
        let output_str = output.to_string();
        // Should error about unsupported custom struct type
        assert!(output_str.contains("compile_error"));
        assert!(output_str.contains("custom"));
        assert!(output_str.contains("unsupported type"));
    }

    #[test]
    fn test_unsupported_field_type_option() {
        let input = quote! {
            #[derive(SmbiosRecord)]
            #[smbios(record_type = 0x80)]
            pub struct TestRecord {
                pub header: SmbiosTableHeader,
                pub optional: Option<u32>,
                #[string_pool]
                pub strings: Vec<String>,
            }
        };

        let output = smbios_record_derive(input);
        let output_str = output.to_string();
        // Should error about unsupported Option type
        assert!(output_str.contains("compile_error"));
        assert!(output_str.contains("optional"));
        assert!(output_str.contains("unsupported type"));
    }

    #[test]
    fn test_valid_primitive_types() {
        let input = quote! {
            #[derive(SmbiosRecord)]
            #[smbios(record_type = 0x80)]
            pub struct TestRecord {
                pub header: SmbiosTableHeader,
                pub byte: u8,
                pub word: u16,
                pub dword: u32,
                pub qword: u64,
                pub signed_byte: i8,
                pub signed_word: i16,
                pub signed_dword: i32,
                pub signed_qword: i64,
                #[string_pool]
                pub strings: Vec<String>,
            }
        };

        let output = smbios_record_derive(input);
        let output_str = output.to_string();
        // Should NOT contain compile_error - all types are valid
        assert!(!output_str.contains("compile_error"));
        assert!(output_str.contains("impl"));
        assert!(output_str.contains("SmbiosRecordStructure"));
    }

    #[test]
    fn test_valid_byte_array() {
        let input = quote! {
            #[derive(SmbiosRecord)]
            #[smbios(record_type = 0x80)]
            pub struct TestRecord {
                pub header: SmbiosTableHeader,
                pub uuid: [u8; 16],
                #[string_pool]
                pub strings: Vec<String>,
            }
        };

        let output = smbios_record_derive(input);
        let output_str = output.to_string();
        // Should NOT contain compile_error - byte array is valid
        assert!(!output_str.contains("compile_error"));
        assert!(output_str.contains("impl"));
        assert!(output_str.contains("SmbiosRecordStructure"));
    }

    #[test]
    fn test_invalid_array_type() {
        let input = quote! {
            #[derive(SmbiosRecord)]
            #[smbios(record_type = 0x80)]
            pub struct TestRecord {
                pub header: SmbiosTableHeader,
                pub invalid_array: [u32; 4],
                #[string_pool]
                pub strings: Vec<String>,
            }
        };

        let output = smbios_record_derive(input);
        let output_str = output.to_string();
        // Should error - only [u8; N] arrays are allowed
        assert!(output_str.contains("compile_error"));
        assert!(output_str.contains("invalid_array"));
        assert!(output_str.contains("unsupported type"));
    }

    #[test]
    fn test_missing_record_type_attribute() {
        let input = quote! {
            #[derive(SmbiosRecord)]
            pub struct TestRecord {
                pub header: SmbiosTableHeader,
                pub field: u8,
                #[string_pool]
                pub strings: Vec<String>,
            }
        };

        let output = smbios_record_derive(input);
        let output_str = output.to_string();
        // Should error about missing record_type
        assert!(output_str.contains("compile_error"));
        assert!(output_str.contains("Missing") || output_str.contains("record_type"));
    }
}
