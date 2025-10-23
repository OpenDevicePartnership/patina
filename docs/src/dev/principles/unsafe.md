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

Software safety is set of things the compiler can verify and/or the programmer can verify in a pure software
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

For example, take writing the CR3 register on x64 systems.

```rust
fn install_page_table(cr3: u64) -> () {
  // SAFETY: This is an architecturally defined operation to write the page table root.
  unsafe {
      asm!("mov cr3, {0}", in(reg) cr3);
  }

  ()
}
```

There is a precondition here that `cr3` must be a u64 that is the address of a valid page table. This operation is
unsafe because all assembly is unsafe in Rust. However, in Patina, this need not create an unsafe function.

e.g. the macro that writes system registers on ARM64 is not marked as unsafe. That is because it deals with hardware
safety, not software safety. There is nothing the compiler or programmer can do to change any outcome here or validate
anything ahead of time. The hardware is what will validate this and either work correctly or not. It does not provide
value to make the macro unsafe because all the programmer can do is have a safety comment that says

```rust
// SAFETY: This is a valid value to write to a system register and I hope it is right
```

From a software perspective, we have a safe function definition; it simply takes a u64 and writes it to a system
register. However, the hardware will decide whether or not this works, no matter what the safety from the software
side is. As such, in Patina, lack of hardware safety does not mandate an unsafe function. If software safety cannot
be guaranteed in addition to the hardware safety, then an unsafe function should be used.

e.g.

```rust
unsafe fn install_page_table(cr3: u64) -> () {
  // oops, forgot to add a mapping!
  let root_ptr = unsafe {cr3 as *const u64};
  unsafe {root_ptr.write(57)};

  // SAFETY: This is an architecturally defined operation to write the page table root.
  unsafe {
      asm!("mov cr3, {0}", in(reg) cr3);
  }

  ()
}
```

Now we have introduced something that is unsafe in the software context: a raw pointer write. There is now something
the caller can do to validate that `cr3` is safe in the software safety context, whereas it can't do anything about
the hardware safety. As such, it is now reasonable to mark this function unsafe.

### Unsafe Traits and Impls

Patina follows the general guidance from Rust on unsafe traits and trait impls. As above, hardware safety should be
considered here: if a trait or trait impl would only be marked unsafe because it touches hardware directly, that need
not create an unsafe trait/impl. It is up to the programmer to determine whether the hardware access would violate
software safety and if so, list them as unsafe and document preconditions, postconditions, and invariants.

## Summary

The main distinction between software safety and hardware safety is that software safety is complex and has many
possible safe paths that can be validated by the compiler or a programmer. In hardware safety, there is only one safe
path: interacting with the hardware as architecturally defined. As such, it does not add value to propagate unsafe
higher than the code block level when dealing with hardware safety unless it intersects with software safety.
