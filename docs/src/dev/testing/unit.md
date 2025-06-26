# Unit Testing

As mentioned in [Testing](../testing.md), unit tests are written in the same file as the code being tested. Tests
are placed in a conditionally compiled sub-module, and each test should be tagged with `#[test]`.

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_my_functionality() {
        assert!(true);
    }
}
```

Since this conditionally compiled module is a sub-module of the module you are writing, it has access to all
private data in the module, allowing you to test public and private functions, modules, state, etc.

## Unit Testing and UEFI

Due to the nature of UEFI, there tend to be a large number of statics that exist for the lifetime of execution
(such as the GCD in the Patina DXE Core). This can make unit testing complex, as unit tests run in parallel, but
if there exists some global static, it will be touched and manipulated by multiple tests, which can lead to
deadlocks or the static data being in a state that the current test is not expecting. You can choose any pattern
to combat this, but the most common is to create a global test lock.

## Global Test Lock

The easiest way to control test execution—allowing parallel execution for tests that do not require global state,
while forcing all others to run one-by-one—is to create a global state lock. The flow is: acquire the global state
lock, reset global state, then run the test. It is up to the test writer to reset the state for the test. Here is
a typical example used in the Patina DXE Core:

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
            // Test code here
        });
    }
}
```

```mermaid
---
config:
  layout: elk
  look: handDrawn
---
graph TD
    A[Acquire Global Test Lock] --> B[Reset Global State]
    B --> C[Run Test]
    C --> D[Release Lock]
```
