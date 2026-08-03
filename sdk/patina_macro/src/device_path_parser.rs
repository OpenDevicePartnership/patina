//! Parser for the UEFI device path text representation.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

// cspell:ignore UIDSTR

use core::fmt;

/// A device path parsing or encoding error.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct DevicePathError {
    pub(crate) offset: usize,
    pub(crate) message: String,
}

impl DevicePathError {
    pub(crate) fn new(offset: usize, message: impl Into<String>) -> Self {
        Self { offset, message: message.into() }
    }
}

impl fmt::Display for DevicePathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at character {}", self.message, self.offset)
    }
}

impl std::error::Error for DevicePathError {}

/// A parsed device path containing one or more instances.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ParsedDevicePath {
    pub(crate) instances: Vec<Vec<ParsedNode>>,
}

/// A parsed canonical node or file path segment.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ParsedNode {
    pub(crate) offset: usize,
    pub(crate) kind: ParsedNodeKind,
}

/// The two UEFI text node forms.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ParsedNodeKind {
    Canonical { name: String, arguments: Vec<ParsedArgument> },
    FilePath(String),
}

/// A positional or named node argument.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ParsedArgument {
    pub(crate) name: Option<String>,
    pub(crate) value: String,
    pub(crate) offset: usize,
}

/// Parse a complete UEFI text device path.
pub(crate) fn parse_device_path(input: &str) -> Result<ParsedDevicePath, DevicePathError> {
    if input.is_empty() {
        return Err(DevicePathError::new(0, "device path cannot be empty"));
    }

    let mut offset = 0;
    let mut instances = vec![Vec::new()];

    if byte_at(input, offset) == Some(b'/') {
        offset += 1;
        if offset == input.len() {
            return Err(DevicePathError::new(offset - 1, "device path cannot end with a separator"));
        }
    }

    while offset < input.len() {
        let node_start = offset;
        let (node_end, separator) = find_node_end(input, node_start)?;
        if node_end == node_start {
            return Err(DevicePathError::new(node_start, "empty device path node"));
        }

        let node_text = input
            .get(node_start..node_end)
            .ok_or_else(|| DevicePathError::new(node_start, "device path contains an invalid UTF-8 boundary"))?;
        let node = parse_node(node_text, node_start)?;
        instances.last_mut().expect("at least one instance exists").push(node);

        offset = node_end;
        match separator {
            None => break,
            Some(b'/') => {
                offset += 1;
                if offset == input.len() {
                    return Err(DevicePathError::new(offset - 1, "device path cannot end with a separator"));
                }
            }
            Some(b',') => {
                offset += 1;
                if offset == input.len() {
                    return Err(DevicePathError::new(offset - 1, "device path cannot end with an instance separator"));
                }
                instances.push(Vec::new());
            }
            Some(_) => unreachable!("find_node_end only returns recognized separators"),
        }
    }

    if instances.iter().any(Vec::is_empty) {
        return Err(DevicePathError::new(offset, "device path instance cannot be empty"));
    }

    Ok(ParsedDevicePath { instances })
}

fn find_node_end(input: &str, start: usize) -> Result<(usize, Option<u8>), DevicePathError> {
    let bytes = input.as_bytes();
    let mut offset = start;
    let mut depth = 0usize;
    let mut quote_start = None;

    while offset < bytes.len() {
        let byte = *bytes.get(offset).expect("offset is checked against the input length");
        if byte == b'"' {
            quote_start = if quote_start.is_some() { None } else { Some(offset) };
            offset += 1;
            continue;
        }

        if quote_start.is_none() {
            if byte.is_ascii_whitespace() {
                return Err(DevicePathError::new(offset, "unquoted whitespace is not allowed"));
            }
            match byte {
                b'(' => depth += 1,
                b')' => {
                    if depth == 0 {
                        return Err(DevicePathError::new(offset, "unmatched closing parenthesis"));
                    }
                    depth -= 1;
                }
                b'/' | b',' if depth == 0 => return Ok((offset, Some(byte))),
                b'|' | b'<' | b'>' => {
                    return Err(DevicePathError::new(offset, "shell-reserved character is not allowed"));
                }
                _ => {}
            }
        }
        offset += 1;
    }

    if let Some(quote_offset) = quote_start {
        return Err(DevicePathError::new(quote_offset, "unterminated quoted string"));
    }
    if depth != 0 {
        return Err(DevicePathError::new(start, "unmatched opening parenthesis"));
    }

    Ok((offset, None))
}

fn parse_node(text: &str, base_offset: usize) -> Result<ParsedNode, DevicePathError> {
    let Some(open) = text.find('(') else {
        return Ok(ParsedNode { offset: base_offset, kind: ParsedNodeKind::FilePath(text.to_owned()) });
    };

    if !text.ends_with(')') {
        return Err(DevicePathError::new(base_offset + open, "canonical node must end with a closing parenthesis"));
    }

    let name = text.get(..open).expect("find returned a valid UTF-8 boundary");
    if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
        return Err(DevicePathError::new(
            base_offset,
            "device path node name must contain only alphanumeric characters or `-`",
        ));
    }

    let argument_text = text
        .get(open + 1..text.len() - 1)
        .ok_or_else(|| DevicePathError::new(base_offset + open, "invalid canonical node boundary"))?;
    let arguments = parse_arguments(argument_text, base_offset + open + 1)?;

    Ok(ParsedNode { offset: base_offset, kind: ParsedNodeKind::Canonical { name: name.to_owned(), arguments } })
}

