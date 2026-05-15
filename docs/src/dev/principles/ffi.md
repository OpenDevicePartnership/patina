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
