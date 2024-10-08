# Inline Code Documentation

This chapter lays out the standards of practice for inline rust documentation for generating rust
docs. It also provides templates that should be followed when creating documentation for these
items. You can review the [Templates and Quick Reference](./documenting/reference.md), however if
this is your first time seeing this document, please read it in it's entirety.

The most important items to document are those marked with the `pub` keyword, as they will have
automatic documentation generated for them. When adding new code, the developer should always run
`cargo doc --open` and review the documentation for their code.

## Common Sections

All sections are describing as `## section <section_name>` inside inline doc comments. These are a
common set of these sections used below, however do not hesitate to create a custom section if it
is appropriate.

### Examples

The examples section is used to provide example usage to a user using the inline code markdown
functionality e.g. ```` ``` ````. The great thing about writing examples, is that `cargo test` will
run these examples and fail if they are incorrect. This ensures your examples are always up date!

There are situations where you may expect the example to not compile, fail, panic, etc. To support
this, you can pass attributes to the inline code examples, to tell rust what to expect. Some
supported attributes are `should_panic`, `no_run`, `compile_fail`, and `ignore`.

Including `#[doc(html_playground_url = "https://playground.example.com/")]` will allow examples to
be runnable in the documentation.

``` rust
/// ## Examples
///
/// optional description
///
/// ``` <attribute>
/// <code></code>
/// ```
```

### Errors

The errors section documents the expected error values when the output of a function is a `Result`.
This section should be an exhaustive list of expected errors, but **not** an exhaustive list of the
error enum values (unless all are possible).  You should always contain the error type as a linked
reference and the reason why the error would be returned.

``` rust
/// ## Errors
///
/// Returns [ErrorName1](crate::module::ErrorEnum::Error1) when <this> happens
/// Returns [ErrorName2](crate::module::ErrorEnum::Error2) when <this> happens
///
```

### Safety

The safety section must be provided for any function that is marked as `unsafe` and is used to
document (1) What makes this function unsafe and (2) the expected scenario in which this function
will operate safely and as expected. A safety section should also be bubbled up to the `struct`
(if applicable) and the `module` if any function is unsafe.

It is common (but not required) to see pre-condition checks in the function that validates these
conditions, and panic if they fail. One common example is `slice::from_raw_parts` which will panic
with the statement:

``` txt
unsafe precondition(s) violated: slice::from_raw_parts requires the pointer
to be aligned and non-null, and the total size of the slice not to exceed `isize::MAX`
```

``` rust
/// ## Safety
/// 
/// <comments>
```

### Panics

Provide general description and comments on any functions that use `.unwrap()`, `debug_assert!`,
etc. that would result in a panic. Typically only used when describing functions.

### Lifetimes

Provide a general description and comments on any types that have lifetimes more complex than a
single lifetime (explicit or implicit). Assume that the developer understands lifetimes; focus on
why the lifetime was modeled a certain way rather than describing why it was needed to make the
compiler happy! Typically only used when describing types.

## Style Guides

The goal is to create documentation that provides developers with a clear and concise description
on how to use a crate, module, type, or function while keeping it clean when auto-generating
documentation with `cargo doc`. As alluded to, it is the responsibility of the developer to ensure
that each library crate, public module, public type, and public function is well documented. Below
are the expectations for each. If a common section is not applicable to the documented item, do not
include it.

### Crate Style Guide

Crate documentation should be located at the top of the lib.rs or main.rs file. The intent is to
describe the purpose of the crate, providing any setup instructions and examples. This is also the
place to describe any common misconceptions or "gotchas". Doc comments here use `//!` specifying we
are documenting the *parent* item (the crate).

