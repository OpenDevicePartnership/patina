# Patina Project Instructions

Patina is a Rust-based UEFI firmware project. It currently replaces the traditional C-based EDK II
DXE Core with a Rust implementation that introduces a component-based architecture with dependency
injection in addition to a general-purpose UEFI Rust SDK.

## Project Structure

The workspace is organized into four top-level areas:

- `components/` - Feature components (ACPI, Advanced Logger, MM, Performance, SMBIOS, etc.)
- `core/` - Shared core internals (debugger, collections, CPU, dependency expressions, stack traces)
- `patina_dxe_core/` - Main DXE Core library that ties everything together
- `sdk/` - Public SDK (`patina` crate for Boot/Runtime Services, component infrastructure, `patina_ffs` for firmware
   file system, `patina_macro` for proc-macros)

Crate dependency rules are strict. Components depend on `sdk/` only - never on each other or on `core/`. The SDK
depends only on generic external crates. Core crates may depend on each other and on the SDK. See
`docs/src/dev/code_organization.md` for the full dependency matrix.

## Build and Test Commands

All commands go through `cargo make`. Never run raw `cargo` commands.

- `cargo make check` - Type-check all code
- `cargo make fmt` - Format code (run after every edit)
- `cargo make clippy` - Lint with clippy
- `cargo make test` - Run all unit tests
- `cargo make all` - Full PR readiness (fmt-check, deny, cspell, clippy, build, test, coverage, doc)
- `cargo make build-x64` / `cargo make build-aarch64` - Build for UEFI targets
- `cargo make coverage` - Generate test coverage reports
- `cargo make doc` - Build documentation

See `Makefile.toml` for the complete command list.

## Module Organization

- Never use `mod.rs` files. Use named module files (e.g., `my_module.rs`) instead.
- No public definitions directly in `lib.rs` - only public module declarations.
- Break implementations into logical files with clean namespaces.
- Crate naming: `patina_` prefix for public crates, `patina_internal_` for internal
  crates, `_macro` suffix for proc-macro crates.

```rust
// WRONG: using mod.rs
// src/memory/mod.rs

// CORRECT: using named module file
// src/memory.rs
```

See `docs/src/component/requirements.md` for crate layout standards.

## Safety Conventions

- Prefer `zerocopy` for binary data and memory layouts over manual unsafe pointer/slice
  operations. `zerocopy` provides safe, zero-cost abstractions for interpreting byte
  buffers as typed data.
- Minimize `unsafe` code. Constrain it within safe abstraction wrappers.
- Document preconditions, postconditions, and invariants for every `unsafe` block.
- Prefer fallible constructors returning `Result` over constructors that silently
  handle invalid input or panic.
- Mark functions `unsafe` only when the caller must uphold a contract the function
  itself cannot verify internally. If the function validates all inputs before the
  unsafe operation, it can remain safe.

See `docs/src/dev/principles/unsafe.md` for the full safety philosophy
(software safety vs. hardware safety distinctions).

## Error Handling

- Prefer `Result` return types. Avoid panics in production code.
- At UEFI ABI boundaries (`extern "efiapi"` functions), use `efi::Status`.
- Internally, use domain-specific Rust error types with `Debug`, `Display`, and `Error`
  trait implementations.
- Implement `From<>` conversions at error type boundaries to create clean error chains.
- Use `expect("descriptive message")` over bare `unwrap()`. Reserve `unwrap()` for
  test code only.

See `docs/src/dev/principles/error-handling.md` for error
propagation patterns and examples.

## Logging

- Use the `log` crate for structured logging.
- Log at appropriate levels: `trace`, `debug`, `info`, `warn`, `error`.
- Include relevant context in log messages.
- Do not log in hot paths or have entry/exit logs.
- Prefer using `patina::writelncrlf` for formatting strings that will be logged over `writeln!`.

## Component Model

Patina uses dependency injection through component entry point function signatures.
A component only executes when all its declared dependencies are available.

- Define components with the `#[component]` attribute macro on an `impl` block.
  The impl must contain `fn entry_point(self, ...) -> Result<()>`.
- Register service implementations with `#[derive(IntoService)]` and `#[service(dyn Trait)]`.
- Param types: `Config<T>`, `ConfigMut<T>`, `Service<T>`, `Hob<T>`, `Commands`,
  `Handle`, `StandardBootServices`, `StandardRuntimeServices`, `&Storage`/`&mut Storage`,
  `Option<P>`, tuples.
- `ConfigMut<T>` components run first (config is unlocked); calling `lock()` makes the
  value immutable and enables `Config<T>` components to execute.
