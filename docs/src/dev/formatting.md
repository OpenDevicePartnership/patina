# Code Formatting

Formatting is made easy with rust! We rely purely on `cargo fmt` and `cargo clippy` to apply
formatting changes for you.

[`cargo fmt`](https://github.com/rust-lang/rustfmt) will format your code by following default
rules and allows for customization via a `rustfmt.toml` file at the root of a repository. This
tool makes no code functionality changes and is safe to use.

[`cargo clippy`](https://github.com/rust-lang/rust-clippy) is more dangerous and because of this,
by default it only tells you about fixes it suggests (where as `cargo fmt` applies it).
`cargo clippy` can change logic in your code to be more "rusty", but depending on your use case,
this change could be _wrong_. Similar to `cargo fmt`, `cargo clippy` can be customized via a
`clippy.toml` file.

If a change is not applicable, you will need to tell clippy to ignore that bit of code.

`cargo fmt` and `cargo clippy` should be run as a part of CI for all UEFI rust repositories.