``` rust
//! PE32 Management
//!
//! This library provides high-level functionality for operating on and representing PE32 images.
//!
//! ## Examples and Usage
//!
//! ```
//! let file: File = File::open(test_collateral!("test_image.pe32"))
//!   .expect("failed to open test file.");
//!
//! let mut buffer: Vec<u8> = Vec::new();
//! file.read_to_end(&mut buffer).expect("Failed to read test file");
//!
//! let image_info: Pe32ImageInfo = pe32_get_image_info(buffer).unwrap();
//!
//! let mut loaded_image: Vec<u8> = vec![0; image_info.size_of_image as usize];
//! pe32_load_image(&image, &mut loaded_image).unwrap();
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
```

### Module Style Guide

Module documentation should be placed at the top of a module, whether that be a mod.rs file or the
module itself if contained to a single file. If a crate only consists of a single module, the crate
style guide should be used.Submodules should be avoided if possible, as they cause confusion. The
goal is to describe the types found in this module and their interactions with the rest of the
crate. Doc comments here use `//!` specifying we are documenting the *parent* item (the module).

``` rust
//! PE32 Management
//!
//! This module provides high-level functionality for operating on and representing PE32 images.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
```

### Type Style Guide

Type documentation should be available for all public public types such as enums, structs, etc.
The focus should be on the construction of the type (when / how), Destruction of the type if a
custom Drop trait is implemented, and any performance concerns. Doc comments here use `///`
specifying we are documenting the item directly below it (the type or member of the type).

**Document traits, not trait implementations!**

``` rust
/// Type for describing errors that result from working with PE32 images.
#[derive(Debug)]
pub enum Pe32Error {
    /// Goblin failed to parse the PE32 image.
    ///
    /// See the enclosed goblin error for a reason why the parsing failed.
    ParseError(goblin::error::Error),
    /// The parsed PE32 image does not contain an Optional Header.
    NoOptionalHeader,
    /// Failed to load the PE32 image into the provided memory buffer.
    LoadError,
    /// Failed to relocate the loaded image to the destination.
    RelocationError,
}

/// Type containing information about a PE32 image.
#[derive(PartialEq, Debug)]
pub struct Pe32ImageInfo {
    /// The offset of the entry point relative to the start address of the PE32 image.
    pub entry_point_offset: usize,
    /// The subsystem type (IMAGE_SUBSYSTEM_EFI_BOOT_SERVICE_DRIVER [0xB], etc.).
    pub image_type: u16,
    /// The total length of the image.
    pub size_of_image: u32,
    /// The size of an individual section in a power of 2 (4K [0x1000], etc.).
    pub section_alignment: u32,
    /// The ascii string representation of a file (<filename>.efi).
    pub filename: Option<String>,
}
```

### Function Style Guide

Function documentation should be available for functions of a public type (associated functions),
and any public functions. At least one example is required for each function in addition to the
other sections mentioned below.

Do not provide an arguments section, the name and type of the argument should make it self-evident.

Do not provide a Returns section, this should be captured in the longer description and the return
type makes the possible return value self-evident.

``` rust

/// Attempts to parse a PE32 image and return information about the image.
///
/// Parses the bytes buffer containing a PE32 image and generates a [Pe32ImageInfo] struct
/// containing general information about the image otherwise an error.
///
/// ## Errors
///
/// Returns [`ParseError`](Pe32Error::ParseError) if parsing the PE32 image failed. Contains the
/// exact parsing [`Error`](goblin::error::Error).
///
/// Returns [`NoOptionalHeader`](Pe32Error::NoOptionalHeader) if the parsed PE32 image does not
/// contain the OptionalHeader necessary to provide information about the image.
///
/// ## Examples
///
/// ```
/// extern crate std;
///
/// use std::{fs::File, io::Read};
/// use uefi_pe32_lib::pe32_get_image_info;
///
/// let mut file: File = File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/test/","test_image.pe32"))
///   .expect("failed to open test file.");
///
/// let mut buffer: Vec<u8> = Vec::new();
/// file.read_to_end(&mut buffer).expect("Failed to read test file");
///
/// let image_info = pe32_get_image_info(&buffer).unwrap();
/// ```
///
pub fn pe32_get_image_info(image: &[u8]) -> Result<Pe32ImageInfo, Pe32Error> {
  ...
}
```
