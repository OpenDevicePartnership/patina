# RFC: `Remove Atomic from Patina`

This RFC proposes removing the use of `Atomic` operations from Patina in favor of other forms of mutual exclusion.

## Change Log

Initial Revision.

- 2025-10-01: Initial RFC created.
- 2025-10-15: Updated with feedback from PR, moved to FCP.
- 2025-10-23: Updated with learnings from initial draft implementation.

## Motivation

Presently `core::sync::atomic` module types are used in several locations in Patina to allow for thread/interrupt-safe
internal mutability (often to satisfy the rust compiler more than to actually provide additional safety). While these
primitives provide a relatively simple approach to managing concurrency within Patina, they have two significant
drawbacks:

1. **Compatibility** - Atomics require the use of special processor instructions. Not all architectures support these
instructions, or may have issues with them (especially for early-in-development silicon). Use of atomics limits the
potential portability of Patina.
2. **Performance** - Executing atomic instructions typically has a performance impact. In a single-cpu, interrupt-only
model such as UEFI mutual exclusion can be accomplished via interrupt disable which may have less of a performance
impact in the UEFI context.

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

1. Remove Atomics from Patina core and replace with alternative concurrency protection structures using interrupt
management.
2. Revisit concurrency usage within Patina and remove unnecessary nested concurrency protection where it makes sense to
do so.
3. Remove Atomics from optional Patina components except those that have unique requirements that mandate the use of
Atomics.
4. Update documentation with design guidance on avoiding atomic usage.

## Unresolved Questions

- For adv_logger, atomic compare-exchange instructions are used to negotiate logging with external agents (such as
loggers running in MM). It's not clear how to address this use case. *Resolution* no change, retain compare-exchange in
adv_logger as a special case.

- What are the right alternative concurrency mechanisms? Interrupt control seems the obvious one; but are there others?
*Resolution* proceed with interrupt control as the primary concurrency mechanism in Patina.

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

### New Non-Atomic Locking Primitives

This RFC proposes the implementation of new "interrupt-only" locking primitives to supplement the existing `tpl_lock`
primitives presently implemented. Two new primitives are proposed that have similar semantics to existing rust mutex
idioms (such as those in `tpl_lock` or from the `spin::mutex` crate) - i.e., acquiring a lock will return a "Guard"
instance, and the lock is released when the "Guard" object is dropped.

`Mutex<T>`  - This lock will use a volatile bool with temporary suspension of interrupts to detect and panic on
reentrancy to ensure mutual exclusion. Interrupts will only be suspended while attempting to acquire the lock; once the
lock is required interrupts will be enabled for the lifetime of the resulting Guard object.
`InterruptMutex<T>` - This lock will suspend interrupts for the lifetime of the corresponding Guard object. This allows
the critical section protected by the guard to execute without potentially being interrupted. As with `Mutex`, a
volatile bool will be used to detect and panic on reentrancy to ensure mutual exclusion.
`TplMutex<T>` - This lock will be reworked to use `Mutex`/`InterruptMutex` for atomicity, but will continue to manage
TPL as part of the locking implementation. This is expected to be the most prevalent type of lock in Patina due to the
interactions between TPL and interrupt management.

A new `lock` module will be implemented within the patina core to contain the implementation of these new primitives.
Patina components needing lock functionality are expected to use the `TplMutex` within the Patina SDK. Components
are discouraged from directly interacting with interrupts as a means of implementing mutual exclusion.

### Removal of Atomics code in Patina

There are several areas where atomic primitives are used in Patina. The following describes their usage and the planned
alternatives.

1. The `tpl_lock.rs` module uses atomic instructions to implement locks for concurrency protection before the eventing
subsystem and TPL support are ready. This is proposed to be removed in favor of the new locking primitives described in
the previous section.
2. The `patina_internal_collections` module uses atomics to wrap node pointers within the BST and RBT collection
implementations. These should simply be reworked to remove the atomics, with concurrency issues handled outside the
collection type.
3. The `adv_logger` module uses atomics to share memory with code running outside the patina context (e.g. in the MM
context). This is a rather unique requirement; since it requires agreement about concurrency with code that is not in
Patina and likely not written rust. As this is a component and not part of the patina core, atomics will be retained
for this unique requirement.
4. The `patina_debugger` uses atomics for POKE_TEST_MARKER; this can be replaced with a non-atomic volatile marker.
5. The `event` module uses atomics to track the CURRENT_TPL, SYSTEM_TIME and EVENT_NOTIFIES_IN_PROGRESS global state
for the event subsystem. This global state can be protected with the new locking primitives described in the previous
section.
6. The `misc_boot_services` module uses atomics for tracking global protocol pointer installation. This can be protected
with the new locking primitives described in the previous section, or with e.g. `OnceCell` as appropriate.
7. The `memory_attributes_protocol` module uses atomics for tracking the handle and interface for the global memory
attribute protocol instance. This can be protected with the new locking primitives described in the previous section,
or with e.g. `OnceCell` as appropriate.
8. The `config_tables` module uses atomics for tracking the global pointer to the Debug Image Info Table and the Memory
Attributes Table. This can be protected with the new locking primitives described in the previous section,
or with e.g. `OnceCell` as appropriate.
9. The `boot_services` and `runtime_services` modules in the SDK use atomics to store the global pointer to the
corresponding services table. This can be protected with `tpl_mutex`, or with e.g. `OnceCell` as appropriate.
10. The `performance` module in the SDK use atomics to store global state (such as image count and configuration).
This can be protected with the `tpl_mutex` implementation in the sdk or with `OnceCell` as appropriate.

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
