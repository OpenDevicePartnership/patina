# Introduction

This book is a getting started guide for developing UEFI firmware in a `no_std` environment,
integrating the rust implementation of the DXE Core to a platform, developing a pure-rust DXE
Driver, and guides on further developing the rust DXE Core.

This book assumes you already have the pre-requisite knowledge in regards to the [EDKII](https://github.com/tianocore/edk2)
ecosystem, and the necessary tools already installed for building EDKII packages.

## Tools and Prerequisites

Below are a list of tools that need to be installed before working with the contents of this book,
not including the necessary tools to build EDKII packages.

### Rust

The rust installer provides multiple tools including `rustc` (the compiler), `rustup`
(the toolchain installer), and `cargo` (the package manager).

These tools are all downloaded when running the installer here: [Getting Started - Rust Programming Language (rust-lang.org)](https://www.rust-lang.org/learn/get-started).
This may require a restart of your command line terminal.

Once installed, the toolchain and components need to be installed, substituting `$(VERSION)`
for your platform's specified version:

Windows:

``` cmd
> rustup toolchain install $(VERSION)-x86_64-pc-windows-msvc
> rustup component add rust-src --toolchain $(VERSION)-x86_64-pc-windows-msvc
```

Linux:

``` cmd
> rustup toolchain install $(VERSION)-x86_64-unknown-linux-gnu
> rustup component add rust-src --toolchain $(VERSION)-x86_64-unknown-linux-gnu
```

### Cargo Make

Due building in a `no_std` while also supporting multiple rust [uefi target triples](https://doc.rust-lang.org/nightly/rustc/platform-support/unknown-uefi.html#-unknown-uefi),
the command line flags to successfully run any rust commands can be complex and verbose. To counter
this problem, and simplify the developer experience, we use [cargo-make](https://github.com/sagiegurari/cargo-make)
as the drop in replacement for cargo commands. What this means, is that instead of running
`cargo build`, you would now run `cargo make build`. Many other commands exist, and will exist on a
per-repository basis.

To install, run the following command, substituting `$(VERSION)` for your platform's specified
version:

`> cargo install cargo-make --version $(VERSION)`

or if you have `binstall` installed:

`> cargo binstall cargo-make --version $(VERSION)`

### Cargo Tarpaulin

[cargo-tarpaulin](https://github.com/xd009642/tarpaulin) is our tool for generating code coverage
results. Our requirement is that any crate being developed must have at least 80% code coverage,
so developers will want to use `tarpaulin` to calculate code coverage. In an existing repository,
a developer will use `cargo make coverage` to generate coverage results, and a line-coverage html
report.

To install, run the following command, substituting `$(VERSION)` for your platform's specified
version:

`> cargo install cargo-tarpaulin --version $(VERSION)`

or if you have `binstall` installed:

`> cargo binstall cargo-tarpaulin --version $(VERSION)`
