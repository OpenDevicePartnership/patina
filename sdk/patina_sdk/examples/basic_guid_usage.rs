//! This example demonstrates the various ways to work with UEFI GUIDs using the patina_sdk `Guid` type.
//!
//! The `Guid` type provides a flexible and ergonomic way to format and display UEFI GUIDs in human-readable format.
//!
//! It supports two primary input methods:
//! 1. **From `efi::Guid` references**: The traditional method for working with existing GUID constants
//! 2. **From validated string representations**: A fallible method that accepts various string formats
//!
//! ## String Format
//!
//! `Guid` accepts multiple string formats:
//! - `"XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX"`
//! - `"XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"`
//!
//! ## Usage Patterns
//!
//! The `Guid` type can be constructed in several ways:
//! - Direct constructor methods: `Guid::from_ref()` and `Guid::try_from_string()`
//! - Generic `From` trait: `Guid::from()` (for references only)
//! - Fallible `TryFrom` trait: Using `.try_into()` on strings
//!
//! ## Memory Usage
//!
//! The implementation avoids heap allocations, using stack-allocated buffers for formatting.

use patina_sdk::base::guid::{Guid, GuidError, OwnedGuid};
use patina_sdk::guids::*;

/// Demonstrates formatting GUIDs from `efi::Guid` references.
fn demonstrate_reference_formatting() {
    println!("=== GUID Formatting from efi::Guid References ===\n");

    // (1) Using the constructor method
    let dxe_core_guid = Guid::from_ref(&DXE_CORE);
    println!("  DXE Core Module GUID: {}", dxe_core_guid);
    println!("    Debug format: {:?}", dxe_core_guid);

    // (2) Using the generic From trait
    let zero_guid = Guid::from(&ZERO);
    println!("  Zero GUID: {}", zero_guid);

    // (3 Automatic conversion with the Into trait
    let perf_guid: Guid = (&PERFORMANCE_PROTOCOL).into();
    println!("  Performance Protocol GUID: {}", perf_guid);

    // (4) Printing various protocol GUIDs
    println!("\n  Common Protocol GUIDs:");
    println!("    SMM Communication: {}", Guid::from(&SMM_COMMUNICATION_PROTOCOL));
    println!("    Hardware Interrupt: {}", Guid::from(&HARDWARE_INTERRUPT_PROTOCOL));
    println!("    Memory Type Info: {}", Guid::from(&MEMORY_TYPE_INFORMATION));

    println!();
}

/// Demonstrates parsing and formatting GUIDs from string representations.
fn demonstrate_string_parsing() {
    println!("=== GUID Parsing from String Representations ===\n");

    let test_guid_str = "12345678-9ABC-DEF0-1234-56789ABCDEF0";

    // (1) Standard hyphenated format (most common and recommended for readability)
    match OwnedGuid::try_from_string(test_guid_str) {
        Ok(guid_hyphenated) => {
            println!("  From hyphenated string: {}", guid_hyphenated);
            println!("    Input:  \"{}\"", test_guid_str);
            println!("    Output: \"{}\"", guid_hyphenated);
        }
        Err(e) => println!("  Error parsing hyphenated string: {}", e),
    }

    // (2) Compact format without hyphens
    let compact_str = "123456789ABCDEF0123456789ABCDEF0";
    match OwnedGuid::try_from_string(compact_str) {
        Ok(guid_compact) => {
            println!("\n  From compact string: {}", guid_compact);
            println!("    Input:  \"{}\"", compact_str);
            println!("    Output: \"{}\"", guid_compact);
        }
        Err(e) => println!("\n  Error parsing compact string: {}", e),
    }

    // (3) Case insensitive parsing
    let lowercase_str = "12345678-9abc-def0-1234-56789abcdef0";
    match lowercase_str.try_into() as Result<OwnedGuid, GuidError> {
        Ok(guid_lowercase) => {
            println!("\n  From lowercase string: {}", guid_lowercase);
            println!("    Input:  \"{}\"", lowercase_str);
            println!("    Output: \"{}\"", guid_lowercase);
        }
        Err(e) => println!("\n  Error parsing lowercase string: {}", e),
    }

    // (4) Mixed case parsing
    let mixed_str = "12345678-9AbC-DeF0-1234-56789aBcDeF0";
    match OwnedGuid::try_from_string(mixed_str) {
        Ok(guid_mixed) => {
            println!("\n  From mixed-case string: {}", guid_mixed);
            println!("    Input:  \"{}\"", mixed_str);
            println!("    Output: \"{}\"", guid_mixed);
        }
        Err(e) => println!("\n  Error parsing mixed-case string: {}", e),
    }

    // (5) Whitespace handling
    let spaced_str = " 12345678-9ABC-DEF0-1234-56789ABCDEF0 ";
    match OwnedGuid::try_from_string(spaced_str) {
        Ok(guid_spaced) => {
            println!("\n  From string with whitespace: {}", guid_spaced);
            println!("    Input:  \"{}\"", spaced_str);
            println!("    Output: \"{}\"", guid_spaced);
        }
        Err(e) => println!("\n  Error parsing string with whitespace: {}", e),
    }

    println!();
}

