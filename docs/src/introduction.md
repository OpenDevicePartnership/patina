
# Introduction

**Welcome to Patina** - a pure Rust project dedicated to evolving and modernizing UEFI firmware with a strong focus on
security, performance, and reliability. This book serves as high-level documentation for developers and platform
owners working with or contributing to Patina.

It provides guidance on building firmware in a `no_std` Rust environment, integrating the Patina DXE Core, developing
pure-Rust Patina components, and contributing to the ecosystem - all without assuming prior experience with
Patina itself.

Before getting started, you may want to read the [Patina Background](patina.md), which outlines the project's goals
and design philosophy.

In addition, here are some of the more commonly referenced documentation in this book:

1. [Patina Background](patina.md)
2. [RFC Lifecycle](rfc_lifecycle.md)
3. [Platform Integration](integrate/patina_dxe_core_requirements.md)
4. [Component Development](component/getting_started.md)
5. [Developer Guides](dev/documenting.md)

```admonish note
This documentation aims to be as detailed as possible, not assuming any previous knowledge. However some general Rust
knowledge is beneficial throughout the book, and some EDK II knowledge is beneficial to understanding how consume the
final pure-Rust platform Patina DXE core in EDK II style firmware.
```
