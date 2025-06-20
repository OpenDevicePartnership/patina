# Core Concepts

In this section, we will talk about the core concepts for developing pure Rust DXE Drivers including the DXE Core, and
how it differs doing the same in EDK II.

## Platform Configuration

With EDK II, there are two core parts of the build system that allow for configuration. The first is the
`LibraryClasses` concept which creates abstraction points to completely change implementation details to support a
specific platform, architecture, or silicon. The second is the `Platform Configuration Database (PCD)` concept, which
allows for configuration of specific implementations and the sharing of certain settings across multiple
implementations or components.

The following sections will talk about these two functionalities, and the options available through the Patina DXE Core.

### LibraryClasses

The `LibraryClasses` concept in EDK II is really used for two distinct reasons. The first is that it allows for code
reuse. The easiest example is that `BaseLib` is used for every single component and library class written. The second
is that it is a point of abstraction that allows the platform to change the underlying implementation. This can be for
any reason, but is commonly used to change functionality for platform, hardware, architecture, or even silicon vendor
specific reasons.

In Patina, we split the `LibraryClasses` concept into two distinct concepts - [Traits](https://blog.rust-lang.org/2015/05/11/traits.html)
for abstractions and [Crates](https://doc.rust-lang.org/book/ch07-01-packages-and-crates.html)
for code reuse. To learn how to effectively use Traits and Crates to represent EDK II
LibraryClasses, Please review the [Abstractions](dev/principles/abstractions.md) and [Code reuse](dev/principles/reuse.md)
sections.

```admonish important
`Traits` are used to create abstractions / interfaces. No component, library, or the DXE Core itself should care about,
or take dependence on, implementation details of a specific trait. Swapping trait implementations should be as simple
as passing the implementation to the struct initializer.
```

### Platform Configuration Database (PCD) Like Configuration in Patina

In EDK II, all components are compiled separately per the Platform Description File (DSC), which makes it difficult to
share configuration settings across all components. Due to this, EDK II created the concept of PCDs to allow for
sharing configuration across all modules. There are multiple different types of PCDs with both static and dynamic
values.

Due to the monolithic nature of the Patina DXE Core, this complex system for platform configuration is no longer
necessary. Instead, we can perform configuration in code, and share configuration values between the Patina DXE Core
and any component being compiled with the Patina DXE Core.

For more information, please review the [Configuration in Code](dev/principles/config.md) section.
