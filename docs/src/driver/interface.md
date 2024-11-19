# Component Interface

**TODO** This section is work-in-progress.

This chapter will walk you through writing UEFI components in rust that will be compiled into the
pure rust DXE Core image.

## NOTE1

To write a driver (or crate) that can be easily modified by a platform to support different
hardware, it is expected that the developer use the concept of [generics](https://doc.rust-lang.org/book/ch10-01-syntax.html)
and [traits](https://doc.rust-lang.org/book/ch10-02-traits.html). This creates a point of
abstraction through which the platform can provide the necessary functionality. The driver
creator can provide trait implementations if they desire, or leave it to the platform to write
their own.

## NOTE2

Now that we've established how to create a trait to define an interchangeable interface for
developers to select platform specific code, we can move on to creating a component!

Creating a component is similar to the above, except that instead of creating or implementing a
trait, you implement the [Component] trait. This trait is simple, only requiring an entry point
function, which is executed by the DXE Core. Specifying trait
dependencies in your component implementation is the same as described above.

``` rust
    pub struct MyComponent;
    impl uefi_sdk::Component for MyComponent {
        fn entry_point() -> Result<(), uefi_sdk::error::EfiError> {
            // Your component code here
            Ok(())
        }
    }
```

So now that you have a component, how do you instantiate it? This is as simple as creating
a type alias with all trait instances selected. Then you pass this type alias's `entry_point`
function to the DXE Core for execution. The type alias not truly necessary, but is good for
code organization / readability - especially if there are many trait dependencies and/or
multiple components in the same workspace.

``` rust
    use uefi_sdk::Component;
    pub struct MyComponent;
    impl Component for MyComponent {
    fn entry_point() -> Result<(), uefi_sdk::error::EfiError> {
        // Your component code here
        Ok(())
    }
    }
    pub type MyDriver = MyComponent;

    // Where the DxeCore is located
    let components = vec![MyDriver::entry_point];
```

## Getting Started

The design principle behind components and traits is similar to that of EDKII in which
core functionality is present in the component (or library) and any usage of library
functionality is abstracted through a library interface. in EDK2, the library classes system
has two distinct use-cases: (1) Code reuse and (2) Implementation abstraction where (2) implies
functionality substitution. In rust, these two use cases have a separate mechanism for each
scenario.
