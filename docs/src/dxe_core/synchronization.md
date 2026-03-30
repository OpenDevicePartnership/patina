# Synchronization

UEFI does not support true multi-threaded operation; in general, all interaction
with the Patina DXE Core is expected to take place on a single processor thread.
UEFI does permit that single thread to have multiple "tasks" executing
simultaneously at different "Task Priority Levels[^events_and_tpl]."

Routines executing at a higher TPL may interrupt routines executing at a lower
TPL. Both routines may access Patina DXE Core Services, so global state in the
Patina DXE Core, such as the protocol database, event database, dispatcher
state, etc. must be protected against data races caused by simultaneous access
from event callbacks running at different TPL levels.

The primary way this is implemented in the Patina DXE Core is via the `TplMutex`
structure.

[^events_and_tpl]: See [Event, Timer, and Task Priority Services](events.md#event-timer-and-task-priority-services) elsewhere in this book, as well as
the [UEFI Specification Section 7.1](https://uefi.org/specs/UEFI/2.11/07_Services_Boot_Services.html#event-timer-and-task-priority-services).

## TplMutex

`TplMutex` implements mutual exclusion for the Patina DXE Core using semantics
very similar to the Rust [sync::Mutex](https://doc.rust-lang.org/std/sync/struct.Mutex.html).
Each `TplMutex` has a type parameter which represents the data that it is
protecting. The data can only be accessed through the `TplGuard` objects
returned from `lock()` and `try_lock()` methods on the TplMutex.

There are two mutual exclusion mechanisms that protect the data guarded by
`TplMutex`:

### TplMutex - TPL interactions

The first mutual exclusion mechanism used by `TplMutex` is the TPL - When a
`TplMutex` is created, it takes a `tpl_lock_level` parameter that specifies a
TPL level. When the a `TplMutex` is locked, the TPL is raised to that level;
this prevents any code at that TPL level or lower from executing. This ensures
that access to the lock is not attempted by other code, and helps avoid deadlock
scenarios.

```admonish warning
Care must be taken when selecting the `tpl_lock_level` for a `TplMutex`. Code
executing at a TPL higher than the `TplMutex` will panic if it attempts to
acquire the lock (because it will attempt to raise the TPL to a lower level,
which is an error). But setting a `tpl_lock_level` to a high TPL level will
prevent other (unrelated) usage of that TPL, potentially reducing system
responsiveness. It is recommended to set the `tpl_lock_level` as low as possible
while still guaranteeing that the no access to the lock will be attempted at a
higher TPL level.
```

### TplMutex - Locking and Reentrancy

The second mutual exclusion mechanism used by `TplMutex` is a flag to control
access to the lock. To acquire the `TplMutex`, the flag must be clear to
indicate that the lock is not owned by any other agent. There is a significant
difference between the `TplMutex` and `sync::Mutex` - while `sync::Mutex` will
simply block on a call to `lock()` when the lock is owned, `TplMutex` will panic
if an attempt is made to call `lock()` when it is already owned.

```admonish warning
Reentrant calls to `lock()` are not permitted for `TplMutex`.
```

This is by design: `sync:Mutex` presumes the existence of a multi-threaded
environment where the owner of the lock might be another thread that will
eventually complete work and release the lock. In the `sync:Mutex` context a
blocking `lock()` call makes sense since it is reasonable to expect that the
lock will be released by another thread. In the UEFI `TplMutex` context,
however, there is no multi-threading, only interrupts on the same thread at
higher TPL. For a re-entrant call to `lock()` to occur, an attempt to call
`lock()` must have been made from the same or higher TPL level than the original
call to `lock()`. This means that if the re-entrant call to `lock()` were to
block, control would never return to the original caller of `lock()` at the same
or lower TPL. So in the UEFI context, all reentrant calls to `lock()` are
guaranteed to deadlock. Note that [`sync::Mutex` behavior](https://doc.rust-lang.org/std/sync/struct.Mutex.html#method.lock)
is similar if `lock()` is attempted on the same thread that already holds the
mutex.

The `try_lock()` routine in `TplMutex` allows a lock to be attempted and fail
without blocking; this can be used for scenarios where a lock might be held by
another agent in a lower TPL but the caller can handle not acquiring the lock,
or in scenarios where a call is re-entrant at the same TPL.

## TplGuard

When `lock()` is called on `TplMutex` a `TplGuard` structure is returned that
provides access to the locked data. The `TplGuard` structure implements `Deref`
and `DerefMut`, which allows access to the underlying data:

```rust,ignore
use crate::tpl_mutex::TplMutex;
use r_efi::efi;
let tpl_mutex = TplMutex::new(efi::TPL_HIGH_LEVEL, 1_usize, "test_lock");

*tpl_mutex.lock() = 2_usize; //deref to set
assert_eq!(2_usize, *tpl_mutex.lock()); //deref to read.
```

In addition, the when the `TplGuard` structure returned by `lock()` goes out of
scope or is dropped, the lock is automatically released:

```rust,ignore
use crate::tpl_mutex::TplMutex;
use r_efi::efi;
let tpl_mutex1 = TplMutex::new(efi::TPL_HIGH_LEVEL, 1_usize, "test_lock");

let mut guard1 = tpl_mutex1.lock(); //mutex1 locked.
*guard1 = 2_usize; //set data behind guard1
assert_eq!(2_usize, *guard1); //deref to read.
assert!(tpl_mutex1.try_lock().is_err()); //mutex1 still locked.
drop(guard1); //lock is released.
assert!(tpl_mutex1.try_lock().is_ok()); //mutex1 unlocked and can be acquired.
```

## General Guidelines for Synchronization within Patina

This section documents some good rules of thumb for synchronization design
within Patina.

1. If a type needs to be `Sync`/`Send`, that means it is expected to be shared
across contexts - Rust documentation refers to these contexts as threads, but
within UEFI this generally refers to event callbacks running on the same CPU
thread - see discussion above. The primary place that the Rust compiler requires
this in the Patina context is global shared state, such as the protocol/event
databases or global allocator state. The default safe way to do this is a
`TplMutex` as described above that ensures that data races and deadlock do not
occur on access to this global state.

2. Do not use non-TPL aware synchronization primitives such as `spin::mutex` or
`spin::rwlock`. In the UEFI/Patina threading model, these primitives are prone
to deadlock because they assume that contention for the lock from one context
will not block execution of the context currently holding the lock. But as
described in the previous section of this chapter, that is not the case for
UEFI: if code running at a low TPL holds a lock and is interrupted by code in an
event callback at a higher TPL that tries to acquire the lock, deadlock will
occur. `TplMutex` helps to ensure this doesn't happen by panicking on an attempt
to acquire the lock in this case, helping the developer identify and resolve the
design issues that would otherwise lead to deadlock.

3. When designing a `TplMutex` to guard shared data, select the highest TPL that
the shared state will possibly be accessed at as the TPL level associated with
the mutex. Typically this will be `TPL_NOTIFY` as that is the highest level that
the UEFI specification normally allows for general usage - see [Table 7.2](https://uefi.org/specs/UEFI/2.11/07_Services_Boot_Services.html#tpl-usage)
in the UEFI spec. Designs must guarantee the invariant that there will be no
attempts to access the `TplMutex` at a higher TPL level than associated with the
mutex, and this will be enforced with an `assert`. This is known as "TPL
Inversion," and if it were allowed, it would mean that higher TPL levels could
break mutual exclusion and cause data races.
[Table 7.3](https://uefi.org/specs/UEFI/2.11/07_Services_Boot_Services.html#tpl-restrictions)
lists the TPL restrictions associated with various core services and common protocols.

   ```admonish warning
   Note: it is important to understand that the TPL level associated with a
   `TplMutex` is not the same thing as the TPL level associated with an event
   callback routine. The TPL level associated with an event callback determines
   the TPL level at which the event callback is permitted to run and can be
   thought of as the "ground state" TPL that the event callback executes at. The
   callback is permitted to acquire a TplMutex at a higher level than the event
   callback is running at, and the TPL will be raised for the duration that the
   TPL guard is owned to prevent data races with event callbacks running at the
   higher context.
   ```

4. Care should be taken to ensure that `TplMutex` usages are scoped so that the
critical sections are as narrow as possible. This is especially true if
accessing shared data from a TPL context that is at a lower TPL than the
`TplMutex` lock level since holding the lock at a higher TPL for long periods
will starve event servicing at or below the `TplMutex` lock level as long as the
guard is active.

   Prefer:

   ```rust,ignore
   {
       let guard = my_tpl_mutex.lock();
       // TPL raised to level associated with my_tpl_mutex
       guard.mutation();
   }
   // mutex dropped, TPL restored to the base TPL level for the event callback.
   long_running_computation();
   // re-acquire mutex
   my_tpl_mutex.lock().mutation2();
   ```

   instead of:

   ```rust,ignore
   let guard = my_tpl_mutex.lock();
   guard.mutation();
   //mutex held and TPL stays high during long_running_computation
   long_running_computation();
   guard.mutation2();
   ```

5. If the design calls for interior mutability on data that is _not_ shared between
contexts, use a standard Rust interior mutability primitive (i.e. `UnsafeCell`
and its derivatives). Do not use `TplMutex` for interior mutability on non-shared
data. A good rule of thumb is that if your usage doesn't require `Sync`/`Send`,
then you don't need a `TplMutex`.

6. The UEFI spec APIs often use constructs like `context: *mut c_void` to share
data between contexts. When implementing FFI interfaces to support these API
contracts, `TplMutex` should be used to guard shared data accessed via these
context raw pointers even though the raw pointers are not required to be `Sync`/
`Send` by the compiler. Whether data races can occur and how they are prevented
should be documented as part of the safety comments for usage of the raw context
pointer. Sometimes the `context` pointer is known to be unique to the event
callback and never accessed from other contexts, in which case a `TplMutex` is
not required.

7. Direct `raise_tpl` and `restore_tpl` calls should be avoided. Directly
manipulating the TPL decouples the mutual exclusion primitives from the data
that is being protected and makes it hard to associate the TPL requirements with
the data synchronization model of the code.

8. Care should be taken to avoid violating UEFI spec caller restrictions on TPL
as described in
[Table 7.3](https://uefi.org/specs/UEFI/2.11/07_Services_Boot_Services.html#tpl-restrictions)
of the UEFI spec. For example, the following usage of `TplMutex` would be an
error:

   ```rust,ignore
   let my_tpl_mutex = TplMutex::<Data>::new(efi::TPL_NOTIFY, Data::new(), "my lock");
   let _guard = my_tpl_mutex.lock(); //TPL raised to NOTIFY while _guard is in scope.
   let acpi_services = locate_acpi_table_protocol();
   acpi_services.install_acpi_table(); //BUG: UEFI spec requires invocation at < TPL_NOTIFY
   ```

As with any set of guidelines, exceptions to the above may be required for
specific cases; these should include design rationale for the departure from
these rules of thumb. For example, it might be possible to use a non-TPL
synchronization primitive that only uses `try_lock` to avoid deadlock and is
designed to handle failure to acquire the lock in a non-fatal manner.
