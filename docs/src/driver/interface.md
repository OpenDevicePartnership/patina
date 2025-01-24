# Monolithically Compiled Components

There is no standard entry point for a monolithically compiled rust component that is dispatched by the rust DXE core.

Where EDKII Dxe Core dispatcher expects a well defined entry point of
`EFI_HANDLE ImageHandle, EFI_SYSTEM_TABLE *SystemTable`, the pure rust DXE Core uses [Dependency Injection](https://wikipedia.org/wiki/Dependency_injection)
to allow a component to define an interface that specifies all dependencies needed to properly execute. Due to this,
dependency expressions are no longer necessary, as the function interface is the dependency expression. What this
means is that instead of evaluating a dependency expression to determine if a driver can be executed, it instead
attempts to fetch all requested parameters defined in the function interface. If all are successfully fetched, then the
component is executed, if not, it will not be dispatched, and another attempt will be made in the next iteration.

In the Rust DXE Core, a component is simply a trait implementation. So long as a struct implements [Component](todo/docs.rs)
and [IntoComponent](todo/docs.rs), it can be consumed and executed by the pure rust DXE core. [uefi_sdk](todo/docs.rs)
currently provides two implementations for `Component`:

The first is [FunctionComponent](todo/docs.rs). This type cannot be instantiated manually, but a blanket implementation
of the `IntoComponent` trait allows any function whose parameters support dependency injection to be converted into a
`FunctionComponent`.

The second is [StructComponent](todo/docs.rs). This type cannot be instantiated manually, but a derive proc-macro of
`IntoComponent` is provided that will allow any struct or enum to be used as a component. This derive proc-macro
expects that a `Self::entry_point(self, ...) -> uefi_sdk::error::Result<()> { ... }` exists, where the `...` in the
function definition can be any number of parameters twho support dependency injection as shown below. The function
name can be overwritten with the attribute macro `#[entry_point(path = path::to::func)]` on the same struct.

See [Samples](https://github.com/OpenDevicePartnership/uefi-dxe-core/tree/main/sample_components) or [Examples](#examples)
for examples of basic components using these two methods.

Due to this, developing a component is as simple as writing a function whose parameters are a part of the below list of
supported parameters (which is subject to change). Always reference the trait's [Type Implementations](todo/docs.rs)
for a complete list, however the below list should be up to date:

<!-- markdownlint-disable -->
| Param                        | Description                                                                                                                                                |
|------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Option\<P\>                  | An Option, where P implements `Param`. Allows components to run even when the underlying parameter is unavailable. See the [params] module for more info.  |
| (P1, P2, ...)                | A Tuple where each entry implements `Param`. Useful when you need more parameters than the current parameter limit. See the [params] module for more info. |
| Config\<T\>                  | An immutable config value that will only be available once the underlying data has been locked. See The [params] module for more info.                     |
| ConfigMut\<T\>               | A mutable config value that will only be available while the underlying data is unlocked. See the [params] module for more info.                           |
| StandardBootServices         | Rust implementation of Boot Services                                                                                                                       |
<!-- markdownlint-enable -->

```admonish warning
unfortunately, the compile-time error you get when trying to register a function whose parameters do not all implement
`ComponentParam` can be long and unclear. If you see an error similar to the below error message, just know that it is
likely because one of your parameters does not implement `ComponentParam`. Keep in mind, the `&` (or lack there of)
does matter!

    error[E0277]: the trait `function_component::ComponentParamFunction<_>` is not implemented for fn item `<fn_interface>`
```

Please reference the documentation in `uefi_sdk::component` for more information regarding these parameters.

## Examples

### FunctionComponent Examples

```rust
use uefi_sdk::{
    boot_services::StandardBootServices,
    component::params::{Config, ConfigMut},
    error::Result,
};

fn validate_random_data_driver(bs: StandardBootServices, data: Config<&[u8]>, expected_crc32: Config<u32>) -> Result<()> {
    assert_eq!(bs.calculate_crc_32(*data), Ok(*expected_crc32))
    Ok(())
}

#[cfg(test)]
mod tests {
    use uefi_sdk::component::IntoComponent;
    use super::validate_random_data_driver;

    #[test]
    fn ensure_function_implements_into_component() {
        // If this test compiles, `validate_random_data_driver` correctly implements `Component` via the blanket
        // implementation. Changing the function interface could unknowingly break this expectation, so we want to test
        // it.
        let _ = validate_random_data_driver.into_component();
    }
}
```

### StructComponent Examples

```rust
use uefi_sdk::{
    boot_services::StandardBootServices,
    component::{
        IntoComponent,
        params::{Config, ConfigMut},
    },
    error::{EfiError, Result},
};

#[derive(IntoComponent)]
#[entry_point(path = entry_point)]
struct MyComponent {
    private_config: u32,
}

fn entry_point(c: MyComponent, public_config: Config<u32>) -> uefi_sdk::error::Result<()> {
    if *public_config != c.private_config {
        return Err(EfiError::Unsupported)
    }
    Ok(())
}

#[derive(IntoComponent)]
struct MyComponent2 {
    private_config: u32,
}

impl MyComponent2 {
    fn entry_point(self, public_config: ConfigMut<u32>) -> uefi_sdk::error::Result<()> {
        public_config += self.private_config;
        Ok(())
    }
}
```
