# Writing Tests

Another benefit of rust is that testing is "baked-in" and made easy. There is plenty of documentation regarding testing
for rust, so if you don't know much about testing in rust, start with that and come back here. In this documentation,
we will stick with what is directly applicable to this project.

Testing in rust can be broken up into four core categories: (1) Unit testing, where the developer has access to the
internal, private state of the module to ensure the internals work as expected. (2) Integration Testing, which is done
outside of the module and tests the code from an external interface standpoint. (3) Platform Testing - e.g. writing
tests that run on the platform, physical or virtual. (4) Doc testing, which is a testing type that is covered in
[Documentation](documenting.md), but suffice it to say that code snippets for inline documentation can be compiled and
executed.

[Rust Book Testing](https://doc.rust-lang.org/rust-by-example/testing.html)

## Development Dependencies

Rust has the concept of `dev-dependencies` that can be specified in a crate's `Cargo.toml` file. These dependencies are
only used in the writing and running of tests, and thus will only be downloaded and compiled for test execution. One
common example, as specified in the rust book (linked at the chapter start) is `pretty_assertions` which extends the
standard assertions to create a colorful diff.

## Code Coverage

Code coverage is another incredibly important aspect of our project, that was lacking in other projects. Our intent is
to keep above 80% code coverage for all crates in any given repository. We use [Cargo Tarpaulin](https://crates.io/crates/cargo-tarpaulin)
as our code coverage reporting tool as it works well with windows and Linux, and can generate different report types.
Each repository must have CI that fails if any code added to the repository has less than 80% coverage, or the
repository as a whole is less than 80% coverage.
