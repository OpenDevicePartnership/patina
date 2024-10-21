# Unit Testing

As mentioned in [Testing](../testing.md), unit tests are written in the exact file that you
are working in. Tests are written in a conditionally compiled sub-module and any tests should be
tagged with `#[test]`.

``` rust
#[cfg(test)]
mod tests {

    #[test]
    fn test_my_functionality() {
        assert!(true);
    }
}
```

Since this conditionally compiled module is a sub-module of the module you are writing, it has
access to all private data in the module, allowing you to test public and private functions,
modules, state, etc.

## Unit Testing and UEFI

Due to the nature of UEFI, there tend to be a large amount of statics that exist for the lifetime
of the execution (such as the GCD in the DXE_CORE). This can make unit testing somewhat complex as
unit tests run in parallel, but if there exists some global static, it will be touched and
manipulated by multiple tests, which can lead to dead locks or the static data in a state that the
current test is not expecting. You can chose to follow any pattern to combat this, but the most
common we use is to create a global test lock.

## Global Test Lock

The easiest way we have found to control test execution to allow parallel execution for tests that
do not require global state, while forcing all other tests that do to run one-by-one is to create
a global state lock. The flow is that that the global state lock is acquired, global state is
reset, then the test is run. It is ultimately up to the test writer to reset the state for the
test. Here is a typical example that is used in the dxe_core itself:

```rust
mod test_support {
    static GLOBAL_STATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub fn with_global_lock(f: impl Fn()) {
        let _guard = GLOBAL_STATE_TEST_LOCK.lock().unwrap();
        f();
    }
}

#[cfg(test)]
mod tests {
    use test_support::with_global_lock;
    fn with_reset_state(f: impl Fn()) {
        with_global_lock(|| {
            // Reset the necessary global state here
            f();
        });
    }

    #[test]
    fn run_my_test() {
        with_reset_state(|| {
            // Run the actual tests
        })
    }
}
```
