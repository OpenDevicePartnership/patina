# DXE Core Testing

Writing DXE Core tests follows all the same principles defined in the [Testing](../dev/testing.md) chapter, so if you
have not reviewed it yet, please do so before continuing. One of the reasons that [uefi-dxe-core](https://github.com/pop-project/uefi-dxe-core)
is split into multiple crates and merged the the `dxe_core` umbrella crate is to support code separation and ease of
unit testing.

## Testing with Global State

One of the difficulties with writing host based unit tests in the DXE core is that the DXE core has multiple static
pieces of data that are referenced throughout the codebase. Since unit tests are ran in parallel, this means that
multiple tests may be manipulating this static data at the same time. This will lead to either dead locks or the
static data state being something unexpected by the test.

To help with this issue in the dxe_core crate, we have created a created a [test_support](https://github.com/pop-project/uefi-dxe-core/blob/main/dxe_core/src/test_support.rs)
module to provide functionality to make writing tests more convenient. This most important functionality is the
`with_global_lock` function, which takes your test closure / function as a parameter. This function locks a private
global mutex, ensuring you have unique access to all statics within the dxe_core. 

``` admonish warning
It is the responsibility of the test writer to reset the global state to meet their expectations. It is **not** the
responsibility of the test writer to clear the global state once the test is finished.
```
