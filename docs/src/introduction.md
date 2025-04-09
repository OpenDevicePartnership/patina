# Introduction

This book is a getting started guide for developing UEFI firmware in a `no_std` environment,
integrating the rust implementation of the DXE Core to a platform, developing a pure-rust DXE
Driver, and guides on further developing the rust DXE Core.

This book assumes you already have the pre-requisite knowledge in regards to the [EDKII](https://github.com/tianocore/edk2)
ecosystem, and the necessary tools already installed for building EDKII packages.

Major features are proposed through [RFCs](rfc/template.md). The RFC template should be completed and pushed to a PR.

The complete process is:

1. **Create** a new branch for your RFC.
2. **Copy** the template from `docs/src/rfc/template.md` to a new file in the `docs/src/rfc/text` directory named
   `0000-<feature-name>.md` where `0000` is a placeholder until the RFC is accepted (so use `0000` in your PR) and
   `<feature-name>` is a short name for the feature that you create.
3. **Fill out** the RFC template with your proposal.
4. Submit a **pull request** (PR) with your RFC.
5. The PR will be discussed, reviewed, and may be iteratively updated.
6. Once there is consensus and approval, the RFC will be **merged** and assigned an official number.

## The RFC Life Cycle

Each RFC goes through these stages:

- **Draft**: The initial state when a PR is opened. The community and relevant teams provide feedback.
  - Note: The RFC PR does not need to be draft. Only make it a draft if you're still working on the PR prior to
    submitting it for review.
- **Final Comment Period (FCP)**: Once there is rough consensus, an FCP of 7–10 days starts. During this time, final
  objections can be raised.
- **Merged**: After FCP with no blocking concerns, the RFC is merged and becomes official.
- **Postponed**: RFCs may be deferred due to lack of clarity, priority, or readiness.
- **Rejected**: With strong reasoning and community consensus, RFCs can be declined.

## Implementing and Maintaining an RFC

Once accepted:

- The implementation is tracked through linked issues or repositories.
- Any changes during implementation that deviate from the RFC must go through a **follow-up RFC** or an
  **amendment** process.
- An RFC can be **revised** in-place via a new RFC that supersedes or modifies the previous one.

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
