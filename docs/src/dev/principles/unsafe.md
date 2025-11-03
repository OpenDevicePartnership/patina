# Unsafe Guidance

Unsafe code in Rust is a necessity for systems programming environments such as Patina. This document details
guidance in how and when to write unsafe code in Patina.

This document is intended to build upon the
[official Rust guidance on unsafe](https://doc.rust-lang.org/std/keyword.unsafe.html) and
[detailed rust-lang discussion](https://internals.rust-lang.org/t/what-does-unsafe-mean/6696) with Patina specific
principles and applications.

This document expects the reader has a general understanding of the `unsafe` keyword in Rust and how the compiler
enforces it. The above documentation and the
[UEFI Memory Safety Case Studies](../../background/uefi_memory_safety_case_studies.md) provide good starting points.

## Unsafe Philosophy

As a general principle, Patina splits the idea of safety into two categories: software safety and hardware safety.

|                   | Software Safety| Hardware Safety|
|-------------------|----------------|----------------|
| Compiler Enforced | ✅            | ❌             |
| Preconditions     | ✅            | ✅             |
| Postconditions    | ✅            | ✅             |
| Invariants        | ✅            | ❌             |

Software safety is the set of things the compiler can verify and/or the programmer can verify in a pure software
environment. Hardware safety is the set of things that the compiler cannot verify, the programmer may be able to verify,
and that interacts with hardware in a direct way.

Patina splits the idea of safety into these two categories to delineate usage of the unsafe keyword: the general Rust
guidance can be applied for software safety, but hardware safety expands beyond the bounds of what software can enforce.

Below are the breakdowns of the different unsafe usages and how we view software vs hardware safety here.

### Unsafe Code Blocks

Unsafe code blocks are not given much discussion here because the compiler will enforce what operations require the
unsafe block and cargo-clippy will enforce that unsafe code blocks are only used in cases where the compiler enforces
it. As general guidance: write as few unsafe operations as possible, constrain them underneath safe abstractions, and
document the preconditions, postconditions, and invariants, as applicable.

There is no distinction between hardware and software safety here.

### Unsafe Functions

Unsafe functions are the programmer's choice on whether to declare a given function as unsafe. An unsafe function has
a set of preconditions, postconditions, and/or invariants that must be met in order to use the function safely. The
presence of unsafe code blocks inside a function *does not* mean that the function must be declared unsafe; only if
the function cannot guarantee the safety of the unsafe code blocks within it without a contract with the caller should
a function be marked as unsafe.

For software safety, all of the above holds true. If software must guarantee a pre/postcondition or an invariant in
order to safely use a function, e.g.

```rust
/// Returns a reference to the element at the specified index without performing bounds checking.
///
/// # Parameters
/// - `index`: The position of the element to retrieve.
///
/// # Returns
/// A reference to the element at the given index.
///
/// # Safety
/// Calling this function with an out-of-bounds index is undefined behavior.
/// The caller must ensure that `index` is within the bounds of the collection.
unsafe fn get_element_unchecked<T>(slice: &[T], index: usize) -> &T {
  // SAFETY: Caller must ensure that index < slice.len()
  unsafe { slice.get_unchecked(index) }
}
```

In this case, the invariant is that `index` is within the bounds of `slice`. It is up to the programmer to decide how
this interface should be defined and whether the function is unsafe. For example, the function could have been written
as:

```rust
/// Returns an Option containing a reference to the element at the specified index without performing bounds checking.
///
/// # Parameters
/// - `index`: The position of the element to retrieve.
///
/// # Returns
/// An Option containing a reference to the element at the given index.
///
/// # Safety
/// Calling this function with an out-of-bounds index is undefined behavior.
/// The caller must ensure that `index` is within the bounds of the collection.
fn get_element_unchecked<T>(slice: &[T], index: usize) -> Option<&T> {
  if index >= slice.len() {
    return None;
  }
  // SAFETY: Caller must ensure that index < slice.len()
  unsafe { Some(slice.get_unchecked(index)) }
}
```

In this version, the function is no longer unsafe, despite containing an unsafe code block within it, because it no
longer has an invariant; the function itself is able to manage whether its inputs are valid.

Whenever possible, programmers should write safe functions that validate all preconditions, postconditions, and
invariants. Of course, this is not always possible, in which case unsafe functions are called for.

For hardware safety, we don't have invariants, necessarily. For example, in writing a system register, there is no
invariant that must hold true for that operation. There may be preconditions or postconditions, but not an invariant.

One simple example of hardware safety is the invalidate cache instruction on x86_64 that invalidates caches without
writing them back. This operation can be very dangerous if not called at a proper time, but nothing about the software
can change that. In Patina, this is hardware safety and does not warrant an unsafe function.

We may define a wrapper function for it as:

```rust,no_run
# use std::arch::asm;
fn invalidate_caches() {
  // SAFETY: This is an architecturally defined way to invalidate caches
  unsafe { asm!("invld") }
}
```

The concept of software safety could come into play, say in the below unsafe example:

```rust,no_run
# use std::arch::asm;
/// Invalidates the cache without writing them back. Also set a memory region to 4.
///
/// # Parameters
/// - `ptr`: The pointer to the memory region to set to 4.
///
/// # Safety
/// The caller must ensure `ptr` points to a valid memory region
unsafe fn invalidate_caches_and_set_ptr_to_4(ptr: *mut u8) { 
  // SAFETY: This is an architecturally defined way to invalidate caches
  unsafe { asm!("invld"); }

  // Better set this ptr to 4!
  // SAFETY: See function comment, caller assumes responsibility
  unsafe { ptr.write(4); }
}
```

However, the function could be made safe again by using the standard library to
validate assumptions (or validating the assumptions ourselves):

```rust,no_run
# use std::arch::asm;
fn invalidate_caches_and_set_ptr_to_4(mut ptr: Box<u8>) { 
  // SAFETY: This is an architecturally defined way to invalidate caches
  unsafe { asm!("invld"); } 

  // Better set this ptr to 4!
  *ptr = 4;
}
```

A more complex example, is writing the CR3 register on x64 systems.

```rust,no_run
# use std::arch::asm;
/// Installs a page table.
///
/// # Parameters
/// - `cr3`: The address of the page table root to install.
///
/// # Safety
/// The caller must ensure `cr3` is the address of a valid page table
unsafe fn install_page_table(cr3: u64) {
  // SAFETY: This is an architecturally defined operation to write the page table root.
  unsafe {
      asm!("mov cr3, {0}", in(reg) cr3)
  }
}
```

There is a precondition here that `cr3` must be a u64 that is the address of a valid page table. This operation is
unsafe because all assembly is unsafe in Rust. This is more complicated than the previous example because we are taking
a u64 that the hardware will treat as a pointer. However, software never dereferences this. So it falls into the
category of hardware safety. The software cannot validate that the hardware will accept this as a valid page table, no
matter what the software has done to validate it on its end. The software also does not dereference this u64 as a
pointer, the hardware does. The u64 does not have to be mapped (as it is the address providing the mappings to the
hardware). So Rust's memory safety cannot be violated by this operation. Certainly the system can become unusable,
if this did not point to a page table the hardware understands or if the mappings are incorrect. But that is the
hardware safety aspect. When viewed through this lens, the function can be a safe function, because all a caller could
do is provide the following safety comment:

```rust,ignore
// SAFETY: Hopefully this works!
unsafe { install_page_table(cr3);}
```

However, when viewed from the lifetime lens, this does become a software safety issue. The caller must guarantee that
this u64 provided, when treated as a pointer, will live for the lifetime of the time it is written to CR3. If this
gets reallocated, we have just violated both software and hardware safety. As such, the function is marked unsafe and
should be given the appropriate function comment describing the caller expectations.

There are other assumptions the software can validate to make this more likely to succeed, even if it cannot predict
exactly how the hardware will process this write.

This function could also be written to have a safe abstraction over it, e.g.:

```rust,no_run
# use std::arch::asm;
type PageTableRoot = u64;

enum PageTableInstallError {
  /// The CR3 address is not 4KB-aligned
  NotAligned,
  /// The CR3 address exceeds the maximum physical address width
  ExceedsMaxPhysAddr,
  /// Reserved bits are set in CR3
  ReservedBitsSet,
  /// PCIDE bit set but CR4.PCIDE is not enabled
  PcidWithoutCr4Support,
}

struct PageTableOptions {
  /// Page-level cache disable (CR3 bit 4)
  pub cache_disable: bool,
  /// Page-level write-through (CR3 bit 3)
  pub write_through: bool,
  /// Preserve PCID TLB entries (CR3 bit 63, requires CR4.PCIDE=1)
  pub preserve_pcid: bool,
  /// PCID value (CR3 bits 11:0 when CR4.PCIDE=1)
  pub pcid: Option<u16>,
}

/// Installs a page table.
///
/// # Parameters
/// - `cr3`: The address of the page table root to install.
///
/// # Safety
/// The caller must ensure `cr3` is the address of a valid page table and lives for the lifetime of being written in
/// CR3.
unsafe fn write_cr3(cr3: u64) {
  // SAFETY: This is an architecturally defined operation to write the page table root.
  unsafe {
      asm!("mov cr3, {0}", in(reg) cr3)
  }
}

# fn not_aligned(cr3: &PageTableRoot) -> bool { false }
# fn exceeds_maximum_addr_width (cr3: &PageTableRoot) -> bool { false }

fn install_page_table(cr3: &'static PageTableRoot, options: PageTableOptions) -> Result<(), PageTableInstallError> {
  if not_aligned(cr3) {
    return Err(PageTableInstallError::NotAligned);
  }

  if exceeds_maximum_addr_width(cr3) {
    return Err(PageTableInstallError::ExceedsMaxPhysAddr);
  }

  // additional checks such as if any reserved bits are set, if CR4 is configured correctly, etc.

  // Parse options, apply them in CR4, etc, ensure they are self consistent

  // SAFETY: We have ensured the lifetime is static, that the reference is valid, and we have done our best to ensure
  // this is a valid page table
  unsafe { write_cr3(*cr3 as u64); }

  Ok(())
}
```

You can take this example a step farther and refactor `write_cr3` to use a safe `write_reg` macro. This macro is
declared as safe in Patina because writing a system register in and of itself is not necessarily unsafe. It falls into
the hardware safety. For a given case, say the CR3 case, we may create an unsafe abstraction on top that declares this
particular register write has memory safety implications that the caller must guarantee.

```rust,no_run
# use std::arch::asm;
macro_rules! write_sysreg {
  ($dest:ident, $value:expr) => {
    // SAFETY: We have a compile time guarantee that this is a valid register to write to
    unsafe {
      asm!(concat!("mov ", stringify!($dest), ", {value:x}"),
      value = in(reg) $value)
    }
  }
}

type PageTableRoot = u64;

enum PageTableInstallError {
    /// The CR3 address is not 4KB-aligned
    NotAligned,
    /// The CR3 address exceeds the maximum physical address width
    ExceedsMaxPhysAddr,
    /// Reserved bits are set in CR3
    ReservedBitsSet,
    /// PCIDE bit set but CR4.PCIDE is not enabled
    PcidWithoutCr4Support,
}

struct PageTableOptions {
  /// Page-level cache disable (CR3 bit 4)
  pub cache_disable: bool,
  /// Page-level write-through (CR3 bit 3)
  pub write_through: bool,
  /// Preserve PCID TLB entries (CR3 bit 63, requires CR4.PCIDE=1)
  pub preserve_pcid: bool,
  /// PCID value (CR3 bits 11:0 when CR4.PCIDE=1)
  pub pcid: Option<u16>,
}

/// Installs a page table.
///
/// # Parameters
/// - `cr3`: The address of the page table root to install.
///
/// # Safety
/// The caller must ensure `cr3` is the address of a valid page table and lives for the lifetime of being written in
/// CR3.
unsafe fn write_cr3(cr3_reg: u64) {
  // SAFETY: This is an architecturally defined operation to write the page table root.
  write_sysreg!(cr3, cr3_reg)
}

# fn not_aligned(cr3: &PageTableRoot) -> bool { false }
# fn exceeds_maximum_addr_width (cr3: &PageTableRoot) -> bool { false }

fn install_page_table(cr3: &'static PageTableRoot, options: PageTableOptions) -> Result<(), PageTableInstallError> {
  if not_aligned(cr3) {
    return Err(PageTableInstallError::NotAligned);
  }

  if exceeds_maximum_addr_width(cr3) {
    return Err(PageTableInstallError::ExceedsMaxPhysAddr);
  }

  // additional checks such as if any reserved bits are set, if CR4 is configured correctly, etc.

  // Parse options, apply them in CR4, etc, ensure they are self consistent

  // SAFETY: We have ensured the lifetime is static, that the reference is valid, and we have done our best to ensure
  // this is a valid page table
  unsafe { write_cr3(*cr3 as u64); }

  Ok(())
}
```

### Unsafe Traits and Impls

Patina follows the general guidance from Rust on unsafe traits and trait impls. As above, hardware safety should be
considered here: if a trait or trait impl would only be marked unsafe because it touches hardware directly, that need
not create an unsafe trait/impl. It is up to the programmer to determine whether the hardware access would violate
software safety and if so, list them as unsafe and document preconditions, postconditions, and invariants.

## Summary

The main distinction between software safety and hardware safety is that software safety is complex and has many
possible safe paths that can be validated by the compiler or a programmer. In hardware safety, there is only one safe
path: interacting with the hardware as architecturally defined. Pragmatically speaking, it does not add value to
propagate unsafe higher than the code block level when dealing with hardware safety unless it intersects with
software safety.
