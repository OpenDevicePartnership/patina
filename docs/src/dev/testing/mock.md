# Mocking

Mocking is supported through the [mockall](https://crates.io/crates/mockall) crate. This crate provides multiple ways
to mock throughout your crate including:

- [Mocking Local Traits](https://docs.rs/mockall/0.13.0/mockall/index.html#getting-started)
- [Mocking External Traits](https://docs.rs/mockall/0.13.0/mockall/index.html#external-traits)
- [Mocking Structs](https://docs.rs/mockall/0.13.0/mockall/#mocking-structs)
- [Mocking Modules](https://docs.rs/mockall/0.13.0/mockall/index.html#modules)
- [Mocking FFI Functions](https://docs.rs/mockall/0.13.0/mockall/index.html#foreign-functions)

This documentation will not go into the specifics of any of those as the `mockall` crate itself has a large amount of
documentation for each, and has [Examples](https://docs.rs/mockall/0.13.0/mockall/index.html#examples) on how to do
these different types of mocking.

## uefi-dxe-core Examples

### Mocking an external trait

The below example can be found in `uefi_test/src/lib.rs` and is mocking the `DxeComponentInterface` from the
`uefi_component_interface crate.

``` rust
#[cfg(test)]
mod tests {
  mockall::mock! {
    ComponentInterface {}
      impl DxeComponentInterface for ComponentInterface {
        fn install_protocol_interface(&self, handle: Option<efi::Handle>, protocol: efi::Guid, interface: *mut c_void) -> Result<efi::Handle, efi::Status>;
    }
  }

  #[test]
  fn test_example() {
    let mut interface = MockComponentInterface::new();
    interface.expect_install_protocol_interface().return_once(move |_,_,_| Ok(core::ptr::null_mut()));
    let _ = component.entry_point(&interface);
  }
}
```
