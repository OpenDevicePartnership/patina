# UEFI Strings

The UEFI Specification defines two string encodings, and Patina provides a native and idiomatic type for each of them
in the `patina` SDK.

Use these types instead of passing raw `&[u16]`, `&[u8]`, or bare pointers around, so that encoding and NUL-termination
rules are enforced by the type system.

These types do not need to be used for every string in the codebase. Use them only for strings that are passed to or
received from UEFI interfaces, or for fixed-size fields in `#[repr(C)]` structures that mirror UEFI binary layouts. For
all other strings, use Rust's built-in `str` and `String`.

## Encodings

| UEFI type | Width          | Character set                       | Patina types                                  |
|-----------|----------------|-------------------------------------|-----------------------------------------------|
| `CHAR8`   | 1 byte (8-bit) | ASCII / ISO-Latin-1 (ISO-8859-1)    | `Char8Str`, `Char8String`, `Char8Array<N>`    |
| `CHAR16`  | 2 bytes        | UCS-2 (Unicode 2.1 / ISO/IEC 10646) | `Char16Str`, `Char16String`, `Char16Array<N>` |

`CHAR8` covers the ASCII characters and the ISO-Latin-1 character set, where every byte in `0x01..=0xFF` maps directly
to a Unicode scalar in `U+0001..=U+00FF`. `CHAR16` is used for all other characters and strings. UCS-2 is a fixed-width
encoding covering the Basic Multilingual Plane. Code points above `U+FFFF` (which would require a UTF-16 surrogate
pair) and lone surrogate code units are not valid UCS-2 and are rejected during conversion.

## Type families

Each encoding provides three types that mirror the roles of `str`, `String`, and a fixed-size array:

| Role                                       | CHAR8           | CHAR16           |
|--------------------------------------------|-----------------|------------------|
| Borrowed, unsized view                     | `Char8Str`      | `Char16Str`      |
| Owned, heap-backed (requires `alloc`)      | `Char8String`   | `Char16String`   |
| Owned, fixed capacity (usable in `const`)  | `Char8Array<N>` | `Char16Array<N>` |

The borrowed types are the CHAR8/CHAR16 analogues of [`core::ffi::CStr`], and the owned types are the analogues of
[`alloc::ffi::CString`]. All of them guarantee a single trailing NUL with no interior NUL, so a pointer obtained from
one is always valid to hand to an `extern "efiapi"` interface that expects a NUL-terminated string.

## Choosing a type

- Use `Char16Str` / `Char8Str` to borrow an existing NUL-terminated buffer, such as a value received across an FFI
  boundary, or to accept a string as a function parameter.
- Use `Char16String` / `Char8String` to build or own a string, typically by converting from a Rust `str`. These require
  the `alloc` feature.
- Use `Char16Array<N>` / `Char8Array<N>` for compile-time string constants and for `#[repr(C)]` structure fields that
  hold a fixed-size character array, which is a common UEFI layout.

Prefer `CHAR16` for anything that is passed to the UEFI interfaces that expect wide strings (variable names, load
options, file paths, text output). Reserve `CHAR8` for the interfaces and data structures that are specified in terms
of ASCII/Latin-1 bytes, such as SMBIOS strings and ACPI signatures.

```admonish note
The trailing NUL is part of the value's invariant, not something callers manage. `as_ptr()` always returns a pointer to
a NUL-terminated sequence, and the length accessors (`len`) never count the terminator.
```

## Borrowing a string from FFI

When a UEFI interface hands back a `*const CHAR16`, wrap it in a `Char16Str` to read it safely. The constructor scans
for the terminator and validates the encoding.

```rust
# extern crate patina;
use patina::Char16Str;

// A NUL-terminated CHAR16 buffer, as might be received across an FFI boundary.
let raw: [u16; 4] = [0x0045, 0x0046, 0x0049, 0x0000]; // "EFI\0"

// SAFETY: `raw` is NUL-terminated and remains valid for the duration of the borrow.
let name = unsafe { Char16Str::from_ptr(raw.as_ptr()) }.expect("valid UCS-2");

assert_eq!(name.len(), 3);
assert!(name == "EFI");
assert_eq!(name.chars().collect::<String>(), "EFI");
```

Use `from_ptr_bounded` when a maximum length is known, so the scan cannot read past the end of a
buffer that is missing its terminator.

## Building an owned string for an FFI call

To pass a Rust string to a UEFI interface, encode it into an owned string and use its pointer. The
conversion is fallible and returns a [`Result`], so invalid input never panics.

```rust
# extern crate patina;
use patina::Char16String;

// Encode a Rust string as UCS-2 for a call that expects `*const CHAR16`.
let variable_name = Char16String::try_from_str("BootOrder").expect("valid UCS-2");

// `as_ptr()` is always NUL-terminated and ready for `extern "efiapi"`.
let name_ptr = variable_name.as_ptr();
# let _ = name_ptr;

assert_eq!(variable_name.len(), 9);
```

## Compile-time string literals

The `char16!` and `char8!` macros build a `&'static` string from a string literal and validate the encoding at compile
time. An invalid literal is a compilation error (not a runtime panic).

