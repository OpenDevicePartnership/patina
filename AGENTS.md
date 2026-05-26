# AGENTS.md

Operational guide for AI coding agents (Copilot CLI, Codex, Claude Code, Cursor,
Aider, etc.) working in the **Patina** repository. Human contributors should
start with [`README.md`](README.md), [`CONTRIBUTING.md`](CONTRIBUTING.md), and
the published mdbook at <https://opendevicepartnership.github.io/patina/>.

Patina is a pure-Rust implementation of UEFI firmware: a replacement for the
EDK II DXE Core plus a general-purpose Rust SDK and a growing set of optional
"Patina components". The workspace targets `x86_64-unknown-uefi`,
`aarch64-unknown-uefi`, and host (`std`) for unit tests.

> **Read this entire document before making changes.** Patina has strict
> dependency rules, an unusual `cargo make`-driven workflow, hard
> `no_std`/UEFI constraints, and an **AI contribution policy** that materially
> affects how you must propose changes.

---

## 0. AI contribution policy (read first)

[`CONTRIBUTING.md`](CONTRIBUTING.md#ai-policy) and the
[README](README.md#ai-policy) state explicitly:

> Patina does not accept contributions directly from AI tools (e.g. GitHub
> Copilot).

What this means for you, the agent:

1. **Never open a PR autonomously.** The human operator must drive any PR
   into `OpenDevicePartnership/patina` and is responsible for understanding,
   reviewing, and testing every line of the change.
2. AI-assisted contributions are permitted only when the human contributor:
   - has the legal right to submit the code under Apache-2.0;
   - fully understands the change and can explain it to other contributors;
   - has thoroughly reviewed the diff;
   - has thoroughly tested it — **firmware changes must be tested on QEMU and
     on a physical platform**.
3. Default to producing small, well-scoped diffs that a human can actually
   review end to end. If you find yourself generating a large, sweeping
   refactor, stop and ask the operator to narrow the scope.
4. When summarizing your work for the operator, surface every non-trivial
   decision, every `unsafe` block touched, and every dependency added so the
   human reviewer can evaluate them deliberately.
5. Do **not** sign commits as the human or impersonate them. Use the operator-
   provided author identity and add the assistance trailer (see §11).

---

## 1. Repository at a glance

- **Language / edition:** Rust 2024, MSRV 1.89 (see workspace `Cargo.toml`).
- **Toolchain:** nightly, pinned by `rust-toolchain.toml`
  (currently `nightly-2026-02-13`). Required components: `rust-src`, `clippy`,
  `rustfmt`, `rust-docs`. Required targets: `x86_64-unknown-uefi`,
  `aarch64-unknown-uefi`.
- **Build driver:** [`cargo-make`](https://github.com/sagiegurari/cargo-make)
  via [`Makefile.toml`](Makefile.toml). **All builds and tests go through
  `cargo make`** — never invoke raw `cargo build`/`cargo test` for routine
  workflows (see §4).
- **Workspace resolver:** `3`. Members: `components/*`, `core/*`, `sdk/*`,
  `patina_dxe_core`.
- **Targets:** UEFI (`no_std`) for production, host (`std`) for unit tests
  and `dxe_core_std` example.
- **License:** Apache-2.0 (single license for the whole workspace).
- **CI driver:** Shared reusable workflows from
  `OpenDevicePartnership/patina-devops` (see §10).
- **Important sync note:** Several top-level files are **auto-synchronized
  from `OpenDevicePartnership/patina-devops`** and must not be edited in this
  repo. Files carrying this header include `Makefile.toml`,
  `rust-toolchain.toml`, `.github/workflows/ci-workflow.yml`, and others.
  See §13.

### Directory layout

```
.
├── AGENTS.md                  ← this file
├── README.md                  ← human-facing intro, build/test recipes
├── CONTRIBUTING.md            ← AI policy, RFC process, etiquette
├── CODE_OF_CONDUCT.md
├── SECURITY.md
├── Cargo.toml                 ← workspace manifest, shared deps, lints
├── Makefile.toml              ← cargo-make task definitions (synced)
├── rust-toolchain.toml        ← pinned nightly + [tools] (synced)
├── rustfmt.toml
├── deny.toml                  ← cargo-deny policy
├── cspell.yml                 ← spell-check policy
├── codecov.yml
├── .markdownlint.yaml, .markdownlintignore
├── .git-blame-ignore-revs
├── .gitattributes             ← `* -text` (no EOL normalization)
├── .cargo/                    ← cargo config (registry, target dir, etc.)
├── .github/
│   ├── copilot-instructions.md         ← canonical convention digest
│   ├── instructions/
│   │   └── components.instructions.md  ← scoped to components/**
│   ├── agents/
│   │   └── reviewer.agent.md           ← read-only reviewer persona
│   ├── prompts/
│   │   └── review.prompt.md            ← review checklist prompt
│   ├── workflows/                      ← CI (synced from patina-devops)
│   ├── ISSUE_TEMPLATE/, dependabot.yml, pull_request_template.md, …
├── .vscode/
├── components/                ← optional "Patina components" (see §3.1)
│   ├── patina_acpi/
│   ├── patina_adv_logger/
│   ├── patina_mm/
│   ├── patina_performance/
│   ├── patina_samples/
│   ├── patina_smbios/
│   └── patina_test/
├── core/                      ← Core-internal building blocks (see §3.2)
│   ├── patina_debugger/
│   ├── patina_internal_collections/
│   ├── patina_internal_cpu/
│   ├── patina_internal_depex/
│   └── patina_stacktrace/
├── sdk/                       ← Public SDK consumed by components (see §3.3)
│   ├── patina/                ← the umbrella SDK crate
│   ├── patina_ffs/
│   ├── patina_ffs_extractors/
│   └── patina_macro/          ← proc-macros re-exported by `patina`
├── patina_dxe_core/           ← The DXE Core library + dxe_core_std example
├── docs/                      ← mdbook source (see §12)
└── supply-chain/              ← cargo-vet audits / exemptions
```

### Workspace dependency rules (hard)

From [`docs/src/dev/code_organization.md`](docs/src/dev/code_organization.md)
and [`.github/copilot-instructions.md`](.github/copilot-instructions.md):

| Tier              | May depend on                                  | Must NOT depend on |
|-------------------|------------------------------------------------|--------------------|
| `sdk/*`           | Generic external crates only                   | `core/*`, `components/*`, `patina_dxe_core` |
| `core/*`          | `sdk/*`, other `core/*`, external crates       | `components/*`, `patina_dxe_core` |
| `components/*`    | `sdk/*` only, plus generic external crates     | Other `components/*`, `core/*`, `patina_dxe_core` |
| `patina_dxe_core` | `sdk/*`, `core/*`, selected components         | (top of the tree) |

If a code change appears to require a forbidden dependency edge, **stop and
flag it to the operator** — the right fix is almost always to move the shared
logic into `sdk/` or `core/`, not to add the edge.

---

## 2. Quickstart for an agent session

1. **Read** this `AGENTS.md`, [`.github/copilot-instructions.md`](.github/copilot-instructions.md),
   and [`.github/instructions/components.instructions.md`](.github/instructions/components.instructions.md)
   (when touching `components/**`).
2. **Locate** the right crate using §3. If you're not sure where a change
   belongs, ask the operator — placement matters because of dependency tiers.
3. **Plan**: state the smallest change that achieves the goal and the files
   you intend to touch.
4. **Edit** narrowly. Preserve existing module structure (no `mod.rs`, no
   public defs in `lib.rs`, named submodules — see §6).
5. **Verify locally** in this order (see §4 for full recipes):
   - `cargo make fmt-check`
   - `cargo make clippy`
   - `cargo make check`
   - `cargo make test` (host)
   - `cargo make build-x64` and/or `cargo make build-aarch64` if you touched
     UEFI-relevant code
   - For shipping changes: `cargo make all`
6. **Document**: every new public item needs a doc comment; `unsafe` needs
   `# Safety`; fallible APIs need `# Errors` when ambiguous.
7. **Commit** with the author identity, sign-off style, and trailer the
   operator specifies (see §11).
8. **Hand off** to the human for review and PR submission. Do not open the
   PR yourself.

---

## 3. Workspace members in depth

Each crate's `src/lib.rs` and `README.md` (where present) are the source of
truth — the summaries below are pointers, not specifications.

### 3.1 `components/*` — Patina components

Optional, opt-in feature modules. They are registered by an integrator into
a Patina DXE Core build. By the rules above, they depend **only on `sdk/`**
and generic external crates.

| Crate                          | Purpose (one-liner)                                                                  |
|--------------------------------|---------------------------------------------------------------------------------------|
| `patina_acpi`                  | ACPI provider/manager components; can also publish EDK II ACPI Table & SDT protocols.|
| `patina_adv_logger`            | "Advanced Logger" log buffer support (see crate `README.md`).                         |
| `patina_mm`                    | Management Mode (MM) support component.                                               |
| `patina_performance`           | FBPT performance measurement / reporting component.                                   |
| `patina_samples`               | Example/reference components used for tutorials and tests.                            |
| `patina_smbios`                | SMBIOS table production component.                                                    |
| `patina_test`                  | On-platform test runner harness (`cargo make patina-test`).                           |

Conventions specific to `components/**` are codified in
[`.github/instructions/components.instructions.md`](.github/instructions/components.instructions.md)
and [`docs/src/component/requirements.md`](docs/src/component/requirements.md):

- Apply `#[component]` to an `impl` block whose `entry_point(self, …) ->
  patina::error::Result<()>` declares the component's dependencies via
  parameter types (`Config<T>`, `ConfigMut<T>`, `Service<T>`, `Hob<T>`,
  `Commands`, `Handle`, `StandardBootServices`, `StandardRuntimeServices`,
  `&Storage`/`&mut Storage`, `Option<P>`, tuples).
- Register services with `#[derive(IntoService)]` + `#[service(dyn Trait)]`.
- Use the **stored dependencies pattern**: cache injected references in
  struct fields at initialization; methods call through stored fields.
- Prefer `Service<dyn Trait>` over `Service<ConcreteType>` for mockability.
- `ConfigMut<T>` components run while config is unlocked; once `lock()` is
  called, `Config<T>` consumers can run.
- Crate layout: no `mod.rs`, no public defs in `lib.rs`, required `component`
  module, optional `config`/`error`/`hob`/`service` modules.
- Test names: `test_<component_name>_*` (snake_case). Use `Config::mock`,
  `Service::mock`, `Hob::mock`, `Commands::mock` for entry-point tests.

### 3.2 `core/*` — Core-internal building blocks

Reusable internals consumed mainly by `patina_dxe_core`. Internal-only crates
use the `patina_internal_` prefix; the rest use `patina_`.

| Crate                          | Purpose                                                                              |
|--------------------------------|---------------------------------------------------------------------------------------|
| `patina_debugger`              | GDB Remote Serial Protocol debugger that installs into exception handlers.            |
| `patina_internal_collections`  | Collection types tuned for the core (no public re-export contract).                   |
| `patina_internal_cpu`          | CPU abstraction used by the core.                                                     |
| `patina_internal_depex`        | Dependency-expression evaluator (PI spec DEPEX).                                      |
| `patina_stacktrace`            | Stack-trace capture/decoding for the core.                                            |

### 3.3 `sdk/*` — Public SDK

The interface every component (in or out of this repo) depends on.

| Crate                          | Purpose                                                                              |
|--------------------------------|---------------------------------------------------------------------------------------|
| `patina`                       | Umbrella SDK: GUIDs, base types, Boot/Runtime Services, component infrastructure, logging/serial, re-exports `patina_macro`. Feature `core` exposes dispatcher internals; `std`, `alloc`, `mockall` toggles for host testing. |
| `patina_ffs`                   | Firmware File System (FFS) parsing/generation per the PI Specification.               |
| `patina_ffs_extractors`        | Section extractors (e.g., null extractor used in docs/examples).                      |
| `patina_macro`                 | Proc-macro crate re-exported by `patina`. Add new macros here; do not depend on it directly from components. |

### 3.4 `patina_dxe_core`

The DXE Core itself, plus the host-runnable `dxe_core_std` example (used by
`cargo make build-bin` / `run-bin`). Most platform-integration concerns
(memory map, boot/runtime services dispatch, image loading, MM bridging)
live here. Touching this crate has broad blast radius — coordinate with the
operator before sweeping changes.

---

## 4. Build, test, lint commands (cargo make)

Everything goes through `cargo make`. The tasks are defined in
[`Makefile.toml`](Makefile.toml) and use the pinned nightly toolchain.

Profile selector (`-p`):
- `-p development` (default) — debug profile.
- `-p release` — release profile.

Common scoping:
- Many tasks accept a positional package name (`cargo make build-x64 patina`)
  or `-- --features foo` to pass through to `cargo`.
- `-e FEATURES=feature1,feature2` enables features in builds.

### Core tasks

| Task                          | What it runs                                                                                          |
|-------------------------------|-------------------------------------------------------------------------------------------------------|
| `cargo make build`            | Alias for `build-std`: host-target build with `--features std`, including examples.                   |
| `cargo make build-lib`        | Host-target build of libs only (`--all-features`).                                                    |
| `cargo make build-x64`        | UEFI build for `x86_64-unknown-uefi` with `-Zbuild-std=core,compiler_builtins,alloc`.                |
| `cargo make build-aarch64`    | UEFI build for `aarch64-unknown-uefi`, same `build-std` flags.                                        |
| `cargo make build-bin`        | Host-target build of the `dxe_core_std` example.                                                      |
| `cargo make run-bin`          | Runs the `dxe_core_std` example on the host.                                                          |
| `cargo make check`            | Parallel `cargo check --all-targets --all-features` + `cargo test --no-run --all-targets`.            |
| `cargo make check-no-default-features` | Same as above but with `--no-default-features` (regression catch).                            |
| `cargo make test`             | `cargo test` (host). Accepts package & passthrough args via `--`.                                     |
| `cargo make patina-test`      | Builds crates with the `test-runner` feature for on-platform Patina tests.                            |
| `cargo make test-cov`         | `cargo llvm-cov` over the workspace (no report).                                                      |
| `cargo make test-asan`        | Runs tests with AddressSanitizer (Windows x64 / Linux only; otherwise skipped).                       |
| `cargo make coverage`         | `test-cov` + `coverage-lcov` + `coverage-html` → reports in `target/`.                                |
| `cargo make clippy`           | `cargo clippy --all-targets --all-features -- -D warnings`.                                           |
| `cargo make fmt`              | `cargo fmt --all` (use after every edit).                                                             |
| `cargo make fmt-check`        | `cargo fmt --all --check` (CI gate).                                                                  |
| `cargo make doc`              | `cargo doc … --features doc --no-deps`.                                                               |
| `cargo make doc-open`         | `cargo doc … --features doc --open`.                                                                  |
| `cargo make doc-test`         | `cargo test --doc`.                                                                                   |
| `cargo make cspell`           | Spell-check via `cspell` (requires `npm i -g cspell@latest`).                                         |
| `cargo make deny`             | `cargo deny check` per [`deny.toml`](deny.toml).                                                      |
| `cargo make vet`              | `cargo vet --locked` (depends on `generate-lockfile`).                                                |
| `cargo make bench`            | `cargo bench`, passes positional args via `--`.                                                       |
| `cargo make build-mdbook-deps`| Builds workspace libs under the `mdbook` profile so doctests can link.                                |
| `cargo make test-mdbook`      | Builds the mdbook, then runs its embedded doctests using the channel from `rust-toolchain.toml`.      |
| `cargo make serve-mdbook`     | `mdbook serve docs --open` on `http://localhost:3000`.                                                |
| `cargo make all`              | The full PR-readiness chain: `fmt-check, deny, cspell, clippy, check-no-default-features, build, build-x64, build-aarch64, patina-test, coverage, doc-test, doc, test-mdbook`. |

### Recipes the agent should reach for

- **After every edit:** `cargo make fmt`
- **Before declaring "done" on a small fix:**
  `cargo make fmt-check && cargo make clippy && cargo make check && cargo make test`
- **For UEFI-touching code:** also run `cargo make build-x64` and
  `cargo make build-aarch64`.
- **Before a hand-off PR:** `cargo make all` (long; mirrors CI).
- **Targeting one crate:** append the package name, e.g.
  `cargo make test -p patina`, `cargo make build-x64 patina_dxe_core`,
  `cargo make coverage dxe_core`.

### Toolchain prerequisites

These are listed in `[tools]` of [`rust-toolchain.toml`](rust-toolchain.toml)
and must be installed manually (or by CI):

```
cargo install cargo-make cargo-llvm-cov cargo-deny cargo-vet cargo-release
cargo install mdbook mdbook-admonish mdbook-linkcheck mdbook-mermaid
```

If `cargo make` is unavailable in your sandbox, you may fall back to the
underlying `cargo` invocations (visible at the top of each `[tasks.*]` block
in `Makefile.toml`), but call this out explicitly in your hand-off notes so
the operator can re-run with the canonical tooling.

### Verified in this environment

- `cargo fmt --all -- --check` (the underlying command of
  `cargo make fmt-check`) was executed in this workspace with the pinned
  nightly toolchain and exited 0.

Other `cargo make` tasks were **not** executed locally for this AGENTS.md
change because the change is documentation-only and full builds (`build-x64`,
`build-aarch64`, `coverage`, etc.) require substantial wall time and tools
not always present in agent sandboxes. The operator must run `cargo make all`
before merging any code change.

---

## 5. Coding conventions

The authoritative digest is
[`.github/copilot-instructions.md`](.github/copilot-instructions.md). The
sections below summarize the high-impact rules and link onward.

### 5.1 Module organization

- **Never use `mod.rs`.** Use a named module file (e.g., `src/memory.rs`)
  alongside its submodule directory (`src/memory/…`).
- **No public definitions directly in `lib.rs`** — only `pub mod` declarations
  and crate-level attributes. Put public types in named submodules.
- Crate naming: `patina_` for public crates, `patina_internal_` for internal
  crates, `_macro` suffix for proc-macro crates.
- See [`docs/src/component/requirements.md`](docs/src/component/requirements.md).

### 5.2 Safety (unsafe / MMIO / hardware)

- Prefer `zerocopy` for binary layouts over raw pointer/slice work.
- Minimize `unsafe`. Constrain it inside safe abstractions.
- Every `unsafe` block needs a `// SAFETY:` comment documenting
  preconditions, postconditions, and invariants. The workspace lints in
  `Cargo.toml` set `clippy::undocumented_unsafe_blocks = "warn"`.
- Mark functions `unsafe` only if the caller must uphold a contract the
  function cannot verify; otherwise validate inputs and keep the function
  safe.
- **MMIO:** never create `&T`/`&mut T` to MMIO space. Use
  [`safe-mmio`](https://github.com/google/safe-mmio) (`UniqueMmioPointer<T>`,
  `ReadPure`, `ReadPureWrite`, `ReadOnly`, `ReadWrite`, `WriteOnly`). See
  [`docs/src/dev/hardware_access/mmio.md`](docs/src/dev/hardware_access/mmio.md).
- Inline assembly: minimize; wrap architectural interfaces in safe Rust
  abstractions inside the SDK whenever practical.
- See [`docs/src/dev/principles/unsafe.md`](docs/src/dev/principles/unsafe.md)
  and [`docs/src/dev/principles/ffi.md`](docs/src/dev/principles/ffi.md).

### 5.3 Error handling

- Prefer `Result`. Avoid panics in production code.
- At UEFI ABI boundaries (`extern "efiapi"`), use `efi::Status`.
- Internally, use domain-specific Rust error types implementing `Debug`,
  `Display`, and `Error`. Add `From<>` conversions at boundaries.
- Use `expect("descriptive message")` over bare `unwrap()`. Reserve
  `unwrap()` for test code.
- See [`docs/src/dev/principles/error-handling.md`](docs/src/dev/principles/error-handling.md).

### 5.4 Component model

See §3.1 and:

- [`docs/src/component/getting_started.md`](docs/src/component/getting_started.md)
- [`docs/src/component/interface.md`](docs/src/component/interface.md)
- [`docs/src/component/requirements.md`](docs/src/component/requirements.md)

### 5.5 UEFI-specific rules

- Use `TplMutex` for shared-state synchronization. **Do not** use
  `spin::Mutex` or other non-TPL-aware primitives for shared state.
- Do not use `TplMutex` for interior mutability of non-shared data.
- Keep critical sections narrow.
- **No allocation or deallocation after `ExitBootServices`.** Code paths
  reachable from runtime services must avoid the global allocator.
- See [`docs/src/dxe_core/synchronization.md`](docs/src/dxe_core/synchronization.md),
  [`docs/src/dxe_core/memory_management.md`](docs/src/dxe_core/memory_management.md),
  [`docs/src/integrate/patina_dxe_core_requirements.md`](docs/src/integrate/patina_dxe_core_requirements.md).

### 5.6 Trait design

Traits are **abstraction points** for swappable behavior, not a code-reuse
mechanism (use crates for reuse). Keep traits focused (Interface
Segregation). See
[`docs/src/dev/principles/abstractions.md`](docs/src/dev/principles/abstractions.md)
and [`docs/src/dev/principles/reuse.md`](docs/src/dev/principles/reuse.md).

### 5.7 Documentation

- All public items must be documented. Crates set `#[deny(missing_docs)]`
  where appropriate; rustdoc is built with
  `RUSTDOCFLAGS="-D warnings -D missing_docs"`.
- Use `///` doc comments. Add `# Examples`, `# Errors`, `# Safety`,
  `# Panics` only when needed; avoid mechanical `# Arguments` / `# Returns`.
- Document **traits**, not implementations.
- See [`docs/src/dev/documenting.md`](docs/src/dev/documenting.md) and the
  [reference](docs/src/dev/documenting/reference.md).

### 5.8 Formatting

- `rustfmt` is the only authority, configured by
  [`rustfmt.toml`](rustfmt.toml). Run `cargo make fmt` after every edit.
- Markdown is linted with `markdownlint` via
  [`.markdownlint.yaml`](.markdownlint.yaml) (ignored paths in
  [`.markdownlintignore`](.markdownlintignore)).
- Spelling is checked with `cspell` per [`cspell.yml`](cspell.yml).

### 5.9 Common anti-patterns to flag

Lifted from `.github/copilot-instructions.md`:

1. Using `mod.rs` instead of named module files.
2. Raw slice/pointer manipulation where `zerocopy` would work.
3. Unnecessary `unsafe` without a safe abstraction wrapper.
4. `unwrap()` in production code (outside tests).
5. Cross-component dependencies (components must only depend on `sdk/`).
6. Adding public type definitions directly in `lib.rs`.
7. Missing documentation on public items.
8. Non-TPL-aware synchronization primitives for shared state.
9. `&T`/`&mut T` to MMIO instead of using `safe-mmio`.

---

## 6. Dependency management

- All third-party dependencies are vetted through `cargo-deny` (`deny.toml`)
  and `cargo-vet` (`supply-chain/`). Adding a new dependency means:
  1. Reviewing license, advisories, and provenance.
  2. Adding it to the workspace `[workspace.dependencies]` table in the root
     `Cargo.toml` and referencing it from member crates via `dep.workspace =
     true`.
  3. Running `cargo make deny` and `cargo make vet` locally (the latter may
     prompt you to record audits/exemptions under `supply-chain/`).
- Criteria for accepting a dependency:
  [`docs/src/dev/principles/dependency-management.md`](docs/src/dev/principles/dependency-management.md).
- Prefer workspace-wide version pinning over per-crate pinning so the
  workspace stays internally consistent.
- Many platform-critical crates (e.g., `patina_paging`, `patina_mtrr`,
  `mu_rust_helpers`, `safe-mmio`) live outside this repository under
  `OpenDevicePartnership` or upstream — bumping them is a deliberate act.

---

## 7. Testing strategy

- **Coverage target:** ≥80% (workspace and patch). CI gates on this.
- **Unit tests:** colocated with the code under `#[cfg(test)] mod tests`.
  Test names: `test_<component_name>_<scenario>` or
  `test_<service_name>_<scenario>` (snake_case).
- **Mocks:** prefer `mockall` (`#[automock]`). Put extension-trait helpers
  on traits so default methods don't break mock generation.
- **Pretty diffs:** use `pretty_assertions` for clearer failure output.
- **Component testing:** use `Config::mock`, `Service::mock`, `Hob::mock`,
  `Commands::mock` for entry-point unit tests. See
  [`docs/src/dev/testing.md`](docs/src/dev/testing.md) and
  [`docs/src/component/interface.md`](docs/src/component/interface.md).
- **On-platform tests:** `cargo make patina-test` builds the workspace with
  the `test-runner` feature so tests can run on a real or QEMU platform.
- **Mdbook doctests:** `cargo make test-mdbook` builds the mdbook profile
  (`build-mdbook-deps`) and links its doctests against
  `target/mdbook/deps/`. Always run this when editing `docs/`.
- **AddressSanitizer:** `cargo make test-asan` (Windows x64 / Linux only).
- **QEMU PR validation:** A separate workflow validates Patina against the
  Q35 QEMU platform on every PR. See
  [`docs/src/dev/testing/qemu_pr_validation.md`](docs/src/dev/testing/qemu_pr_validation.md)
  and the `patina-qemu-pr-validation*` workflows.
- **Benchmarks:** `criterion`-based, invoked via `cargo make bench` (see the
  Benchmarks section of `README.md`).

---

## 8. Working with `unsafe`, `no_std`, and feature gates

- Member crates set `#![cfg_attr(all(not(feature = "std"), not(test), …), no_std)]`
  and enable the nightly `#![feature(coverage_attribute)]`. When adding
  features, mirror this pattern instead of inventing your own.
- `extern crate alloc;` is gated on `alloc`/`test`/`std` features in most
  crates. Don't introduce unconditional `alloc` use in `no_std` paths.
- The `core` feature on `patina` exposes additional dispatcher internals.
  Component authors should not need it; integrators (DXE core, MM core) do.
- Build flags injected by `Makefile.toml`:
  - `NO_STD_FLAGS = --profile … -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem -Zunstable-options --timings`
  - `STD_FLAGS = --profile … --features std`
  - `COV_FLAGS = --workspace --profile test --ignore-filename-regex .*test.*`
- The `mdbook` profile inherits from `dev` but disables LTO so `rustdoc
  --test` can link rlibs.
- New unstable Rust features must follow
  [`docs/src/dev/unstable_feature.md`](docs/src/dev/unstable_feature.md) and
  [`docs/src/dev/unstable.md`](docs/src/dev/unstable.md); raise an issue
  using the `unstable_feature.yml` / `rustc_feature_gate.yml` templates as
  needed.

---

## 9. RFC process

Significant design changes go through an RFC. RFC drafts live under
[`docs/src/rfc/`](docs/src/rfc/). Open the PR with a `RFC:` title prefix
(see `CONTRIBUTING.md`). The Patina Release Process itself is described in
[`docs/src/rfc/text/0015-patina-release-process.md`](docs/src/rfc/text/0015-patina-release-process.md).
Agents must not invent process — point the operator at the RFC framework
when a change feels architectural.

---

## 10. CI overview

CI is centralized in
[`OpenDevicePartnership/patina-devops`](https://github.com/OpenDevicePartnership/patina-devops);
this repo only declares which reusable workflows to call.

Workflows in `.github/workflows/`:

- `ci-workflow.yml` — calls `patina-devops/.github/workflows/CiWorkflow.yml`
  with `build-tasks: build,build-x64,build-aarch64`. Includes the equivalent
  of `cargo make all` minus `vet`. Also calls `MdbookWorkflow.yml`.
- `calculate-unsafe-code.yml` — refreshes the unsafe-code badges on the
  `unsafe-code-badges` branch.
- `crate-version-update.yml` — generates a PR that bumps workspace crate
  versions to the current release-draft version.
- `publish-mdbook.yml` — publishes the mdbook to GitHub Pages.
- `publish-release.yml` — publishes the GitHub release and all crates to
  crates.io after the version-update PR merges.
- `pull-request-formatting-validator.yml`, `update-release-draft.yml`,
  `release-draft-config.yml` — PR/release-note hygiene.
- `rust-version-check.yml` — flags toolchain drift.
- `label-issues.yml`, `label-sync.yml`, `triage-issues.yml`,
  `advanced-issue-labeler.yml`, `dependabot.yml` — issue management /
  dependency updates.
- `patina-qemu-pr-validation*.yml` — QEMU-based PR validation triad
  (pending / main / post).

Treat CI as the source of truth: if `cargo make all` passes locally but CI
fails, the discrepancy is almost always toolchain drift, a missing feature
flag, or a sync-only workflow input.

---

## 11. Git, commits, branches

### Author identity (per operator)

This repository session uses the following identity, set **per command**
(never globally):

```powershell
git -c user.name="Felipe Balbi" `
    -c user.email="felipe.balbi@microsoft.com" `
    commit -s -m "<subject>" -m "<body>" `
    -m "Assisted-by: GitHub Copilot:claude-opus-4.7"
```

Required trailer on every AI-assisted commit:

```
Assisted-by: GitHub Copilot:claude-opus-4.7
```

Do not add a `Co-authored-by: Copilot <…>` trailer in this repository unless
the operator explicitly asks for one — the Patina AI policy requires the
human contributor to take responsibility for the change.

### Branches

- Default branch: `main`.
- Working branch for this sweep: `improve-agentic-workflow`.
- Push to the fork (`felipebalbi/patina`) only. **No force-push.**
- **Do not open a PR.** Stop at "pushed to fork" and hand off.

### Commit hygiene

- Conventional, present-tense subject lines (see the linked blog post in
  `CONTRIBUTING.md`).
- Sign-off (`-s`) is generally expected.
- One logical change per commit. Keep diffs reviewable.

---

## 12. Documentation (mdbook)

- Sources live in [`docs/`](docs/) (`docs/src/SUMMARY.md` is the index).
- Build: `cargo make doc` for rustdoc; `cargo make serve-mdbook` for the
  book; `cargo make test-mdbook` for embedded doctests (which require
  `build-mdbook-deps` first — the task chains it automatically).
- The mdbook is published by `publish-mdbook.yml` to
  <https://opendevicepartnership.github.io/patina/>.
- When you touch behavior covered by the book (component model, hardware
  access, error handling, testing, debugging, etc.), update the
  corresponding `docs/src/**/*.md` file in the same change.

Key entry points for agents:

- [`docs/src/dev/code_organization.md`](docs/src/dev/code_organization.md)
- [`docs/src/dev/principles/*.md`](docs/src/dev/principles/)
- [`docs/src/component/*.md`](docs/src/component/)
- [`docs/src/dxe_core/*.md`](docs/src/dxe_core/)
- [`docs/src/dev/testing*.md`](docs/src/dev/testing.md)
- [`docs/src/integrate/patina_dxe_core_requirements.md`](docs/src/integrate/patina_dxe_core_requirements.md)

---

## 13. Files synchronized from `patina-devops`

The following files (non-exhaustive) carry a header noting they are
auto-synced from `OpenDevicePartnership/patina-devops` and must be edited
**there**, not here:

- [`Makefile.toml`](Makefile.toml)
- [`rust-toolchain.toml`](rust-toolchain.toml)
- [`.github/workflows/ci-workflow.yml`](.github/workflows/ci-workflow.yml)
- Other workflows / sync configs as flagged by their headers.

The sync configuration lives at:
<https://github.com/OpenDevicePartnership/patina-devops/blob/main/.sync/Files.yml>

If an agent needs to change a synced file, it must:

1. Stop and tell the operator the change belongs in `patina-devops`.
2. Optionally produce a patch suitable for that repo, but do **not** edit
   the local copy expecting it to survive the next sync.

`AGENTS.md`, `README.md`, `CONTRIBUTING.md`, `docs/`, source code, and
`Cargo.toml` are all owned by this repository and may be edited here.

---

## 14. Custom agent/prompt assets

This repo already ships a few assets that agents and reviewers should be
aware of:

- [`.github/copilot-instructions.md`](.github/copilot-instructions.md) —
  canonical convention digest. **AGENTS.md links to this and does not
  contradict it.** When the two appear to disagree, the copilot-instructions
  file wins.
- [`.github/instructions/components.instructions.md`](.github/instructions/components.instructions.md)
  — auto-applied when editing `components/**`.
- [`.github/agents/reviewer.agent.md`](.github/agents/reviewer.agent.md) —
  a read-only "Patina Code Reviewer" persona that consumes the conventions
  above.
- [`.github/prompts/review.prompt.md`](.github/prompts/review.prompt.md) —
  the structured review checklist (module org, safety, error handling,
  component model, testing, documentation, UEFI semantics).

If you author new prompts/personas, place them under the appropriate
`.github/agents/` or `.github/prompts/` directory, keep them small and
declarative, and have them defer to `copilot-instructions.md` for the rules.

---

## 15. Things to avoid

- Touching synced files locally (§13).
- Adding cross-tier dependencies (`components → core`, `components →
  components`, `core → components`, `sdk → core`, `sdk → components`).
- Putting public items in `lib.rs`.
- Introducing `mod.rs`.
- Using `spin::Mutex`/`std::sync::Mutex` for shared state in the core.
- `unsafe` blocks without `// SAFETY:` documentation.
- `&T`/`&mut T` to MMIO; use `safe-mmio` types.
- Allocations on runtime-services paths after `ExitBootServices`.
- `unwrap()` in non-test code.
- Bypassing `cargo make` (it sets profile, target, `-Zbuild-std`, and
  feature flags that raw `cargo` will silently get wrong).
- Force-pushing branches; opening PRs into the upstream repo from an agent
  session.
- Storing secrets, tokens, or non-public information in commits or files.

---

## 16. Open questions / known gaps

- **MM Core support** is on the roadmap but not yet implemented. Don't
  fabricate APIs for it; defer to the RFC process.
- **Unstable feature inventory** lives in `docs/src/dev/unstable.md` and is
  the only authoritative list — when updating MSRV or removing
  `#![feature(...)]`, cross-check it.
- **AI policy interpretation** is enforced by maintainers; when in doubt
  about whether a contribution complies, ask the operator before generating
  more code.

---

*Last reviewed against repository state on the `improve-agentic-workflow`
branch. If you find this document drifting from reality, prefer fixing the
document in the same change as the underlying behavior.*

## Model selection & cost discipline

Premium models (Opus, GPT-5 family, "high"/"xhigh" reasoning variants)
cost an order of magnitude more than standard models (Sonnet, Haiku,
mini). Most steps in a typical task do not need premium reasoning,
and over-using premium models wastes credits without improving
outcomes. The rules below apply to *all* model selection: your own
session, sub-agents launched via the `task` tool, and parallel work
launched via `/fleet`.

### Default posture

- **Default to the cheapest model that can do the job.** Reach for a
  premium model only when one of the escalation triggers below is hit.
- **Plan with premium, execute with cheap.** Spend at most one or two
  premium turns on design / planning, then downshift to a cheaper
  model for mechanical execution of the plan.
- **Never bump the model "just in case."** If you cannot articulate
  *why* a cheaper model would fail, use the cheaper model.

### Escalation triggers (use a premium model)

Reach for a premium model when *any* of these are true:

- Cross-module refactor, architectural design, or API design from
  scratch.
- Subtle correctness reasoning: concurrency, lifetimes, `unsafe`,
  FFI ABI, cryptography, safety-critical control paths.
- Debugging a failure that survived one prior cheap-model attempt.
- Reviewing code on a safety-, security-, or money-critical path.
- The diff cannot be predicted in advance — i.e. there is genuine
  creative or design work to do, not just typing.

### De-escalation triggers (use a cheap model)

Use the cheapest available model when *any* of these are true:

- Searching, reading, summarising files or docs.
- Single-file mechanical edits: rename, format, lint fix, dependency
  bump, boilerplate, scaffolding from a known template.
- Generating tests for code that already works.
- Running builds, tests, linters, or other commands where the model
  only needs to report success/failure.
- Routine commits, PR descriptions, changelog entries.
- The diff is essentially predictable before generation.

### Sub-agent routing (the `task` tool)

When delegating with the `task` tool, set `model:` explicitly. Do not
let sub-agents inherit a premium default for cheap work.

| Sub-agent type    | Default model             | Override to                                     |
|-------------------|---------------------------|-------------------------------------------------|
| `explore`         | cheap                     | keep cheap (`claude-haiku-4.5` or `gpt-5-mini`) |
| `task` (run cmd)  | cheap                     | keep cheap                                      |
| `research`        | cheap for breadth         | premium only for the final synthesis            |
| `general-purpose` | match task                | cheap for mechanical work; premium for design   |
| `rubber-duck`     | premium                   | keep premium — this is where reasoning pays off |
| `code-review`     | premium on critical paths | cheap on cosmetic / mechanical diffs            |

### `/fleet` (parallel sub-agents) rules

- Fleet mode multiplies cost by the fleet width. Apply the rules
  above *per worker*, not in aggregate.
- Split a fleet job along complexity lines: route the cheap,
  parallelisable workers (file edits, test runs, doc updates) to a
  cheap model; reserve premium models for the small number of
  workers that need real reasoning.
- If every worker in a fleet would need a premium model, the work is
  probably not a good fit for fleet mode — reconsider the
  decomposition before paying N× premium.

### Session hygiene

- Keep sessions short and focused. Long premium sessions are the
  single largest source of waste because every turn re-processes the
  full history.
- Use `/compact` when the conversation grows long, and `/new` for
  unrelated work.
- Prefer `/ask` for one-off side questions so they don't extend the
  main session.

### When in doubt

Ask: *"If a cheaper model produced the wrong answer here, would I
catch it in seconds (compiler, tests, my own review) or in
weeks (production incident)?"* If the former, use the cheap model
and let the feedback loop do its job.
