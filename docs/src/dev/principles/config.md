# Component Config

In EDK II, configuration of libraries and components was managed through PCDs due to the fact that all modules were
compiled separately, making it difficult to share configuration. With the Patina DXE Core, components and the DXE Core
are compiled together in a monolithic binary. This allows for configuration to be done in code, during the instantiation
process of each individual driver.

The expectation is that inside the start function of the DXE Core, you will instantiate each driver, and you can share
configuration values between them.

## Example

```rust
pub extern "efiapi" fn _start(physical_hob_list: *const c_void) -> ! {
    let config1: bool = true;
    let config2: u64 = 0x10000;

    let driver1 = Driver1::default()
        .with_config(config1);

    let driver2 = Driver2::default()
        .with_config(config2);

    let dxe_core = Core::default()
        .with_config1(config1)
        .with_config2(config2)
}
```
