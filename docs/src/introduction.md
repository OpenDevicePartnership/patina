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

The specific toolchains and components that are required to be installed can be found in the `rust-toolchain.toml`
file and will automatically be installed by `cargo` upon your first `cargo` command. The file will look something like
this:

``` toml
[toolchain]
version = "1.80.0"
targets = ["x86_64-unknown-uefi", "aarch64-unknown-uefi"]
components = ["rust-src"]
```

There are additional cargo plugins (installabales) that will need to be installed depending on what you are doing. You
can find a list of all tools in the same file under the `[tools]` section. At a minimum, you will need `cargo-make` for
compilation and `cargo-tarpaulin` for code coverage. At a minimum, you should install these tools at the version
specified via `cargo install --force $(tool_name) --version $(version)`, but it would be best to install all of them.

``` admonish note
`cargo install` will download and compile these tools locally. If you first install `cargo-binstall` with
`cargo install cargo-binstall` you can change the command from `install` to `binstall` which will simply download the
pre-compiled binary and will be much faster.
```

### Cargo Make

Due building in a `no_std` while also supporting multiple rust [uefi target triples](https://doc.rust-lang.org/nightly/rustc/platform-support/unknown-uefi.html#-unknown-uefi),
the command line flags to successfully run any rust commands can be complex and verbose. To counter
this problem, and simplify the developer experience, we use [cargo-make](https://github.com/sagiegurari/cargo-make)
as the drop in replacement for cargo commands. What this means, is that instead of running
`cargo build`, you would now run `cargo make build`. Many other commands exist, and will exist on a
per-repository basis.

### Cargo Tarpaulin

[cargo-tarpaulin](https://github.com/xd009642/tarpaulin) is our tool for generating code coverage
results. Our requirement is that any crate being developed must have at least 80% code coverage,
so developers will want to use `tarpaulin` to calculate code coverage. In an existing repository,
a developer will use `cargo make coverage` to generate coverage results, and a line-coverage html
report.