- `Service<dyn Trait>` is preferred over `Service<ConcreteType>` for mockability. Use
  a concrete wrapper struct only when generic method signatures are needed.
- Use the **stored dependencies** pattern: store all dependency references as fields in
  the component struct. The entry point stores references; methods use them.

See `docs/src/component/interface.md` and
`docs/src/component/getting_started.md` for details and examples.

## Mocking and Testing

- Target ≥80% code coverage. Patch coverage ≥80% is a required PR check.
  - Write tests to cover critical logic and edge cases, not just to hit coverage numbers.
- Test naming: prefix with `test_<component_name>_` or `test_<service_name>_`.
- Prefer `mockall` for trait mocking in unit tests. Use `#[automock]` on traits to generate mocks.
- Use `pretty_assertions` for readable test diffs.

See `docs/src/dev/testing.md` for the full testing strategy.

## Trait Design

Traits serve as **abstraction points** - not code reuse mechanisms. Use crates for
code reuse (see `docs/src/dev/principles/reuse.md`).

- Define traits to represent swappable behavior (e.g., `BootServices`, `SerialIO`).
- Implementations can vary per platform without affecting consumers.
- Keep traits focused on a single responsibility (Interface Segregation).

See `docs/src/dev/principles/abstractions.md` for the full trait
design philosophy.

## Documentation Standards

- All public items must be documented. Set `#[deny(missing_docs)]` in component crates.
- Use `///` doc comments with these sections as appropriate: **Examples**, **Errors**,
  **Safety**, **Panics**.
- Document **traits**, not their implementations.
- In most cases, do not add `# Arguments` or `# Returns` sections - the function signature
  should be self-evident. In cases where there is unavoidable ambiguity, add these sections
  to provide clarity.

See `docs/src/dev/documenting.md` for templates and style guides.

## UEFI-Specific Guidelines

- Use `TplMutex` for synchronization in shared state. Do not use non-TPL-aware
  primitives like `spin::Mutex`. Keep critical sections narrow.
- Do not use `TplMutex` for interior mutability on non-shared data.
- No memory allocation or deallocation after `ExitBootServices`.

See `docs/src/dxe_core/synchronization.md`,
`docs/src/dxe_core/memory_management.md`, and
`docs/src/integrate/patina_dxe_core_requirements.md`.

## Hardware Access

### Memory-Mapped I/O (MMIO)

Do not create Rust references (`&T` or `&mut T`) to MMIO space. The compiler may
insert spurious reads through references, which can clear interrupt status bits, pop
FIFO entries, or trigger other device side-effects. MMIO must be accessed exclusively
through raw pointers with volatile operations.

Patina uses the `https://github.com/google/safe-mmio` crate for all MMIO
access. `safe-mmio` provides `UniqueMmioPointer<T>` which never creates a `&T` to device
memory. Its field wrapper types encode read side-effect semantics at the type level:

- `ReadPure<T>` / `ReadPureWrite<T>` — reads have no side-effects (shared access OK).
- `ReadOnly<T>` / `ReadWrite<T>` — reads have side-effects (require `&mut` access).
- `WriteOnly<T>` — write-only register.

See `docs/src/dev/hardware_access/mmio.md` for the full rationale, wrapper
reference table, and links to `safe-mmio` documentation.

### Architectural Interfaces

Minimize the use of inline assembly. Generally, architectural interfaces accessed through
inline assembly should be wrapped in safe Rust abstractions in the SDK. The exception to
this is if the interface is not generally applicable.

## Dependency Management

- All external dependencies are vetted via `cargo-deny` for license compatibility,
  security advisories, and allowed sources.
- Use workspace-level dependency declarations in the root `Cargo.toml` for consistency.
- Evaluate new dependencies against the criteria in
  `docs/src/dev/principles/dependency-management.md`.

## Formatting

Formatting is enforced by `rustfmt` via `rustfmt.toml`. Always run
`cargo make fmt` after editing code.

## Common Anti-Patterns

Flag these patterns during review:

1. Using `mod.rs` instead of named module files
2. Raw slice/pointer manipulation where `zerocopy` would work
3. Unnecessary `unsafe` without a safe abstraction wrapper
4. `unwrap()` in production code (outside of tests)
5. Cross-component dependencies (components must only depend on `sdk/`)
6. Adding public type definitions directly in `lib.rs`
7. Missing documentation on public items
8. Using non-TPL-aware synchronization primitives for shared state
9. Creating Rust references (`&T`/`&mut T`) to MMIO space instead of using `safe-mmio`
