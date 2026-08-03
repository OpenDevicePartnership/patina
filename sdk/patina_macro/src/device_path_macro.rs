//! Procedural macro front end for compile-time UEFI device paths.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use std::collections::HashSet;

use proc_macro2::{Literal, TokenStream};
use quote::quote;
use syn::{
    Ident, LitStr, Token, braced, bracketed,
    parse::{Parse, ParseStream},
};

use crate::{
    device_path_encoder::{efi_guid_bytes, encode_device_path, encode_device_path_with_vendor_defined},
    device_path_nodes::{
        VendorDefinedField, VendorDefinedFieldType, VendorDefinedSchema, VendorDefinedType, is_builtin_node_name,
    },
    device_path_parser::DevicePathError,
};

mod keyword {
    syn::custom_keyword!(vendor);
    syn::custom_keyword!(defined);
    syn::custom_keyword!(guid);
    syn::custom_keyword!(fields);
    syn::custom_keyword!(hardware);
    syn::custom_keyword!(messaging);
    syn::custom_keyword!(media);
}

struct DevicePathInput {
    vendor_defined_schemas: Vec<VendorDefinedSchema>,
    literal: LitStr,
}

impl Parse for DevicePathInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let vendor_defined_schemas =
            if input.peek(keyword::vendor) { parse_vendor_defined_schemas(input)? } else { Vec::new() };
        let literal = input
            .parse::<LitStr>()
            .map_err(|_| syn::Error::new(input.span(), "`devpath!` expects exactly one string literal"))?;
        if !input.is_empty() {
            return Err(syn::Error::new(input.span(), "`devpath!` accepts exactly one string literal"));
        }
        Ok(Self { vendor_defined_schemas, literal })
    }
}

fn parse_vendor_defined_schemas(input: ParseStream<'_>) -> syn::Result<Vec<VendorDefinedSchema>> {
    input.parse::<keyword::vendor>()?;
    input.parse::<Token![-]>()?;
    input.parse::<keyword::defined>()?;
    let content;
    braced!(content in input);
    let schemas = content.parse_terminated(VendorDefinedSchemaInput::parse, Token![,])?;
    input.parse::<Token![;]>()?;

    if schemas.is_empty() {
        return Err(syn::Error::new(content.span(), "`vendor-defined` must declare at least one schema"));
    }

    let mut names = HashSet::new();
    let mut parsed = Vec::with_capacity(schemas.len());
    for schema in schemas {
        if is_builtin_node_name(&schema.schema.name) {
            return Err(syn::Error::new(
                schema.name_span,
                format!("`{}` is a built-in device path node and cannot be redefined", schema.schema.name),
            ));
        }
        if !names.insert(schema.schema.name.clone()) {
            return Err(syn::Error::new(
                schema.name_span,
                format!("vendor-defined schema `{}` was declared more than once", schema.schema.name),
            ));
        }
        parsed.push(schema.schema);
    }
    Ok(parsed)
}

struct VendorDefinedSchemaInput {
    name_span: proc_macro2::Span,
    schema: VendorDefinedSchema,
}