```rust
# extern crate patina;
use patina::{char8, char16};

let ascii = char8!("EFI"); // &'static Char8Str  (CHAR8 / Latin-1)
let wide = char16!("EFI"); // &'static Char16Str (CHAR16 / UCS-2)

assert_eq!(ascii.len(), 3);
assert_eq!(wide.len(), 3);
assert!(wide == "EFI");
```

## Fixed-size fields in binary structures

`Char16Array<N>` and `Char8Array<N>` are `#[repr(transparent)]` over `[u16; N]` and `[u8; N]`, so they can be embedded
directly in `#[repr(C)]` structures that mirror UEFI binary layouts. `N` includes the trailing NUL, and unused trailing
slots are NUL-padded. `from_str` is a `const fn`, so these fields can be initialized in `const` contexts with
compile-time validation.

```rust
# extern crate patina;
use patina::Char16Array;

#[repr(C)]
struct DriverRecord {
    version: u32,
    // A fixed CHAR16[16] field, matching a UEFI binary layout.
    name: Char16Array<16>,
}

const RECORD: DriverRecord = DriverRecord { version: 1, name: Char16Array::from_str("Disk") };

assert_eq!(RECORD.name.as_char16_str().len(), 4);
assert!(RECORD.name.as_char16_str() == "Disk");
```

## Zero-copy parsing and serialization

`Char16Array<N>` and `Char8Array<N>` implement `zerocopy::FromBytes` and `zerocopy::IntoBytes`. A structure that embeds
them as fields can derive those traits too, to parse the structure directly from a raw byte buffer or view it as bytes
without a manual copy.

```rust
# extern crate patina;
# extern crate zerocopy;
use patina::Char8Array;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

#[derive(Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
struct EntryPoint {
    anchor_string: Char8Array<6>,
    revision: u8,
}

let entry = EntryPoint { anchor_string: Char8Array::from_str("_SM3_"), revision: 3 };
let bytes = entry.as_bytes();

let parsed = EntryPoint::read_from_bytes(bytes).expect("buffer is large enough");
assert!(parsed.anchor_string.as_char8_str() == "_SM3_");
```

`FromBytes` allows a value to be constructed from any bit pattern, including one with no NUL byte anywhere in the
array. That can't happen through `from_str`, `try_from_str`, or `Default`, but it can happen when parsing untrusted or
malformed data this way. In that case `as_char16_str` / `as_char8_str` (and the `Deref` impl) return an empty string
rather than panicking or reading out of bounds.

## Latin-1 (CHAR8) strings

`Char8Str` and its owned forms cover the ASCII and ISO-Latin-1 character set. Every byte in `0x01..=0xFF` is a valid
character, so bytes read from an interface are always valid Latin-1.

```rust
# extern crate patina;
use patina::{Char8Str, Char8String};

// Borrow a NUL-terminated Latin-1 buffer.
let borrowed = Char8Str::from_bytes_with_nul(b"ACPI\0").expect("valid Latin-1");
assert_eq!(borrowed.len(), 4);

// Build an owned Latin-1 string from a Rust `str`.
let owned = Char8String::try_from_str("Firmware").expect("valid Latin-1");
assert_eq!(owned.len(), 8);
assert!(owned == "Firmware");
```

## Validation and error handling

All conversions from a Rust `str` return a `Result` with a `StringError` describing why the input was rejected, rather
than panicking. This is the "safe runtime path" and should be preferred over the `const` `from_str` constructors
anywhere the input is not a fixed literal.

```rust
# extern crate patina;
use patina::{Char16String, Char8String, StringError};

// UCS-2 cannot represent code points above U+FFFF; the conversion fails instead of panicking.
let non_bmp = Char16String::try_from_str("emoji \u{1F600}");
assert!(matches!(non_bmp, Err(StringError::NotUcs2 { .. })));

// Latin-1 cannot represent code points above U+00FF.
let non_latin1 = Char8String::try_from_str("\u{0100}");
assert!(matches!(non_latin1, Err(StringError::NotLatin1 { .. })));

// Interior NUL characters are rejected by every conversion.
let interior_nul = Char16String::try_from_str("a\0b");
assert!(matches!(interior_nul, Err(StringError::InteriorNul { position: 1 })));
```

## Avoiding panics

Panics can halt firmware boot, so the string API avoids them during runtime:

- The `try_from_str` constructors return `Result` and never panic. Use them whenever the input is not a fixed literal.
- The `const fn` `from_str` constructors and the `char16!` / `char8!` macros do panic on invalid input, but only in
  `const`/`static` contexts, where the panic manifests as a compile-time error. Do not call `from_str` at runtime with
  untrusted input.
- Reading a string built through an unchecked constructor never panics. An invalid CHAR16 code unit is mapped to the
  Unicode replacement character (`U+FFFD`) rather than aborting.
- `Char16Array` / `Char8Array` values built by casting from arbitrary bytes (through their `zerocopy::FromBytes`
  implementation) are not guaranteed to contain a NUL terminator. Reading such a value returns an empty string rather
  than panicking.

[`core::ffi::CStr`]: https://doc.rust-lang.org/core/ffi/struct.CStr.html
[`alloc::ffi::CString`]: https://doc.rust-lang.org/alloc/ffi/struct.CString.html
[`Result`]: https://doc.rust-lang.org/core/result/enum.Result.html
