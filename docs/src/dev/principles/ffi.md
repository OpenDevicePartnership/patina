# FFI Authoring

Patina exposes C FFIs for interoperability with C based drivers. This documentation gives guidance on how to write these
FFIs with patina specific guidance. For general guidance, view
[the Rust FFI docs](https://doc.rust-lang.org/nomicon/ffi.html).

## Patina Specific Considerations

### EFIAPI

All FFI functions must be marked with `extern "efiapi"` in order to use the EFIAPI ABI. Failure to include this
will cause undefined behavior.

>**Note:**: The one exception to this, temporarily, is for variadic functions. Rust only contains support for these
through the
[unstable c_variadics feature](https://doc.rust-lang.org/beta/unstable-book/language-features/c-variadic.html). This
feature only supports variadics for `extern "C"`. The UEFI spec has some variadic functions that are required. As such,
these functions use `extern "C"` for now. [This issue](https://github.com/r-efi/r-efi/issues/95) tracks adding support
for `extern "efiapi"`.

### Function Pointer Arguments at FFI Boundaries

Extern functions that accept function pointers must use `Option<fn_ptr>` for those arguments, including
parameters that the interface requires to be non-null. Rust
[does not allow `extern "abi" fn` to be null](https://doc.rust-lang.org/nomicon/ffi.html#the-nullable-pointer-optimization).
However, C callers can pass NULL here. This creates undefined behavior; `ptr.is_null()` will return false. The
idiomatic way to describe possibly null `extern "abi" fn` pointers is by wrapping them in an `Option`.

Rust guarantees that `Option<extern "efiapi" fn(...)>` has the same size, alignment, and function call ABI
as the underlying function pointer, with `None` represented as null. See the
[`Option` representation guarantees](https://doc.rust-lang.org/core/option/index.html#representation).

In Patina, all FFIs may be passed `NULL` pointers from C code and so all `extern "abi" fn` pointers must be wrapped
in `Option` to detect this and act accordingly; this may be returning an error or proceeding down a different
execution path.

### va_list

AARCH64 defines the [AAPCS64 ABI](https://github.com/ARM-software/abi-aa/blob/main/aapcs64/aapcs64.rst) as the single
ABI for AARCH64. However, MSVC (and clang's aarch64-unknown-windows-msvc target to align to it) has broken this ABI in
some places, resulting in an MS ABI for AARCH64 as well.

The [aarch64-unknown-uefi target](https://doc.rust-lang.org/rustc/platform-support/unknown-uefi.html) used by Patina
uses the MS ABI because that is what LLVM supports for building PE/COFF images, which are required for UEFI. Within
Patina and Rust build code, this is immaterial, as long as the target is the same. However, it affects interoperability
with C based code built by gcc and clang's aarch64-linux-gnu target.

In most cases, the MS ABI aligns with the AAPCS64 ABI. However, an exception is the
[va_list type](https://github.com/ARM-software/abi-aa/blob/main/aapcs64/aapcs64.rst#142the-va_list-type). In order to
keep using the aarch64-unknown-uefi target and maintain compatibility with C based code, Patina does not allow using
the va_list type in FFIs. In practice, this has been seen very little.

>**Note:** This does not affect using variadic functions that take `...` as a parameter. Those are supported across
C FFIs. It is only the va_list type itself that cannot be passed by value or by reference.

### Pointer Alignment

Raw pointers received across an `extern "efiapi"` (or `extern "C"`) boundary must be treated as **potentially
unaligned**. C callers are not required to honor Rust's alignment invariants for `*mut T` / `*const T`, and the UEFI
specification often passes out-pointers whose alignment cannot be statically guaranteed (e.g. fields embedded in packed
C structures and addresses computed from byte offsets).

A misaligned read or write through a normal Rust dereference is undefined behavior even on architectures that
tolerate it in hardware. The compiler is also free to assume the pointer is aligned and reorder or coalesce accesses
based on that assumption. So, when working with caller-supplied pointers across an FFI boundary, you must use the
unaligned access intrinsics [`read_unaligned()`] and [`write_unaligned()`] or a safe abstraction like [`zerocopy`].

#### Rules

1. **Prefer [`zerocopy`] for structured reads.** When pulling a typed value out of a caller-supplied byte buffer,
   `zerocopy::FromBytes` provides a safe, alignment-agnostic alternative to manual `read_unaligned()` of a casted
   pointer.
2. **Read FFI pointers with [`read_unaligned()`].** Do not use methods that assume alignment, such as `*ptr`, `ptr::read`,
   `&*ptr`, `(*ptr).field`, or `(*(ptr as *const T)).clone()` on a caller-supplied pointer.
3. **Write FFI pointers with [`write_unaligned()`].** Do not use `*ptr = value` or `ptr::write`.
4. **Be consistent within a function.** Mixing aligned and unaligned access on the same pointer is a code smell and
   risks UB if the pointer is in fact misaligned.
5. **Null-check before every unaligned access.** `read_unaligned()` and `write_unaligned()` only relax the *alignment*
   requirement. The pointer must still be non-null and valid for the access. A null check should be the first operation
   performed on an untrusted caller-supplied pointer before any other dereference or access, including unaligned ones.

[`read_unaligned()`]: https://doc.rust-lang.org/std/primitive.pointer.html#method.read_unaligned()
[`write_unaligned()`]: https://doc.rust-lang.org/std/primitive.pointer.html#method.write_unaligned()
[`zerocopy`]: https://crates.io/crates/zerocopy

#### Zerocopy example

The following example shows an `extern "efiapi"` function that receives an untrusted, potentially unaligned byte
buffer from a C caller and parses a fixed-layout header out of it using [`zerocopy`].

Note that there are no raw-pointer casts, no `read_unaligned()` calls, and no `unsafe` blocks needed for the parse
itself: `FromBytes::ref_from_prefix` validates the length and returns a `&PacketHeader` borrowed directly from the
caller's buffer, regardless of its alignment.

```rust,editable
# extern crate zerocopy;
# extern crate zerocopy_derive;
use std::slice;
use zerocopy_derive::{FromBytes, Immutable, KnownLayout, Unaligned};

// This is a stand-in for `r_efi::efi::Status` so the example is self-contained
// and runnable on the Rust Playground (which does not have `r_efi`).
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct Status(usize);

impl Status {
    const SUCCESS: Self = Self(0);
    const INVALID_PARAMETER: Self = Self(0x8000_0000_0000_0002);
}

// `#[repr(C, packed)]` plus `Unaligned` means this type has no alignment requirement,
// so `zerocopy` will reinterpret an arbitrary byte slice as `&PacketHeader`.
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, FromBytes, Immutable, KnownLayout, Unaligned)]
struct PacketHeader {
    version: u16,
    flags: u16,
    payload_len: u32,
}

/// Parses the header of a caller-supplied buffer and writes the payload length back through
/// `out_payload_len`. Returns `INVALID_PARAMETER` if any pointer is null or the buffer is
/// too small to contain a `PacketHeader`.
///
/// # Safety
///
/// Although this function is not marked `unsafe` (it is invoked across the EFIAPI C boundary
/// with the UEFI ABI), the caller must still uphold the following invariants:
///
/// - If `buffer` is non-null, it points to at least `buffer_len` consecutive, initialized,
///   readable bytes.
/// - If `out_payload_len` is non-null, it points to a writable `u32`.
/// - Both regions remain valid and are not mutated by another thread for the duration of
///   the call.
///
/// Violating any of these is undefined behavior. Null pointers and a `buffer_len` smaller
/// than `size_of::<PacketHeader>()` are handled safely and return `INVALID_PARAMETER`.
pub extern "efiapi" fn parse_packet_header(
    buffer: *const u8,
    buffer_len: usize,
    out_payload_len: *mut u32,
) -> Status {
    if buffer.is_null() || out_payload_len.is_null() {
        return Status::INVALID_PARAMETER;
    }

    // SAFETY: Per the function's safety contract, when `buffer` is non-null it points to
    // `buffer_len` readable, initialized bytes in a single allocation. `from_raw_parts::<u8>()`
    // only requires `align_of::<u8>() == 1`, so any non-null `buffer` is sufficiently aligned.
    // The resulting `[u8]` is only handed to `zerocopy::FromBytes::ref_from_prefix()`, which
    // (thanks to `PacketHeader: Unaligned`) imposes no further alignment requirement.
    let bytes = unsafe { slice::from_raw_parts(buffer, buffer_len) };

    let Ok((header, _rest)) = <PacketHeader as zerocopy::FromBytes>::ref_from_prefix(bytes) else {
        return Status::INVALID_PARAMETER;
    };

    // Copy the packed field into a local to avoid taking a reference to an unaligned field.
    let payload_len = header.payload_len;

    // SAFETY: Per the function's safety contract, when `out_payload_len` is non-null it
    // points to a writable `u32`. `write_unaligned` removes the alignment requirement, so
    // any alignment the caller supplied is sound.
    unsafe { out_payload_len.write_unaligned(payload_len) };
    Status::SUCCESS
}

// You would not normally include a `main()` function in an FFI module, this is just done
// for demonstration purposes so the code can run on the Rust Playground.
fn main() {
    // Simulate a buffer handed to us by a C caller: a `PacketHeader` followed by
    // four extra trailing bytes. The buffer is intentionally larger than the header
    // to show that `ref_from_prefix` only consumes what it needs.
    let buffer: [u8; 12] = [
        0x01, 0x00,             // version       = 1
        0x02, 0x00,             // flags         = 2
        0x34, 0x12, 0x00, 0x00, // payload_len   = 0x1234
        0xAA, 0xBB, 0xCC, 0xDD, // trailing data (ignored by the header parse)
    ];

    let mut payload_len: u32 = 0;
    let status = parse_packet_header(buffer.as_ptr(), buffer.len(), &mut payload_len);

    assert_eq!(status, Status::SUCCESS);
    println!("payload_len = 0x{:x}", payload_len);
}
```

Running the example will print:

```text
payload_len = 0x1234
```

Key points:

- Null checks come first, before any dereference or slice construction.
- The parse itself is safe Rust. The only `unsafe` blocks are the unavoidable FFI primitives:
  - Constructing the slice from the caller's `(ptr, len)` pair
  - Writing through the out-pointer.
- `Unaligned` on `PacketHeader` lets `zerocopy` borrow directly from any byte alignment. Without it,
  `ref_from_prefix()` would only succeed on a properly aligned buffer.
- `ref_from_prefix()` performs the length check, so a manual `buffer_len >= size_of::<PacketHeader>()`
  comparison is not required.

#### When direct dereference is acceptable

Direct dereference (`*ptr`, `&*ptr`, `&mut *ptr`) is sound when the pointer's provenance guarantees alignment. In
Patina, this is typically the case for:

- Pointers obtained from `Box::leak`, `Vec`, or other Rust allocators (where alignment is guaranteed by construction).
- The `this` pointer of an internally-produced protocol struct that Patina itself installed via `Box::leak`. The
  layout of `#[repr(C)]` protocol structs is well-defined and Patina-allocated instances are aligned.
- Pointers derived from `&T` / `&mut T` references obtained earlier in the same function.

When in doubt, prefer the unaligned variants as they have very little to no measurable cost on properly aligned
pointers on the targets Patina supports.

#### Anti-pattern examples

```rust,ignore
// WRONG: direct write through a caller-supplied out-pointer.
unsafe { *handle = installed_handle };

// WRONG: aligned read of a caller-supplied integer.
let n = unsafe { *num_bytes };

// WRONG: aligned struct read out of an arbitrary byte buffer.
let header = unsafe { (*(buffer as *const Header)).clone() };

// WRONG: inconsistent as the same pointer is read aligned and is written unaligned.
let n = unsafe { *num_bytes };
// ...
unsafe { num_bytes.write_unaligned(actual) };
```

Replace each with the corresponding `read_unaligned()` / `write_unaligned()` form (and, for the struct case, a
`core::ptr::read_unaligned()::<Header>(buffer.cast())` or a `zerocopy::FromBytes` parse).