/// Demonstrates how malformed cases are handled in GUID string parsing.
fn demonstrate_error_handling() {
    println!("=== Error Handling and Edge Cases ===\n");

    // (1) Invalid format - too short
    match OwnedGuid::try_from_string("12345678-1234-1234") {
        Ok(guid) => println!("  Unexpected success for too short: {}", guid),
        Err(e) => {
            println!("  Invalid (too short): Error - {}", e);
            println!("    Original: \"12345678-1234-1234\"");
        }
    }

    // (2) Invalid format - non-hex characters
    match OwnedGuid::try_from_string("GGGGGGGG-HHHH-IIII-JJJJ-KKKKKKKKKKKK") {
        Ok(guid) => println!("  Unexpected success for non-hex: {}", guid),
        Err(e) => {
            println!("  Invalid (non-hex): Error - {}", e);
            println!("    Original: \"GGGGGGGG-HHHH-IIII-JJJJ-KKKKKKKKKKKK\"");
        }
    }

    // (3) Invalid format - completely malformed
    match OwnedGuid::try_from_string("not-a-guid-at-all") {
        Ok(guid) => println!("  Unexpected success for malformed: {}", guid),
        Err(e) => {
            println!("  Invalid (malformed): Error - {}", e);
            println!("    Original: \"not-a-guid-at-all\"");
        }
    }

    // (4) Empty string
    match OwnedGuid::try_from_string("") {
        Ok(guid) => println!("  Unexpected success for empty: {}", guid),
        Err(e) => {
            println!("  Empty string: Error - {}", e);
            println!("    Original: \"\"");
        }
    }

    // (5) Too long GUID
    match OwnedGuid::try_from_string("12345678-9ABC-DEF0-1234-56789ABCDEF012") {
        Ok(guid) => println!("  Unexpected success for too long: {}", guid),
        Err(e) => {
            println!("  Too long: Error - {}", e);
            println!("    Original: \"12345678-9ABC-DEF0-1234-56789ABCDEF012\"");
        }
    }

    // (6) Demonstrate error types
    println!("\n  Error Type Examples:");
    if let Err(e) = OwnedGuid::try_from_string("too-short") {
        println!("    Length Error: {}", e);
    }
    if let Err(e) = OwnedGuid::try_from_string("12345678-9ABC-DEF0-1234-56789ABCDEFG") {
        println!("    Character Error: {}", e);
    }

    println!();
}