impl Parse for VendorDefinedSchemaInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let name = input.parse::<Ident>()?;
        validate_schema_identifier(&name, "vendor-defined schema name")?;
        let name_span = name.span();
        let content;
        braced!(content in input);

        content.parse::<Token![type]>()?;
        content.parse::<Token![:]>()?;
        let vendor_type = if content.peek(keyword::hardware) {
            content.parse::<keyword::hardware>()?;
            VendorDefinedType::Hardware
        } else if content.peek(keyword::messaging) {
            content.parse::<keyword::messaging>()?;
            VendorDefinedType::Messaging
        } else if content.peek(keyword::media) {
            content.parse::<keyword::media>()?;
            VendorDefinedType::Media
        } else {
            return Err(content.error("vendor-defined `type` must be `hardware`, `messaging`, or `media`"));
        };
        content.parse::<Token![,]>()?;
        content.parse::<keyword::guid>()?;
        content.parse::<Token![:]>()?;
        let guid = content.parse::<LitStr>()?;
        content.parse::<Token![,]>()?;
        content.parse::<keyword::fields>()?;
        content.parse::<Token![:]>()?;
        let fields_content;
        bracketed!(fields_content in content);
        let fields = fields_content.parse_terminated(VendorDefinedFieldInput::parse, Token![,])?;
        if content.peek(Token![,]) {
            content.parse::<Token![,]>()?;
        }
        if !content.is_empty() {
            return Err(content.error("expected only `type`, `guid`, and `fields` in a vendor-defined schema"));
        }

        let guid_bytes = efi_guid_bytes(&guid.value())
            .map_err(|_| syn::Error::new(guid.span(), "vendor-defined `guid` is not a valid GUID"))?;
        let mut field_names = HashSet::new();
        let mut parsed_fields = Vec::with_capacity(fields.len());
        for field in fields {
            if !field_names.insert(field.field.name.clone()) {
                return Err(syn::Error::new(
                    field.name_span,
                    format!("vendor-defined field `{}` was declared more than once", field.field.name),
                ));
            }
            parsed_fields.push(field.field);
        }

        Ok(Self {
            name_span,
            schema: VendorDefinedSchema {
                name: name.to_string(),
                vendor_type,
                guid: guid_bytes,
                fields: parsed_fields,
            },
        })
    }
}

struct VendorDefinedFieldInput {
    name_span: proc_macro2::Span,
    field: VendorDefinedField,
}

impl Parse for VendorDefinedFieldInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let name = input.parse::<Ident>()?;
        validate_schema_identifier(&name, "vendor-defined field name")?;
        let name_span = name.span();
        input.parse::<Token![:]>()?;
        let field_type = input.parse::<Ident>()?;
        let field_type = VendorDefinedFieldType::from_name(&field_type.to_string()).ok_or_else(|| {
            syn::Error::new(
                field_type.span(),
                "unsupported vendor-defined field type; expected `u8`, `u16le`, `u32le`, `u64le`, `guid`, `uuid`, or `bytes`",
            )
        })?;
        Ok(Self { name_span, field: VendorDefinedField { name: name.to_string(), field_type } })
    }
}

fn validate_schema_identifier(identifier: &Ident, description: &str) -> syn::Result<()> {
    let value = identifier.to_string();
    if !value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(syn::Error::new(
            identifier.span(),
            format!("{description} must contain only alphanumeric characters"),
        ));
    }
    Ok(())
}

/// Expand a UEFI text device path into an owned byte-array literal.
pub(crate) fn devpath2(input: TokenStream) -> TokenStream {
    match expand_devpath(input) {
        Ok(tokens) => tokens,
        Err(error) => error.into_compile_error(),
    }
}

