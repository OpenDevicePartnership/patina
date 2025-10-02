# RFC: `Remove Atomic from Patina`

This RFC proposes removing the use of `Atomic` operations from Patina in favor of other forms of mutual exclusion.

## Change Log

Initial Revision.

- 2025-10-01: Initial RFC created.

## Motivation

Presently `core::sync::atomic` module types are used in several locations in Patina to allow for thread/interrupt-safe
internal mutability (often to satisfy the rust compiler more than to actually provide additional safety). While these
primitives provide a relatively simple approach to managing concurrency within Patina, they have two significant
drawbacks:

1. **Compatibility** - Atomics require the use of special processor instructions. Not all architectures support these
instructions, or may have issues with them (especially for early-in-development silicon). Use of atomics limits the
potential portability of Patina.
2. **Performance** - Executing atomic instructions typically has a performance impact. In a single-cpu interrupt-only
model such as UEFI mutual exclusion can be accomplished via interrupt disable which may have less of a performance
impact in the UEFI context.

## Technology Background

The general topic of concurrency and the use of atomic operations therein is a large one. A simple primer is available
on Wikipedia here: [https://en.wikipedia.org/wiki/Linearizability#Primitive_atomic_instructions](https://en.wikipedia.org/wiki/Linearizability#Primitive_atomic_instructions).

The [`core::sync::atomic`](https://doc.rust-lang.org/core/sync/atomic/) module is part of core rust. It provides a set
of atomic types that implement primitive shared-memory communications between threads.

When it comes to concurrency, UEFI is a "simple single-core with timer interrupts" model. This means that (at least with
respect to core UEFI APIs implemented by Patina) that the need for mutual exclusion within UEFI is primarily to guard
against uncontrolled concurrent modification of memory shared between code and an interrupt handler that interrupts that
code. More details on UEFI support for eventing and interrupts is described in [Event, Timer, and Task Priority Services](https://uefi.org/specs/UEFI/2.11/07_Services_Boot_Services.html#event-timer-and-task-priority-services).

In the traditional EDK2 C reference core, concurrency is handled with interrupt control rather than with atomic
instructions.

## Goals

The primary goal of this RFC is to eliminate atomics from Patina to improve portability and performance.

## Requirements

1. Remove Atomics and replace with alternative concurrency protections using interrupt management.
2. Revisit concurrency usage within Patina and remove unnecessary nested concurrency protection where it makes sense to
do so.

## Unresolved Questions

- For adv_logger, atomic compare-exchange instructions are used to negotiate logging with external agents (such as
loggers running in MM). It's not clear how to address this use case.

- What are the right alternative concurrency mechanisms? Interrupt control seems the obvious one; but are there others?

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

- Why is this design the best in the space of possible designs?

The status quo of using atomics throughout the core has the drawbacks of lack of portability and performance impact as
noted in the motivation section above. Aside from using interrupts as the hardware basis for concurrency, other
alternatives are not obvious.

- What other designs have been considered and what is the rationale for not choosing them?

Previously atomics were used in Patina because they were readily available with good language support and easy to use.
The alternatives approaches (of redesigning subsystems without concurrency primitives and moving to interrupt support
where concurrency protection is mandatory) were not considered primarily due to the complexity of implementation.

One possible alternative would be to leave the atomics in place in Patina, and use compiler options (e.g.
`outline-atomics` code gen parameter) to enable platforms to re-implement atomics without using hardware instructions if
desired. The drawback here is that the complexity of implementing safe concurrency primitives that are alternatives to
hardware implementations rests on the integrator; and "normal platforms" that use the atomic hardware primitives are
still subject to the potential performance implications of atomics.

## Rust Code Design

There are several areas where atomic primitives are used in Patina. The following describes their usage and the planned
alternatives.

1. The `tpl_lock.rs` module uses atomic instructions to implement locks for concurrency protection before the eventing
subsystem and TPL support are ready. Here the atomics should be removed and replaced with an interrupt-based
concurrency approach prior to TPL availability. Once TPL support is fully enabled, tpl_lock should use TPL to control
concurrency.
2. The `patina_internal_collections` module uses atomics to wrap node pointers within the BST and RBT collection
implementations. These should simply be reworked to remove the atomics, with concurrency issues handled outside the
collection type.
3. The `adv_logger` module uses atomics to share memory with code running outside the patina context (e.g. in the MM
context). This is a rather unique requirement; since it requires agreement about concurrency with code that is not in
Patina and likely not written rust. One of the open questions above is how to handle this scenario; perhaps making
sharing the log with outside agents via hardware atomics an opt-in feature.
4. The `patina_debugger` uses atomics for POKE_TEST_MARKER; this can be replaced with a non-atomic volatile marker.
5. The `event` module uses atomics to track the CURRENT_TPL, SYSTEM_TIME and EVENT_NOTIFIES_IN_PROGRESS global state
for the event subsystem. This global state can be protected with interrupt-based concurrency protection.
6. The `misc_boot_services` module uses atomics for tracking global protocol pointer installation. If this concurrency
protection is in fact required, this global state can be protected with interrupt-based concurrency protection (such as
`tpl_lock` as reworked above).
7. The `memory_attributes_protocol` module uses atomics for tracking the handle and interface for the global memory
attribute protocol instance. If this concurrency protection is in fact required, this global state can be protected
with interrupt-based concurrency protection (such as `tpl_lock` as reworked above).
8. The `config_tables` module uses atomics for tracking the global pointer to the Debug Image Info Table and the Memory
Attributes Table. If this concurrency protection is in fact required for these objects, they can be protected with
interrupt-based concurrency protection (such as `tpl_lock` as reworked above).
9. The `boot_services` and `runtime_services` modules in `patina_sdk` use atomics to store the global pointer to the
corresponding services table. If this concurrency protection is in fact required, this global state can be protected
with interrupt-based concurrency protection (such as `tpl_mutex` in the sdk).
10. The `performance` module in `patina_sdk` use atomics to store global state (such as image count and configuration).
If this concurrency protection is in fact required, this global state can be protected with interrupt-based concurrency
protection (such as `tpl_mutex` in the sdk).

In addition to the above, there is a large amount of test code that uses atomics. Modifications of test code are not in
view since the primary drawbacks being addressed in this RFC (portability and performance) largely don't apply to unit
tests executing in the build environment.

It is possible that introduction of a new concurrency primitive based solely on interrupt manipulation
(independently of TPL) to live alongside `tpl_lock` may be beneficial in addressing some of the above usages of
atomics. Desirability of this potential approach will be determined as part of implementation of the RFC.

## Guide-Level Explanation

In general, the external APIs of Patina are unaffected by this proposed RFC; so no external guide to usage is needed.
This RFC serves as documentation for the motivation behind the design; module documentation on the various concurrency
primitives (such as `tpl_lock` and `tpl_mutex`) serve as engineering documentation for those modules.
