# Synchronization

UEFI does not support true multi-threaded operation; in general, all interaction
with the Patina DXE Core is expected to take place on a single processor thread.
UEFI does permit that single thread to have multiple "tasks" executing
simultaneously at different "Task Priority Levels[^events_and_tpl]."

Routines executing at a higher TPL may interrupt routines executing at a lower
TPL. Both routines may access Patina DXE Core Services, so global state in the
Patina DXE Core, such such as the protocol database, event database, dispatcher
state, etc. must be protected against simultaneous access.

The primary way this is implemented in the Patina DXE Core is via the `TplMutex`
structure.

[^events_and_tpl]: See [Event, Timer, and Task Priority Services](events.md#event-timer-and-task-priority-services) elsewhere in this book, as well as
the [UEFI Specification Section 7.1](https://uefi.org/specs/UEFI/2.10_A/07_Services_Boot_Services.html#event-timer-and-task-priority-services).

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
accquire the lock (because it will attempt to raise the TPL to a lower level,
which is an error). But setting a `tpl_lock_level` to a high TPL level will
prevent other (unrelated) usage of that TPL, potentially reducing system
responsiveness. It is recommended to set the `tpl_lock_level` as low as possible
while still guaranteeing that the no access to the lock will be attempted at a
higher TPL level.
```

### TplMutex - Locking and Reentrancy

The second mutual exclusion mechanism used by `TplMutex` is a  flag to control
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
another agent but the caller can handle not acquiring the lock.

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