/// Demonstrates practical usage scenarios.
fn demonstrate_practical_usage() {
    println!("=== Practical Usage Scenarios ===\n");

    // (1) Protocol identification in logging
    println!("  Protocol Logging Example:");
    let protocols =
        vec![("Performance Protocol", &PERFORMANCE_PROTOCOL), ("SMM Communication", &SMM_COMMUNICATION_PROTOCOL)];

    for (name, guid) in protocols {
        println!("    [INFO] Loading protocol '{}' with GUID: {}", name, Guid::from(guid));
    }

    // (2) Event group identification
    println!("\n  Event Group Example:");
    println!("    [EVENT] End of DXE event signaled: {}", Guid::from(&EVENT_GROUP_END_OF_DXE));
    println!("    [EVENT] Exit Boot Services failed: {}", Guid::from(&EBS_FAILED));

    // (3) Configuration file or user input parsing
    println!("\n  Configuration Parsing Example:");
    let config_guid_strings = vec![
        "Hardware Config: 32898322-2DA1-474A-BAAA-F3F7CF569470",
        "Memory Type Info: 4C19049F413744D39C108B97A83FFDFA",
        "FPDT Extended: 3b387bfd-7abc-4cf2-a0ca-b6a16c1b1b25",
    ];

    for config_line in config_guid_strings {
        let parts: Vec<&str> = config_line.split(": ").collect();
        if parts.len() == 2 {
            let name = parts[0];
            match OwnedGuid::try_from_string(parts[1]) {
                Ok(guid) => println!("    [CONFIG] Parsed '{}' -> {}", name, guid),
                Err(e) => println!("    [ERROR] Failed to parse '{}': {}", name, e),
            }
        }
    }

    // (4) Comparison and validation example
    println!("\n  GUID Comparison Example:");
    let user_input = "00000000-0000-0000-0000-000000000000";
    match OwnedGuid::try_from_string(user_input) {
        Ok(parsed_guid) => {
            let zero_guid = Guid::from(&ZERO);

            println!("    User input: {}", parsed_guid);
            println!("    Zero GUID:  {}", zero_guid);

            // Direct equality comparison!
            println!("    Direct equality: {}", parsed_guid == zero_guid);
            println!("    String format match: {}", format!("{}", parsed_guid) == format!("{}", zero_guid));
        }
        Err(e) => println!("    Error parsing user input: {}", e),
    }

    // (5) Cross-format equality examples
    println!("\n  Cross-Format Equality Examples:");
    let compact_result = OwnedGuid::try_from_string("00000000000000000000000000000000");
    let hyphenated_result = OwnedGuid::try_from_string("00000000-0000-0000-0000-000000000000");
    let ref_zero = Guid::from(&ZERO);

    match (compact_result, hyphenated_result) {
        (Ok(compact_zero), Ok(hyphenated_zero)) => {
            println!("    Compact format:    {}", compact_zero);
            println!("    Hyphenated format: {}", hyphenated_zero);
            println!("    From reference:    {}", ref_zero);
            println!("    All equal? {}", compact_zero == hyphenated_zero && hyphenated_zero == ref_zero);
        }
        (Err(e1), _) => println!("    Error parsing compact format: {}", e1),
        (_, Err(e2)) => println!("    Error parsing hyphenated format: {}", e2),
    }

    // (6) Case insensitive equality
    let uppercase_result = OwnedGuid::try_from_string("12345678-9ABC-DEF0-1234-56789ABCDEF0");
    let lowercase_result = OwnedGuid::try_from_string("12345678-9abc-def0-1234-56789abcdef0");
    println!("\n    Case Insensitive Equality:");

    match (uppercase_result, lowercase_result) {
        (Ok(uppercase_guid), Ok(lowercase_guid)) => {
            println!("    Uppercase: {}", uppercase_guid);
            println!("    Lowercase: {}", lowercase_guid);
            println!("    Equal? {}", uppercase_guid == lowercase_guid);
        }
        (Err(e1), _) => println!("    Error parsing uppercase: {}", e1),
        (_, Err(e2)) => println!("    Error parsing lowercase: {}", e2),
    }

    // (7) Comparing different protocol GUIDs
    println!("\n  Different Protocol Comparison Example:");
    let guid1 = Guid::from(&PERFORMANCE_PROTOCOL);
    let guid2 = Guid::from(&SMM_COMMUNICATION_PROTOCOL);
    println!("    Performance Protocol: {}", guid1);
    println!("    SMM Communication:    {}", guid2);
    println!("    Are they equal? {}", guid1 == guid2);

    println!();
}

fn main() {
    println!("Patina GUID Usage Examples");
    println!("==========================\n");

    println!("These examples demonstrate how the Patina Guid type can be used.\n");

    demonstrate_reference_formatting();
    demonstrate_string_parsing();
    demonstrate_error_handling();
    demonstrate_practical_usage();
}
