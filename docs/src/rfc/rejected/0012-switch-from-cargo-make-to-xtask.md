# RFC: Switch from cargo-make to xtask

This RFC outlines the limitations of using `cargo-make` setup and discusses how
adopting [`xtask`](https://github.com/matklad/cargo-xtask) can provide a more
flexible and maintainable build and tooling infrastructure.

## Change Log

- 2025-06-11: Initial draft of the RFC.
- 2025-07-25: RFC Rejected

## Motivation

Patina and its associated repositories have been using `cargo-make` as the build
task runner since the beginning. While its TOML-based syntax is suitable for
simple tasks, more complex workflows often require using its custom scripting
language, `duckscript`. These include scenarios like parsing arguments as
package arguments, passing values through environment variables, or the patch
build task from other patina repo. Even basic actions, such as listing available
tasks with descriptions (`cargo make --list-all-steps`), are not
straightforward, and the output is not particularly intuitive. `cargo-make`
supports task dependencies, but its flexibility is limited, and writing
non-trivial workflows in `Makefile.toml` can become tedious over time.

`cargo make all` is our current recommended usage, but it does not manage the
installation of all prerequisite tools, such as `cargo-make` itself,
`cargo-tarpaulin`, `cargo-deny`, and `cspell` which must be installed manually.

The need for a better task runner became apparent while working on the following
PR [#497](https://github.com/OpenDevicePartnership/patina/pull/497), where we
aimed to introduce a more actionable testing tool—`nextest`. The current setup
relies on `cargo test`, whose output is subpar compared to what `nextest`
offers. However, since `nextest` is an external tool that must be installed
manually, the PR understandably received some pushback. I think not using the
right tool holds us back. It makes it harder to improve the developer
experience, and more importantly, we miss the chance to make test results more
actionable. To prove the point, using `nextest` we were able to uncover faulty
test cases that had been incorrectly passing. These were later identified and
fixed in [#491](https://github.com/OpenDevicePartnership/patina/pull/491) and
[#495](https://github.com/OpenDevicePartnership/patina/pull/495). Setting up of
any such additional prerequisite tools should be automated and included as the
first command a user runs—ideally as a one-time setup step.

While `cargo-make` covers most of our current needs, setting up environment
variables and crafting the correct command-line arguments for tasks has become
increasingly cumbersome. On top of that, Even something as simple as rerunning a
failed test by name is difficult due to the current rigid conventions imposed in
the `Makefile.toml`.

```cmd
C:\r\patina>cargo make test -p patina_dxe_core gcd::tests::test_full_gcd_init
...
[cargo-make] INFO - Execute Command: "cargo" "test" "-p" "-p;patina_dxe_core;gcd::tests::test_full_gcd_init"
error: invalid character `;` in package name: `;patina_dxe_core;gcd`, the first character must be a Unicode XID start character (most letters or `_`)
[cargo-make] ERROR - Error while executing command, exit code: 101

C:\r\patina>cargo make test patina_dxe_core gcd::tests::test_full_gcd_init
...
[cargo-make] INFO - Execute Command: "cargo" "test" "-p" "patina_dxe_core;gcd::tests::test_full_gcd_init"
error: invalid character `;` in package name: `patina_dxe_core;gcd`, characters must be Unicode XID characters (numbers, `-`, `_`, or most letters)
[cargo-make] ERROR - Error while executing command, exit code: 101
```

The primary motivation for choosing `xtask` is to establish a clear and
actionable workflow for building, testing, and running Patina, while also
future-proofing our setup. This not only improves the onboarding experience but
also maintains flexibility for advanced users who need to customize execution
through additional command-line arguments or extend workflows without rigid
configuration constraints.

   > "There should be one and preferably only one obvious way to do it." -Zen of Python

## Who is using `xtask`?

An `xtask` based build setup is used by many well-known Rust projects,
including:

- [cargo](https://github.com/rust-lang/cargo/blob/master/crates/xtask-build-man/src/main.rs)
- [rust-analyzer](https://github.com/rust-lang/rust-analyzer/tree/master/xtask)
- [uefi-rs](https://github.com/rust-osdev/uefi-rs/tree/main/xtask)
- [openvmm](https://github.com/microsoft/openvmm/tree/main/xtask)
- [ratatui](https://github.com/ratatui/ratatui/tree/main/xtask)
- [IronRDP](https://github.com/Devolutions/IronRDP/tree/master/xtask)
- [microbit](https://github.com/nrf-rs/microbit/tree/main/xtask)
- many others across the Rust ecosystem.

## Technology Background

Even though `cargo-make` is a capable build task runner, `xtask` presents a
significantly more flexible and maintainable alternative. Before diving into how
we can leverage it, it’s important to clear up some common misconceptions about
what `xtask` actually is:

1. **Not an External Tool**: Unlike `cargo-make`, `xtask` is not a third-party
   tool or crate that we install from crates.io. This means it introduces
   **zero additional dependencies** upfront.

2. **Convention**: `xtask` is simply a **convention** for organizing a custom
   binary crate inside the Rust workspace. The structure typically looks like
   this:

   ```text
   patina/
   ├── .cargo/config.toml     # Defines alias: xtask = "run --package xtask --"
   ├── xtask/                 # A binary crate containing build CLI logic
   │   └── src/main.rs
   ├── patina-specific-crates/
   └── Cargo.toml             # Includes xtask in [workspace] members
   ```

3. **Standard Rust Code**: The `xtask` crate is just another Rust binary that
   uses standard library features, particularly `std::process::Command`, to
   orchestrate build operations by spawning OS-level processes.

4. **Cargo Alias Integration**: To make running `xtask` feel native, it's
   recommended to define a Cargo alias like so:

   ```toml
   [alias]
   xtask = "run --package xtask --"
   ```

   This enables commands like `cargo xtask ...`, where everything after `xtask`
   is passed as arguments to the `xtask` binary.

5. **Execution**: When we run `cargo xtask ...`, it effectively invokes
   `target/debug/xtask.exe ...`.

6. **Naming**: There’s nothing special about the name `xtask`. We could name
   the crate `patina` or even `make` though the latter is discouraged to avoid
   confusion. That said, `xtask` has become the **conventionally accepted name**
   used by many Rust projects.

7. **Fully Customizable in Rust**: Since the build orchestration logic lives in
   real Rust code, it's type-safe, testable, and easier to evolve over time
   unlike TOML-based configurations or shell scripts.

8. **No Custom Tooling Sections Needed**: We don’t need `[tool]` sections in
   TOML or separate shell scripts to install tools. Instead, we can simply run
   `cargo xtask setup`, where `setup` is a custom task we author to handle
   installations.

9. **Unified Local and CI Workflows**: With `xtask`, We can streamline the local
   and CI workflows, ensuring they behave consistently with minimal duplication
   or special cases.

10. **Startup and Performance**: Since `xtask` is purpose-built for the
    specific repository, it avoids the overhead of generic task runners like
    `cargo-make`. This specialization makes it faster to start up and execute
    tasks, as it's compiled along with the workspace and tailored to just what
    the repo needs, no plugin resolution, runtime scripting, or extra setup.

## Goals

1. Ensure that new users can set up the prerequisite tools and environment for
   Patina with minimal effort and a consistent workflow across platforms and
   architectures.
2. Being able to author build tasks more reliably and make them easy to extend
   over time.
3. Eliminate stale `Makefile.toml` tasks.
4. Future-proof Patina’s build and tooling infrastructure.
5. Each Patina repo has slightly different build requirements, so each repo can
   adopt the conventions used in this one. However, trying to completely reuse
   the `xtask` across repos can be over-engineering and make it hard to evolve.
   The spirit of `xtask` is in many ways similar to traditional Makefiles -
   Custom implementation for each repo but with the added benefit of improved
   reliability and maintainability by authoring them in Rust, not necessarily
   aiming for reusability.

## Requirements

1. Maintain a one-to-one mapping with the current `cargo-make` `<tasks>` to
   ensure a smooth transition.
2. Update documentation accordingly, though we expect only minimal changes to
   the existing workflow.

## Unresolved Questions

## Current Build Setup

Here are the build tasks currently defined in the `Makefile.toml` at the root of
the Patina workspace:

- `all`                         - Runs all required tasks before raising a PR (most commonly used)
- `build`                       - Currently redundant; aliased to `build-std`
- `build-aarch64`               - Builds the AArch64 flavor of Patina
- `build-bin`                   - Not used
- `build-std`                   - Not used
- `build-x64`                   - Builds the x64 flavor of Patina
- `check`                       - Custom task to support Rust Analyzer in VSCode for Patina
- `check_no_std`                - Used by `check`
- `check_std`                   - Used by `check`
- `clippy`                      - Used indirectly by the `all` task
- `coverage`                    - Used indirectly by the `all` task
- `coverage-fail`               - Not used
- `coverage-fail-package`       - Not used
- `coverage-filter`             - Not used
- `cspell`                      - Used indirectly by the `all` task
- `deny`                        - Used indirectly by the `all` task
- `doc`                         - Used indirectly by the `all` task
- `doc-open`                    - Used indirectly by the `all` task
- `fmt`                         - Used indirectly by the `all` task
- `individual-package-targets`  - A Duckscript-based task for splitting command-line options (very limiting)
- `run-bin`                     - Not used
- `test`                        - A `cargo test` based task; not very actionable

As we can see, several of these tasks are either unused or rarely used.
Additionally, since the tasks are authored using TOML syntax, implementing
control flow logic such as parsing command-line arguments, injecting environment
variables (like `RUSTC_BOOTSTRAP=1`), or passing custom arguments is cumbersome
and limiting.

## How Does `xtask` Solve These Problems?

To begin with, `xtask` is simply a regular Rust binary crate. It does not have
the notion of a build task. Instead, it follows the Rust ecosystem conventions
to structure and orchestrate build logic.

At the core, the `src/main.rs` file acts as a dispatcher. It parses the task
name passed via the command line (`cargo xtask <task-name>`) and delegates
execution to corresponding Rust functions, each implemented in its own module:

```rust
fn main() {
    if let Err(e) = try_main() {
        eprintln!("{}", e.to_string().bright_red());
        std::process::exit(-1);
    }
}

fn try_main() -> Result<(), DynError> {
    let task = env::args().nth(1);                 // This is how we access the argument passed to `cargo xtask`
    match task.as_deref() {                        // This is how we branch off to individual tasks(in other words separate module)
        Some("all") => all()?,                     // This is defined in `src\all.rs` it essentially call all below tasks
        Some("build-aarch64") => build_aarch64()?,
        Some("build-x64") => build_x64()?,         // For example: build_x64() is defined in `src\build_x64.rs`
        Some("check") => check()?,
        Some("ci") => ci()?,                       // task to call more tasks to perform ci related operations
        Some("clippy") => clippy()?,
        Some("coverage") => coverage()?,
        Some("cspell") => cspell()?,
        Some("deny") => deny()?,
        Some("docs") => docs()?,
        Some("fmt") => format()?,
        Some("help") => print_help(),
        Some("test") => test()?,
        Some("setup") => setup()?,                 // setup task to install any prerequisite tools
        _ => print_help(),
    }
    Ok(())
}
```

For example, here’s how the `build-x64` task is implemented in
`src/build_x64.rs`:

```rust
pub(crate) fn build_x64() -> Result<(), DynError> {
    println!("─────────────────────────────────");
    println!("{}", "🚀 Running: x64 - cargo build".bright_green());

    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

    // Spawn cargo build process for x64
    let status = Command::new(&cargo)     // Build cargo command
        .current_dir(project_root())
        .env("RUSTC_BOOTSTRAP", "1")      // Setup the environment
        .args([                           // Pass additional arguments to build x64 patina specific targets
            "build",
            "--target",
            "x86_64-unknown-uefi",
            "-Zbuild-std=core,compiler_builtins,alloc",
            "-Zbuild-std-features=compiler-builtins-mem",
            "-Zunstable-options",
            "--timings=html",
        ])
        .args(env::args().skip(2))        // Pass through any user specified arguments
        .status()?;

    if !status.success() {
        Err("❌ Failed: x64 cargo build")?;
    }

    println!("{}", "✔️    Done: x64 cargo build".bright_green());

    Ok(())
}
```

### Key Benefits of Using `xtask`

1. **All logic is written in Rust** - no more `duckscript` hacks.
2. **Intuitive control flow** - unlike TOML, control flow is now directly
   expressed in code(Rust).
3. **Easy to express task dependencies** - unlike TOML, expressing dependencies
   among tasks become function calls.
4. **Cleaner module structure** - each task resides in its own Rust module,
   improving clarity and maintainability.
5. **Leverage Rust crate ecosystem** - Can access any crate from `crates.io` if
   needed.
6. **Easy customization** - updating build flags or environment variables is
   straightforward.
7. **Argument forwarding** - pass additional CLI arguments without the need for
   parsing.
8. **Better setup experience** - a `setup` task can handle all installation of
   prerequisite tools like `cargo-deny`, `cargo-tarpaulin`, or `cargo-nextest`.
9. **Extensibility** – Easily add new tasks such as binary size tracking, Git
   commit hooks, and more, and enhance existing tasks with richer command-line
   options.
10. **Local vs. CI Parity** – Unify local and CI workflows under a single,
    consistent interface. All CI-related operations can be encapsulated in a
    `ci` task and invoked using `cargo xtask ci` in the pipeline.

Though `xtask` might seem laborious, it gives us far more control and
flexibility in authoring and extending the tasks for any future needs.

An initial implementation (including `cargo xtask all`) is available in a [WIP
branch](https://github.com/OpenDevicePartnership/patina/compare/main...users/vineelko/xtask_0610)
and has been tested on Windows. Additional validation, especially for the
`setup` task, is still in progress.

```cmd
C:\r\patina>tree xtask
xtask
├── Cargo.toml
└── src
    ├── all.rs
    ├── build_aarch64.rs
    ├── build_x64.rs
    ├── check.rs
    ├── ci.rs
    ├── clippy.rs
    ├── coverage.rs
    ├── cspell.rs
    ├── deny.rs
    ├── docs.rs
    ├── format.rs
    ├── help.rs
    ├── main.rs
    ├── setup.rs
    ├── test.rs
    └── util.rs
```

### Get help for existing tasks

```cmd
C:\r\patina>cargo xtask help
Usage: `cargo xtask <task> [options]`
Tasks are run in the root of the repository.

Tasks:
all           Run all task before drafting a PR
build-aarch64 Build the project for aarch64
build-x64     Build the project for x86_64
check         Run cargo check
ci            Run all ci related tasks
clippy        Run cargo clippy
coverage      Generate code coverage report
cspell        Print words that cspell does not recognize
deny          Run cargo deny
docs          Generate documentation
fmt           Run cargo fmt
help          Print this help message
setup         Install prerequisite tools
test          Run tests

Options:
Task specific cargo options can be passed after the task name, e.g.:
cargo xtask build-x64 --release
cargo xtask doc --open
```

## Alternatives

- Keep using `cargo-make`
  - This option is not recommended, as this the right time to settle on a more
    stable build and tooling infrastructure.

## Rejection Summary

The community deemed that this RFC added more complexity to the build system than was desireable. The goal was to
address shortcoming of `cargo-make` and only if those cannot be addressed should we move to other tools.
