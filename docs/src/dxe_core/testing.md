# DXE Core Testing

Writing DXE Core tests follows all the same principles defined in the [Testing](../dev/testing.md) chapter, so if you
have not reviewed it yet, please do so before continuing. One of the reasons that [patina](https://github.com/OpenDevicePartnership/patina)
is split into multiple crates and merged the `patina_dxe_core` umbrella crate is to support code separation and ease of
unit testing. Support crates (in the `crates/*` folder) should not contain any static data used by the core. Instead,
they should provide the generic implementation details that the core uses to function. This simplifies unit tests, code
coverage, and the future possibility of extracting functionality to be used in additional cores (such as PEI, MM, etc).

The DXE Core supports all 4 types of testing mentioned in the Testing chapter; this includes on-platform unit tests.
Any function with the `patina_test` attribute will be consolidated and executed on any platform that uses the
`TestRunnerComponent` (unless specifically filtered out by the platform).

## Testing with Global State

The standout difference between typical testing as described in the testing chapter, is that the DXE core has multiple
static pieces of data that are referenced throughout the codebase. Since unit tests are ran in parallel, this means
that multiple tests may be manipulating this static data at the same time. This will lead to either dead-locks, panics,
or the static data being in an unexpected state for the test.

To help with this issue in the `patina_dxe_core` crate, a [test_support](https://github.com/OpenDevicePartnership/patina/blob/main/patina_dxe_core/src/test_support.rs)
module was added to make writing tests more convenient. The most important functionality in the module is the
`with_global_lock` function which takes your test closure / function as a parameter. This function locks a private
global mutex, ensuring you have exclusive access to all statics within the DXE Core.

``` admonish warning
It is the responsibility of the test writer to reset the global state to meet their expectations. It is **not** the
responsibility of the test writer to clear the global state once the test is finished.
```

## Examples

### Example 1

```rust
use crate::test_support::{with_global_lock, init_test_gcd};

#[test]
fn test_that_uses_gcd() {
    with_global_lock(|| {
        init_test_gcd(None);

        todo!("Finish the test");
    });
}
```

### Example 2

```rust
use crate::test_support::{with_global_lock, init_test_gcd};

fn with_gcd<F: Fn()>(gcd_size: Option<usize>, f: F) {
    with_global_lock(|| {
        init_test_gcd(gcd_size);
        f();
    })
}

#[test]
fn test1() {
    with_gcd(None, || {
        todo!("Write the test");
    });
}

#[test]
fn test2() {
    with_gcd(Some(0x1000), || {
        todo!("Write the test");
    });
}
```
