# Code Reuse

In EDKII, code re-use was done using the `LibraryClasses` build concept. In rust, we do **not** use
rust `Traits` for code-reuse. Instead we use rust `Crates`. See some of the generic reading here:

- [Packages and Crates](https://doc.rust-lang.org/book/ch07-00-managing-growing-projects-with-packages-crates-and-modules.html)
- [MIT Crates and Modules](https://web.mit.edu/rust-lang_v1.25/arch/amd64_ubuntu1404/share/doc/rust/html/book/first-edition/crates-and-modules.html)
- [Project Structure](https://learning-rust.github.io/docs/cargo-crates-and-basic-project-structure/)

``` admonish important
When creating crates that are being published, you should do your best to make
your crates dependency versions as least specific as possible. What this means is that if possible,
do not do `crate_dependency == "1.42.8"`. Instead do  `crate_dependency == "1.*"` if any version
between 1 and two is expected to work. `crate_dependency == "~1"` is equivalent if you do not want
to use wildcards. See [Version Requirement Syntax](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#version-requirement-syntax)
for specifics.
```

Cargo will do its best to resolve dependency requirements down to a single version of each crate. However, if it can't,
it will simply download and compile multiple versions of the same crate. This has a couple of issues:

1. It increases compile time
2. It can bloat the size
3. It can cause API expectations to break resulting in compilation failures

What (3) means is that `TraitA` in `Crate1` version `1.0.0` will be treated as a completely
different trait than `TraitA` in `Crate1` version `1.0.1`. You'll end up seeing compilation errors
such as the following, when it works previously.

``` txt
^^^^^^^ the trait `XXXXX` is not implemented for `YYYYYYY`
```
