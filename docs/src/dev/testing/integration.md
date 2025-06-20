# Integration Testing

Integration testing is very similar to unit testing; however, each test is placed in a `tests/*` folder at the same
directory level as `src/*`. When writing tests of this type, the developer does not have access to the internal
state of the module—only the external interfaces are being tested. `Cargo` will detect and run these tests with
the same command as for unit tests.

See the Cargo Book entry on [Integration Testing](https://doc.rust-lang.org/rust-by-example/testing/integration_testing.html).

Here is an example file structure for a crate that contains integration tests:

```text
├── src
│   └── main.rs
├── tests
│   ├── integration_test1.rs
│   └── integration_test2.rs
├── .gitignore
└── Cargo.toml
```

> **Note:** Integration tests are ideal for verifying the public API and behavior of your crate as a whole.