fn expand_devpath(input: TokenStream) -> syn::Result<TokenStream> {
    let input = syn::parse2::<DevicePathInput>(input)?;
    let value = input.literal.value();
    let bytes = if input.vendor_defined_schemas.is_empty() {
        encode_device_path(&value)
    } else {
        encode_device_path_with_vendor_defined(&value, &input.vendor_defined_schemas)
    }
    .map_err(|error| syn::Error::new(input.literal.span(), format_device_path_error(&value, &error)))?;
    let bytes = bytes.into_iter().map(Literal::u8_suffixed);
    Ok(quote!([#(#bytes),*]))
}

fn format_device_path_error(input: &str, error: &DevicePathError) -> String {
    let byte_offset = error.offset.min(input.len());
    let character_offset = input.get(..byte_offset).map_or(byte_offset, |prefix| prefix.chars().count());
    let context = device_path_error_context(input, byte_offset);
    format!("{} in {context} at character {character_offset}", error.message)
}

fn device_path_error_context(input: &str, offset: usize) -> String {
    let (node_start, node_end) = find_node_range(input, offset);
    let node = input.get(node_start..node_end).unwrap_or("");
    let Some(open_parenthesis) = node.find('(') else {
        return "file path node".to_owned();
    };

    let name = node.get(..open_parenthesis).unwrap_or("<unknown>");
    let argument_start = node_start + open_parenthesis + 1;
    if offset < argument_start {
        return format!("node `{name}`");
    }

    let relative_offset = offset.min(node_end).saturating_sub(argument_start);
    let arguments = input.get(argument_start..node_end).unwrap_or("");
    let (argument_index, argument) = argument_at_offset(arguments, relative_offset);
    if let Some(parameter) = parameter_name(argument) {
        format!("node `{name}`, parameter `{parameter}`")
    } else {
        format!("node `{name}`, argument {argument_index}")
    }
}

fn find_node_range(input: &str, offset: usize) -> (usize, usize) {
    let mut node_start = 0;
    let mut depth = 0usize;
    let mut quoted = false;

    for (index, character) in input.char_indices() {
        if character == '"' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }

        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '/' | '\\' | ',' if depth == 0 => {
                if offset <= index {
                    return (node_start, index);
                }
                node_start = index + character.len_utf8();
            }
            _ => {}
        }
    }

    (node_start, input.len())
}

fn argument_at_offset(arguments: &str, offset: usize) -> (usize, &str) {
    let mut argument_start = 0;
    let mut argument_index = 1;
    let mut quoted = false;

    for (index, character) in arguments.char_indices() {
        if character == '"' {
            quoted = !quoted;
        } else if character == ',' && !quoted {
            if offset <= index {
                return (argument_index, arguments.get(argument_start..index).unwrap_or(""));
            }
            argument_start = index + character.len_utf8();
            argument_index += 1;
        }
    }

    (argument_index, arguments.get(argument_start..).unwrap_or(""))
}

fn parameter_name(argument: &str) -> Option<&str> {
    let equals = argument.find('=')?;
    let name = argument.get(..equals)?;
    (!name.is_empty() && name.bytes().all(|byte| byte.is_ascii_alphanumeric())).then_some(name)
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::*;

    #[test]
    fn test_devpath_expands_to_owned_u8_array() {
        let expansion = devpath2(quote!("PciRoot(0)/Pci(0x11,0)"));

        assert_eq!(
            expansion.to_string(),
            "[2u8 , 1u8 , 12u8 , 0u8 , 208u8 , 65u8 , 3u8 , 10u8 , 0u8 , 0u8 , 0u8 , 0u8 , 1u8 , 1u8 , 6u8 , 0u8 , 0u8 , 17u8 , 127u8 , 255u8 , 4u8 , 0u8]"
        );
    }

    #[test]
    fn test_devpath_rejects_non_literal_input() {
        let expansion = devpath2(quote!(DEVICE_PATH));

        assert!(expansion.to_string().contains("expects exactly one string literal"));
    }

    #[test]
    fn test_devpath_rejects_additional_tokens() {
        let expansion = devpath2(quote!("Pci(1,0)", "USB(1,0)"));

        assert!(expansion.to_string().contains("accepts exactly one string literal"));
    }

    #[test]
    fn test_devpath_encodes_vendor_hardware_schema() {
        let custom = devpath2(quote!(
            vendor-defined {
                AcmeController {
                    type: hardware,
                    guid: "00112233-4455-6677-8899-aabbccddeeff",
                    fields: [port: u8, flags: u16le],
                },
            };
            "AcmeController(3,0x1234)"
        ));
        let canonical = devpath2(quote!("VenHw(00112233-4455-6677-8899-aabbccddeeff,033412)"));

        assert_eq!(custom.to_string(), canonical.to_string());
    }

    #[test]
    fn test_devpath_encodes_vendor_messaging_schema() {
        let custom = devpath2(quote!(
            vendor-defined {
                AcmeTransport {
                    type: messaging,
                    guid: "00112233-4455-6677-8899-aabbccddeeff",
                    fields: [channel: u8, flags: u16le],
                },
            };
            "AcmeTransport(3,0x1234)"
        ));
        let canonical = devpath2(quote!("VenMsg(00112233-4455-6677-8899-aabbccddeeff,033412)"));

        assert_eq!(custom.to_string(), canonical.to_string());
    }

    #[test]
    fn test_devpath_encodes_vendor_media_schema() {
        let custom = devpath2(quote!(
            vendor-defined {
                AcmeMedia {
                    type: media,
                    guid: "00112233-4455-6677-8899-aabbccddeeff",
                    fields: [instance: u8, flags: u16le],
                },
            };
            "AcmeMedia(3,0x1234)"
        ));
        let canonical = devpath2(quote!("VenMedia(00112233-4455-6677-8899-aabbccddeeff,033412)"));

        assert_eq!(custom.to_string(), canonical.to_string());
    }

    #[test]
    fn test_devpath_encodes_all_vendor_defined_field_types() {
        let input = syn::parse2::<DevicePathInput>(quote!(
            vendor-defined {
                VendorData {
                    type: hardware,
                    guid: "00112233-4455-6677-8899-aabbccddeeff",
                    fields: [
                        byte: u8,
                        word: u16le,
                        dword: u32le,
                        qword: u64le,
                        efiGuid: guid,
                        rfcUuid: uuid,
                        data: bytes,
                    ],
                },
            };
            "VendorData(1,0x0203,0x04050607,0x08090a0b0c0d0e0f,00112233-4455-6677-8899-aabbccddeeff,00112233-4455-6677-8899-aabbccddeeff,aabb)"
        ))
        .expect("schema should parse");
        let bytes = encode_device_path_with_vendor_defined(&input.literal.value(), &input.vendor_defined_schemas)
            .expect("vendor node should encode");

        assert_eq!(
            &bytes[20..],
            &[
                0x01, 0x03, 0x02, 0x07, 0x06, 0x05, 0x04, 0x0f, 0x0e, 0x0d, 0x0c, 0x0b, 0x0a, 0x09, 0x08, 0x33, 0x22,
                0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11, 0x22,
                0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0xaa, 0xbb, 0x7f, 0xff,
                0x04, 0x00,
            ]
        );
    }

    #[test]
    fn test_devpath_rejects_invalid_vendor_defined_schemas() {
        for (input, expected) in [
            (
                quote!(
                    vendor-defined {
                        Pci {
                            type: hardware,
                            guid: "00112233-4455-6677-8899-aabbccddeeff",
                            fields: [],
                        },
                    };
                    "Pci()"
                ),
                "built-in device path node",
            ),
            (
                quote!(
                    vendor-defined {
                        Acme {
                            type: hardware,
                            guid: "not-a-guid",
                            fields: [],
                        },
                    };
                    "Acme()"
                ),
                "is not a valid GUID",
            ),
            (
                quote!(
                    vendor-defined {
                        Acme {
                            type: hardware,
                            guid: "00112233-4455-6677-8899-aabbccddeeff",
                            fields: [port: u24le],
                        },
                    };
                    "Acme(1)"
                ),
                "unsupported vendor-defined field type",
            ),
            (
                quote!(
                    vendor-defined {};
                    "Pci(1,0)"
                ),
                "must declare at least one schema",
            ),
            (
                quote!(
                    vendor-defined {
                        Acme {
                            type: hardware,
                            guid: "00112233-4455-6677-8899-aabbccddeeff",
                            fields: [port: u8, port: u16le],
                        },
                    };
                    "Acme(1,2)"
                ),
                "field `port` was declared more than once",
            ),
            (
                quote!(
                    vendor-defined {
                        Acme {
                            type: hardware,
                            guid: "00112233-4455-6677-8899-aabbccddeeff",
                            fields: [],
                        },
                        Acme {
                            type: messaging,
                            guid: "11223344-5566-7788-99aa-bbccddeeff00",
                            fields: [],
                        },
                    };
                    "Acme()"
                ),
                "schema `Acme` was declared more than once",
            ),
            (
                quote!(
                    vendor-defined {
                        Acme {
                            type: bios,
                            guid: "00112233-4455-6677-8899-aabbccddeeff",
                            fields: [],
                        },
                    };
                    "Acme()"
                ),
                "`type` must be `hardware`, `messaging`, or `media`",
            ),
        ] {
            assert!(devpath2(input).to_string().contains(expected));
        }
    }

    #[test]
    fn test_devpath_reports_node_and_argument_context() {
        let expansion = devpath2(quote!("Pci(1,9)"));
        let message = expansion.to_string();

        assert!(message.contains("node `Pci`, argument 2"));
        assert!(message.contains("character 6"));
    }

    #[test]
    fn test_devpath_reports_named_parameter_and_unicode_character_offset() {
        let expansion = devpath2(quote!("é/Pci(Device=32,Function=0)"));
        let message = expansion.to_string();

        assert!(message.contains("node `Pci`, parameter `Device`"));
        assert!(message.contains("character 13"));
    }
}
