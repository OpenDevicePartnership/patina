# RFC: `Remove Atomic from Patina`

This RFC proposes removing the use of `Atomic` operations for interior mutability from Patina in favor of other forms of
interior mutability.

## Change Log

Revised based on performance data collected on interrupt-based locking which indicates it is not as performant as
expected. RFC updated to propose the removal of Atomics for regular interior mutability, but not for concurrency
protection.

- 2025-10-01: Initial RFC created.
- 2025-10-15: Updated with feedback from PR, moved to FCP.
- 2025-10-23: Updated with learnings from initial draft implementation.
- 2025-11-07: Revised based on measured performance data that indicates interrupt-based locking is not as performant.

## Motivation

Presently `core::sync::atomic` module types are used in several locations in Patina to allow for interior mutability
(often to satisfy the rust compiler rather than to actually provide concurrency safety). Atomics are also used in Patina
to implement concurrency protection, especially for global static state.

While these Atomic primitives provide a relatively simple approach to interior mutability, they have two significant
drawbacks:

1. **Compatibility** - Atomics require the use of special processor instructions. Not all architectures support these
instructions, or may have issues with them (especially for early-in-development silicon). Use of atomics limits the
potential portability of Patina. There are options, such as enabling `outline-atomics` in the underlying LLVM codegen
and supplying direct replacements for atomic implementations, or using crates like [portable_atomic](https://crates.io/crates/portable-atomic).
But minimizing the overall uses of Atomics where feasible will help to ensure that the scope
of any future portability efforts around Atomics is smaller.
2. **Performance** - Executing atomic instructions typically has a performance impact. For interior mutability, using
non-atomic approaches (such as `Cell`/`RefCell`/`OnceCell`) can improve performance.

The following table gives a general sense of the impact of removing atomics from the Red-
Black Tree implementation that is used as the backing collection for the GCD. These were collected using `cargo make
bench -p patina_internal_collections` on relatively recent aarch64 and x64 hardware:

| Operation         | Architecture | Speed Improvement |
| :---------------- | :----------: | :---------------: |
| add/rbt/32bit     | aarch64      | 49%               |
| add/rbt/32bit     | x64          | 35%               |
| add/rbt/128bit    | aarch64      | 49%               |
| add/rbt/128bit    | x64          | 30%               |
| add/rbt/384bit    | aarch64      | 48%               |
| add/rbt/384bit    | x64          | 32%               |
| delete/rbt/32bit  | aarch64      | 50%               |
| delete/rbt/32bit  | x64          | 34%               |
| delete/rbt/128bit | aarch64      | 58%               |
| delete/rbt/128bit | x64          | 31%               |
| delete/rbt/384bit | aarch64      | 51%               |
| delete/rbt/384bit | x64          | 27%               |
| search/rbt/32bit  | aarch64      | 8%                |
| search/rbt/32bit  | x64          | 10%               |
| search/rbt/128bit | aarch64      | 5%                |
| search/rbt/128bit | x64          | 8%                |
| search/rbt/384bit | aarch64      | 6%                |
| search/rbt/384bit | x64          | 3%                |

While these performance benchmarks are narrow and somewhat synthetic, they do illustrate a material performance
improvement from removal of atomics.

For situations where concurrency protection is required (such as for global static state), interrupt-based control could
be used to replace the current Atomic concurrency primitives. We conducted performance measurements of overall boot
performance replacing all the Atomic concurrency primitives with interrupt control, but system boot performance declined
by a small but measureable amount using interrupt-based concurrency primitives instead of atomics.

## Technology Background

The general topic of concurrency and the use of atomic operations therein is a large one. A simple primer is available
on Wikipedia here: [https://en.wikipedia.org/wiki/Linearizability#Primitive_atomic_instructions](https://en.wikipedia.org/wiki/Linearizability#Primitive_atomic_instructions).

The [`core::sync::atomic`](https://doc.rust-lang.org/core/sync/atomic/) module is part of core rust. It provides a set
of atomic types that implement primitive shared-memory communications between threads.

When it comes to concurrency, UEFI is a "simple single-core with timer interrupts" model. This means that (at least with
respect to core UEFI APIs implemented by Patina) that the need for mutual exclusion within UEFI is primarily to guard
against uncontrolled concurrent modification of memory shared between code and an interrupt handler that interrupts that
code. More details on UEFI support for eventing and interrupts is described in [Event, Timer, and Task Priority Services](https://uefi.org/specs/UEFI/2.11/07_Services_Boot_Services.html#event-timer-and-task-priority-services).

Rust requires global static variables to be both `Send` and `Sync` to prevent data races and ensure safe access from
any thread. In Patina, this is largely accomplished via means of Atomics to provide conccurent-safe interior mutability
on the global state. In C, there are no such restrictions - it is up to the programmer to ensure that global data access
is safe from data races; but global state may be directly accessed without any protections. This means that in C, direct
access to global state can happen without requiring an atomic instruction.  In the traditional EDK2 C reference core,
concurrency is handled with interrupt control rather than with atomic instructions.

## Goals

The primary goal of this RFC is to reduce unnecessary atomics in Patina to improve portability and performance.

## Requirements

1. Remove unnecessary Atomics from Patina and replace with alternative interior mutability structures using Rust
primitives for interior mutability, such as `Cell` or `RefCell`.
2. Revisit concurrency usage within Patina and remove unnecessary nested concurrency protection where it makes sense to
do so.
3. Update documentation with design guidance on appropriate use of Atomics.

## Prior Art (Existing PI C Implementation)

The EDK2 C implementation of the core does not use atomics for concurrency protection. Where concurrency protections are
required, it uses the TPL subsystem to implement locking. The TPL implementation uses interrupt enable/disables as the
primary hardware concurrency protection mechanism.

```C
/**
  Raising to the task priority level of the mutual exclusion
  lock, and then acquires ownership of the lock.

  @param  Lock               The lock to acquire

  @return Lock owned

**/
VOID
CoreAcquireLock (
  IN EFI_LOCK  *Lock
  )
{
  ASSERT (Lock != NULL);
  ASSERT (Lock->Lock == EfiLockReleased);

  Lock->OwnerTpl = CoreRaiseTpl (Lock->Tpl);
  Lock->Lock     = EfiLockAcquired;
}

/**
  Releases ownership of the mutual exclusion lock, and
  restores the previous task priority level.

  @param  Lock               The lock to release

  @return Lock unowned

**/
VOID
CoreReleaseLock (
  IN EFI_LOCK  *Lock
  )
{
  EFI_TPL  Tpl;

  ASSERT (Lock != NULL);
  ASSERT (Lock->Lock == EfiLockAcquired);

  Tpl = Lock->OwnerTpl;

  Lock->Lock = EfiLockReleased;

  CoreRestoreTpl (Tpl);
}
```

## Alternatives

1. Retain the status quo: This has the drawbacks of lack of portability and performance impact as noted in the
motivation section above.
2. Remove _all_ use of Atomic, including those used for concurrency protection of global state. Prototype implementation
indicates that this is technically viable, but performance measurements on the prototype indicate that using interrupt-
control instead of atomics for concurrency protection results in small but measurable performance degradation.
3. Leave the atomics in place in patina, and use compiler features (e.g. `outline-atomics` or similar) to allow them
to be overridden with alternatives on platforms where perf/compat are issues. This is still possible with the proposed
design, which simply proposes the overall reduction of Atomics for non-concurrency cases.

## Rust Code Design

### Removal of Atomics code in Patina

There are several areas where atomic primitives are used in Patina. The following describes their usage and the planned
alternatives.

1. The `tpl_lock.rs` module uses atomic instructions to implement locks for concurrency protection before the eventing
subsystem and TPL support are ready. Instead of using an atomic lock flag, instead use a "bool" protected by the TPL
mechanism already in play.
2. The `patina_internal_collections` module uses atomics to wrap node pointers within the BST and RBT collection
implementations. These should simply be reworked to remove the atomics, with concurrency issues handled outside the
collection type.
3. The `patina_debugger` uses atomics for POKE_TEST_MARKER; this can be replaced with a non-atomic volatile marker.
4. The `misc_boot_services` module uses atomics for tracking global protocol pointer installation. Replace with e.g.
`OnceCell` as appropriate.
5. The `memory_attributes_protocol` module uses atomics for tracking the handle and interface for the global memory
attribute protocol instance. Replace with e.g. `OnceCell` as appropriate.
6. The `config_tables` module uses atomics for tracking the global pointer to the Debug Image Info Table and the Memory
Attributes Table. Replace with e.g. `OnceCell` as appropriate.
7. The `boot_services` and `runtime_services` modules in the SDK use atomics to store the global pointer to the
corresponding services table. Replace with e.g. `OnceCell` as appropriate.

As part of implementing this RFC, issues will be filed for all of the above items as appropriate to track the work of
implementing atomic removal and updating documentation as needed to describe the new primitives and recommended approach
to synchronization.

In addition to the above, there is a large amount of test code that uses atomics. Modifications of test code are not in
view for modification in this RFC since the primary drawbacks being addressed in this RFC (portability and performance)
largely don't apply to unit tests executing in the build environment.

## Guide-Level Explanation

In general, the external APIs of Patina are unaffected by this proposed RFC; so no external guide to usage is needed.
This RFC serves as documentation for the motivation behind the design; module documentation on the various concurrency
primitives (such as `tpl_lock` and `tpl_mutex`) serve as engineering documentation for those modules.
