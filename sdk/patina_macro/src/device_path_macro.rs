//! Procedural macro front end for compile-time UEFI device paths.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use proc_macro2::{Literal, TokenStream};
use quote::quote;
use syn::{
    LitStr,
    parse::{Parse, ParseStream},
};

use crate::{device_path_encoder::encode_device_path, device_path_parser::DevicePathError};

struct DevicePathInput {
    literal: LitStr,
}

impl Parse for DevicePathInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let literal = input
            .parse::<LitStr>()
            .map_err(|_| syn::Error::new(input.span(), "`devpath!` expects exactly one string literal"))?;
        if !input.is_empty() {
            return Err(syn::Error::new(input.span(), "`devpath!` accepts exactly one string literal"));
        }
        Ok(Self { literal })
    }
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
    let bytes = encode_device_path(&value)
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