fn parse_arguments(text: &str, base_offset: usize) -> Result<Vec<ParsedArgument>, DevicePathError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }

    let bytes = text.as_bytes();
    let mut arguments = Vec::new();
    let mut argument_start = 0;
    let mut quote_start = None;

    for offset in 0..=bytes.len() {
        let at_end = offset == bytes.len();
        let byte = bytes.get(offset).copied();

        if byte == Some(b'"') {
            quote_start = if quote_start.is_some() { None } else { Some(offset) };
        }

        if at_end || (byte == Some(b',') && quote_start.is_none()) {
            let value = text
                .get(argument_start..offset)
                .ok_or_else(|| DevicePathError::new(base_offset + argument_start, "invalid argument boundary"))?;
            arguments.push(parse_argument(value, base_offset + argument_start)?);
            argument_start = offset + 1;
        }
    }

    if let Some(offset) = quote_start {
        return Err(DevicePathError::new(base_offset + offset, "unterminated quoted argument"));
    }

    Ok(arguments)
}

fn parse_argument(text: &str, offset: usize) -> Result<ParsedArgument, DevicePathError> {
    let mut quoted = false;
    let equals = text.char_indices().find_map(|(index, character)| match character {
        '"' => {
            quoted = !quoted;
            None
        }
        '=' if !quoted => Some(index),
        _ => None,
    });
    let (name, value, value_offset) = if let Some(equals) = equals {
        let name = text.get(..equals).expect("find returned a valid UTF-8 boundary");
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err(DevicePathError::new(offset, "parameter identifier must be alphanumeric"));
        }
        (
            Some(name.to_owned()),
            text.get(equals + 1..).expect("find returned a valid UTF-8 boundary"),
            offset + equals + 1,
        )
    } else {
        (None, text, offset)
    };

    let value = if value.starts_with('"') || value.ends_with('"') {
        if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
            return Err(DevicePathError::new(value_offset, "quoted argument must have matching double quotes"));
        }
        value
            .get(1..value.len() - 1)
            .ok_or_else(|| DevicePathError::new(value_offset, "invalid quoted argument boundary"))?
            .to_owned()
    } else {
        value.to_owned()
    };

    Ok(ParsedArgument { name, value, offset: value_offset })
}

fn byte_at(input: &str, offset: usize) -> Option<u8> {
    input.as_bytes().get(offset).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_device_path_nodes_and_instances() {
        let parsed = parse_device_path("PciRoot(0)/Pci(0x11,0),USB(1,0)").expect("path should parse");

        assert_eq!(parsed.instances.len(), 2);
        assert_eq!(parsed.instances[0].len(), 2);
        assert_eq!(parsed.instances[1].len(), 1);
    }

    #[test]
    fn test_parse_named_and_quoted_arguments() {
        let parsed = parse_device_path("AcpiEx(HID=HWP0002,UIDSTR=\"Root Bridge\")").expect("path should parse");
        let ParsedNodeKind::Canonical { arguments, .. } = &parsed.instances[0][0].kind else {
            panic!("expected canonical node");
        };

        assert_eq!(arguments[0].name.as_deref(), Some("HID"));
        assert_eq!(arguments[1].value, "Root Bridge");
    }

    #[test]
    fn test_parse_rejects_unquoted_whitespace() {
        let error = parse_device_path("Pci(1, 0)").expect_err("path should fail");
        assert_eq!(error.offset, 6);
    }

    #[test]
    fn test_parse_allows_equals_inside_quoted_value() {
        let parsed = parse_device_path("Uri(\"https://example.com/?a=b\")").expect("path should parse");
        let ParsedNodeKind::Canonical { arguments, .. } = &parsed.instances[0][0].kind else {
            panic!("expected canonical node");
        };

        assert_eq!(arguments[0].name, None);
        assert_eq!(arguments[0].value, "https://example.com/?a=b");
    }

    #[test]
    fn test_parse_allows_hyphenated_node_name() {
        parse_device_path("Wi-Fi(Test)").expect("path should parse");
    }

    #[test]
    fn test_parse_keeps_quoted_commas_inside_one_argument() {
        let parsed = parse_device_path("Uri(\"https://example.com/a,b\")").expect("path should parse");
        let ParsedNodeKind::Canonical { arguments, .. } = &parsed.instances[0][0].kind else {
            panic!("expected canonical node");
        };

        assert_eq!(arguments.len(), 1);
        assert_eq!(arguments[0].value, "https://example.com/a,b");
    }

    #[test]
    fn test_parse_accepts_leading_separator_and_named_then_positional_arguments() {
        let parsed = parse_device_path("/Pci(Device=17,0)").expect("path should parse");
        let ParsedNodeKind::Canonical { arguments, .. } = &parsed.instances[0][0].kind else {
            panic!("expected canonical node");
        };

        assert_eq!(arguments[0].name.as_deref(), Some("Device"));
        assert_eq!(arguments[1].name, None);
    }

    #[test]
    fn test_parse_preserves_backslashes_in_file_path() {
        let parsed = parse_device_path(r"\EFI\BOOT\BOOTX64.EFI").expect("path should parse");
        let ParsedNodeKind::FilePath(path) = &parsed.instances[0][0].kind else {
            panic!("expected file path node");
        };

        assert_eq!(path, r"\EFI\BOOT\BOOTX64.EFI");
    }

    #[test]
    fn test_parse_rejects_reserved_shell_characters() {
        let error = parse_device_path("Uri(http://example.com/<bad>)").expect_err("path should fail");

        assert_eq!(error.message, "shell-reserved character is not allowed");
    }
}
