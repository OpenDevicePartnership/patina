# Error Handling

Due to the difficulty of recovering from panics in firmware, it is almost always preferrable to return and propagate up an error than to panic. In order of most to least safe, code should:
1. Propagate errors using `Result` or `Option` whenever possible. 
2. For panics guarded by existing code (for example, a `is_null` check before a `.as_ref()` call), provide a detailed message on how the existing code should prevent panics. Use `expect`, `log`, or `debug_assert` for such cases. 
3. For genuinely unrecoverable errors, ensure a detailed error message is provided, usually through `expect`. Code should avoid `unwrap` except in test scenarios.

## Example
Consider the following example involving the `adv_logger`. Since the logger is not necessarily required to boot drivers / continue normal execution, we can attempt to continue even if it is not properly initialized.

This code which `unwrap`s on logger initialization panics unnecessarily:

``` rust
let log_info = self.adv_logger.get_log_info().unwrap();
```

Consider replacing it with `match` and returning a `Result`:

``` rust
let log_info = match self.adv_logger.get_log_info() {
    Some(log_info) => log_info,
    None => {
        log::error!("Advanced logger not initialized before component entry point!");
        return Err(EfiError::NotStarted);
    }
};
```
