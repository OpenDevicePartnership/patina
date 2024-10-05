# Core Concepts

In this section, we will talk about the core concepts for developing pure rust DXE Drivers
including the DXE Core, and how it differs doing the same in EDKII.

## Platform Configuration

With EDKII, there are two core parts of the build system that allow for configuration. The first
is the `LibraryClasses` concept which creates abstraction points to completely change
implementation details to support a specific platform, architecture, or silicon. The second is the
`Platform Configuration Database (PCD)` concept, which allows for configuration of specific
implementations and the sharing of certain settings across multiple implementations or components.

The following sections will talk about these two functionalities, and the options available through
the pure rust dxe core.

### LibraryClasses

The `LibraryClasses` concept in EDKII is really used for two distinct reasons. The first is that it
allows for code - reuse. The easiest example is that BaseLib is used for every single component and
library class written. The second is that it is a point of abstraction that allows the platform to
change the underlying implementation. This can be for any reason, but is commonly used to change
functionally based off the platform, hardware components, architecture specific reasons, or even
silicon vendor specific reasons.

in the rust implementation, we split the `LibraryClasses` concept into two distinct concepts -
[Traits](https://blog.rust-lang.org/2015/05/11/traits.html) for abstractions and [Crates](https://doc.rust-lang.org/book/ch07-01-packages-and-crates.html)
for code reuse. To learn how to effectively use Traits and Crates to represent EDKII
LibraryClasses, Please review the [Abstractions](dev/abstractions.md) and [Code reuse](dev/reuse.md)
sections of the Best Practices Chapter.

### Platform Configuration Database

In EDKII, all components are compiled separately per the Platform Description File, which makes it
difficult to share configuration settings across all components. Due to this, EDKII created the
concept of PCDs to allow for sharing configuration across all modules. There are multiple different
types of PCDs with both static and dynamic values.

Due to the monolithic nature of the pure rust DXE Core, this complex system for platform
configuration is no longer necessary. Instead, we can perform configuration in code, and share
configuration values between the dxe_core and any driver being compiled as a part of the dxe_core.
To learn how to effectively do configuration in code, please review the [Configuration in Code](dev/principles/config.md)
section of the Best practices Chapter.
