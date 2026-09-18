# RFC: Run Tests with `cargo-nextest`

This RFC proposes replacing `cargo test` with [cargo-nextest](https://nexte.st/) as the test runner behind Patina's
`cargo make` test tasks, while keeping `cargo test` for running doctests since nextest can't run them.

## Change Log

- 2026-08-06: Initial draft of RFC.
- 2026-08-11: Update "Unresolved Questions" to indicate that `#[serial]` should be removed in cases where nextest's
  isolation is sufficient. Add details about how nextest works with Miri in Technology Background.

## Motivation

Patina's has a few test-related tasks in `Makefile.toml`:

- `test`
- `check_tests`
- `check-no-default-features-tests`
- `test-asan`
- Coverage tasks

Those run tests through plain `cargo test`. `cargo test` works, but its output is hard to read at a glance for a
workspace this size. It does not give a summary of which tests were slow, and offers no way to retry a test that fails
intermittently.

In the past, a concern was that nextest is an external tool that had to be installed by hand. Patina's `Makefile.toml`
now has an `install-tools` task that reads a `[tools]` table in `rust-toolchain.toml` and installs everything listed
there automatically, so that particular objection no longer applies. This RFC picks the nextest idea back up on its own,
without tying it to a larger build system rewrite.

## Technology Background

[cargo-nextest](https://nexte.st/) is a cargo subcommand that discovers and runs the same tests `cargo test` does,
invoked as `cargo nextest run` instead of `cargo test`. It reads the same package, feature, and target selection flags,
so most of the command line usage Patina already has does not need to change.

The core difference is how tests are executed. `cargo test` builds one binary per test target and runs every `#[test]`
function inside it as a thread within a single process. `cargo nextest run` runs every individual test
[in its own operating system process](https://nexte.st/docs/design/why-process-per-test/). This has a few consequences:

The first is isolation. If one test panics in a way that corrupts shared memory, triggers a hard abort, or segfaults,
only that one process is affected. The rest of the run keeps going, and nextest reports exactly which test caused the
failure. Patina already runs an `AddressSanitizer` pass over its tests (`cargo make test-asan`). Under `cargo test`, an
ASan abort in one test can prevent further results from being reported in the binary. Using nextest, that test fails on
its own and every other test still reports a result.

The second is that Nextest cannot run doctests, because doing so requires unstable compiler support that only works
through the ordinary `cargo test` harness today. This is a known, permanent restriction tracked at
[nextest-rs/nextest#16](https://github.com/nextest-rs/nextest/issues/16), not something specific to Patina. Doctests
need to keep running through `cargo test --doc`.

The third is that Patina uses the [`serial_test`](https://crates.io/crates/serial_test) crate's `#[serial]` attribute in
several places (`patina_adv_logger`, `patina_debugger`, the `patina_internal_cpu` interrupt tests, and the
`patina_dxe_core` allocator usage tests) to stop specific tests from running at the same time as each other. `#[serial]`
works by holding a lock inside the process. `serial_test`'s documentation recommends its `file_serial` variant instead
for tests that run as separate processes. Since nextest always runs tests as separate processes, `#[serial]` no longer
prevents two tests from running concurrently under nextest. In cases reviewed so far, the state those tests guard is a
static that already lives entirely inside one process, so a new process isolates it on its own and the attribute appears
to be redundant rather than actively wrong, but this has not been checked for every use of `#[serial]` in the repository
though. See [Unresolved Questions](#unresolved-questions).

Nextest also adds a few capabilities `cargo test` does not have. It can automatically retry a test a set number of times
and report whether it passed on retry, which is useful for flaky tests. It can produce JUnit XML output for tooling that
expects it (like CI dashboards). It has a filter expression language for selecting tests by package, binary, or name
pattern, that's more sophisticated than what `cargo test` supports. It also emulates enough of `cargo test`'s own
command line arguments, including `-- --nocapture`, that existing arguments mostly carry over as long as a recent enough
nextest version is used, which would be the case going forward in Patina. It is a well-supported, popular tool in the
Rust ecosystem.

A final note is that nextest is beneficial for running tests under [Miri](https://github.com/rust-lang/miri). As
described in [cargo-nextest - Miri and nextest](https://nexte.st/docs/integrations/miri/), because nextest runs each
test in its own process and Miri is single-threaded, nextest can run multiple Miri tests in parallel across processes
which can improve test run performance by 3-4x.

There is also a caveat to be aware of:

> Note, however, that cargo miri test is able to detect data races where two tests race on a shared resource. Miri with
> nextest will not detect such races.

## Goals

1. Give clearer, more actionable output for test runs than `cargo test` provides today.
2. Reduce the chance that a crash or resource conflict in one test hides or corrupts the results of other tests.
3. Keep running the same set of tests Patina runs today, with the exception of doctests.
4. Let every contributor and CI pipeline pick up the tool automatically, with no manual install step.
5. Get a small overall reduction in total test execution time.

Since nextest can run tests in parallel across processes rather than across threads within a single process, it was
found to provide a slight overall reduction in total test execution time.

- `cargo test` average: 29.18s
- `cargo nextest run` average: 26.65s

> These numbers do not include the compilation tests (in `sdk/patina/tests/compile_fail_tests.rs`). They were taken
> on a Windows 11 machine and only their relative difference is shown to be meaningful.

## Requirements

1. Every test that runs under `cargo test` today must keep running, except doctests, which nextest cannot execute and
   which continue to run through the existing `doc-test` task.
2. `cargo-nextest` must install automatically through the existing `cargo make install-tools` task, with no separate
   step for contributors to follow by hand.
3. Coverage collection through `cargo-llvm-cov` must keep working, including coverage collected from doctests.
4. Existing developer workflows, such as running a single test by name, scoping a run to one package, or asking for
   output on a passing test, must keep working from the command line.
5. AddressSanitizer testing (`cargo make test-asan`) must keep exercising both ordinary tests and doctests.

## Unresolved Questions

1. Should Patina adopt nextest's test group feature as a replacement for `#[serial]`? This is treated more as a follow
   up rather than something this RFC needs to resolve.

   - The decision at the time of the RFC is remove `#[serial]` in cases where cargo-nextest's isolation is clearly
     sufficient.

## Prior Art (Existing PI C Implementation)

We've briefly considered other test runners as noted in the RFC introduction but never made a decision to adopt one.

## Alternatives

1. Stay with `cargo test`. This avoids adding a new tool, but keeps the output and process isolation limitations
   described above, and does not address the concern that originally motivated PR 497.
2. Write custom tooling around `cargo test` to improve its output, for example a wrapper that parses and reformats its
   results. This would take ongoing effort to build and maintain, and would still run every test in a shared process, so
   it would not gain the isolation or performance benefits nextest provides.

## Rust Code Design

This proposal is a build and test tooling change. It does not require changes to production Patina source code. The
changes are as follows.

In `rust-toolchain.toml`, add `cargo-nextest` to the `[tools]` table, the latest so `cargo make install-tools` installs
it the same way it installs `cargo-llvm-cov`, `cargo-deny`, and the other pinned tools.

In `Makefile.toml`:

1. `test`, `check_tests`, and `check-no-default-features-tests` run `cargo nextest run` instead of `cargo test`.
2. `test-asan` runs its ordinary tests through `cargo nextest run`, then runs a second, separate `cargo test --doc`.
3. `test-cov` collects coverage for ordinary tests through `cargo llvm-cov nextest`, and a new task collects doctest
   coverage separately through `cargo llvm-cov --doc`, since nextest cannot run doctests itself. The report generation
   tasks (`coverage-lcov`, `coverage-html`) pass `--doctests` so both sets of collected data are merged into one report.
4. `doc-test` is unchanged. It keeps running `cargo test --doc` because that is the only way to run doctests at all.

## Guide-Level Explanation

Running tests looks almost the same as it does today. `cargo make test`, `cargo make test -p patina_dxe_core`, and
`cargo make test -p patina_dxe_core -- --nocapture` all keep working the way they do now.

`cargo make check`, `cargo make coverage`, and `cargo make test-asan` also keep working without any change to how you
invoke them. The output changes and includes additional information. Instead of a stream of dots, you get a pass or fail
line per test with how long it took, and a summary at the end.

`cargo make test` no longer runs doctests as part of the same command. Nextest cannot run them, so they continue to run
through `cargo make doc-test`, which already existed as its own task before this change.

If you write a test that depends on another test having already run and left some state behind, nextest is likely to
catch that quickly, because every test starts in its own process with no leftover state from anything else. If you hit
this, the fix is the same one used there, reset the shared state explicitly at the start of the test rather than relying
on it having already been set up by something else.
