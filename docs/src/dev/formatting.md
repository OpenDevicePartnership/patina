# Code Formatting

Formatting is made easy with Rust! We rely on `cargo fmt` and `cargo clippy` to apply formatting changes.

[`cargo fmt`](https://github.com/rust-lang/rustfmt) will format your code by following default rules and allows for
customization via a `rustfmt.toml` file at the root of a repository. This tool makes no code functionality changes and
is safe to use.

[`cargo clippy`](https://github.com/rust-lang/rust-clippy) is a more comprehensive linting tool that requires careful
consideration. Unlike `cargo fmt`, which automatically applies formatting changes, `cargo clippy` provides suggestions
for improving code quality and adherence to Rust idioms. These recommendations may involve modifications to code logic
and structure. While these changes typically enhance code quality by promoting idiomatic Rust patterns, they should be
reviewed carefully as they may not be appropriate for all use cases. Configuration options are available through a
`clippy.toml` configuration file for customizing the tool's behavior.

If a change is not applicable, you will need to tell clippy to ignore that bit of code.

`cargo fmt` and `cargo clippy` should always be run as part of CI and are run in the `cargo make all` command.
