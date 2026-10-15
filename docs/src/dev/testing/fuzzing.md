# Fuzzing

[Fuzzing](https://rust-fuzz.github.io/book/) feeds a program a large number of automatically generated inputs and
watches for crashes. Unit tests check the inputs that someone thought of. A fuzzer finds the inputs that no one thought
of, which makes it especially valuable for code that parses data from an untrusted source.

Patina fuzzes with [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz), which uses
[libFuzzer](https://llvm.org/docs/LibFuzzer.html). The fuzz targets run every night in GitHub Actions (see
[Nightly Fuzzing](#nightly-fuzzing)) and can be run locally.

## Running the Fuzz Targets Locally

libFuzzer requires Linux. On Windows, use [WSL](https://learn.microsoft.com/windows/wsl/) and run the fuzz tasks from
there; the tasks fail with an explanation on any other operating system. A C++ compiler (such as `g++` or `clang`) is
also required to build libFuzzer.

`cargo-fuzz` is one of the tools pinned in `rust-toolchain.toml`, so it is installed with the rest of the tools by
running `cargo make install-tools` from the repository root. The fuzz targets are built with the toolchain pinned in
`rust-toolchain.toml`, so no nightly toolchain is needed.

The fuzz tasks are in `fuzz/Makefile.toml` and are run from the `fuzz` directory:

```text
cd fuzz
cargo make fuzz pecoff_parse  # Fuzzes pecoff_parse for 60 seconds.
```

Each `fuzz` starts from the inputs the target has already found (`fuzz/corpus/<target>/`) and from any seed inputs the
target has (see [Adding a Fuzz Target](#adding-a-fuzz-target)). Neither the corpus nor a crash is checked in. A crash is
saved to `fuzz/artifacts/<target>/` and fails the task; see [Investigating a Crash](#investigating-a-crash).

| Task                                   | Description                                                                  |
| -------------------------------------- | ---------------------------------------------------------------------------- |
| `cargo make fuzz <target> [args]`      | Fuzzes a target for `FUZZ_SECONDS`. Extra arguments are passed to libFuzzer. |
| `cargo make reproduce <target> <file>` | Runs a target on one input, such as a crash.                                 |
| `cargo make minimize <target>`         | Removes corpus inputs that do not add coverage.                              |
| `cargo make build [target]`            | Builds the fuzz targets without running them.                                |

These environment variables control the tasks:

| Variable       | Default  | Description                                                                   |
| -------------- | -------- | ----------------------------------------------------------------------------- |
| `FUZZ_SECONDS` | `60`     | Seconds to fuzz one target.                                                   |
| `FUZZ_MODE`    | `dev`    | The build configuration: `dev`, `test` or `release`. See below.               |
| `FUZZ_MAX_LEN` | `262144` | The longest input in bytes. Raise it for a target whose inputs can be larger. |

For example, to fuzz in the release configuration for ten minutes:

```text
FUZZ_MODE=release FUZZ_SECONDS=600 cargo make fuzz pecoff_load
```

### Build Configurations

The build configuration is selected with `FUZZ_MODE`:

- `dev` (the default): Integer overflow checks and debug assertions are enabled, like Patina's `dev` profile. An
  arithmetic wrap is a panic, and so is a failed `debug_assert!` (including `log_debug_assert!`).
- `test`: Overflow checks are enabled and debug assertions are disabled, like Patina's `test` profile. An arithmetic
  wrap is a panic, but a debug assertion that crafted input can reach does not stop the run.
- `release`: Both are disabled, like Patina's `release` profile. This is the code that ships.

The nightly workflow fuzzes `test` and `release`. Overflow checks make a wrap that a release build ignores very easy to
find, and the `release` configuration finds what remains when they are off. `dev` is for investigating a debug
assertion locally.

## What Is Fuzzed

The targets are in the [`fuzz`](https://github.com/OpenDevicePartnership/patina/tree/main/fuzz) directory.

| Target         | Code under test                                                                                             |
| -------------- | ----------------------------------------------------------------------------------------------------------- |
| `pecoff_parse` | `UefiPeInfo::parse`, `parse_mapped` and `validate_sections`: the checks every image gets first.             |
| `pecoff_load`  | The rest of image processing: reading the HII resource section, `load_image`, and `relocate_image` (twice). |

## Nightly Fuzzing

The [Nightly Fuzzing](https://github.com/OpenDevicePartnership/patina/blob/main/.github/workflows/nightly-fuzzing.yml)
workflow runs every night and can also be started manually from the Actions tab (**Run workflow**), where the number of
seconds to fuzz each target can be set. It runs one job for each combination of fuzz target and build configuration.
The fuzz targets are read from `fuzz/Cargo.toml`, so a new target is fuzzed without changing the workflow. It does not
run on pull requests.

- The corpus of each target is cached between runs, so each night continues from the coverage that the previous nights
  reached instead of starting over. The corpus is minimized before it is saved so that it does not grow without bound.
- A crash fails the job. The crashing input and the log are uploaded as the `fuzz-crashes-<target>-<mode>` artifact,
  and the job summary shows the panic message.

## Investigating a Crash

A crash comes from either a local run or the nightly workflow.

1. Get the crashing input, a file named `crash-<hash>`.
   - From a local run, it is in `fuzz/artifacts/<target>/`, and the path is printed when the crash is found.
   - From the nightly workflow, download the `fuzz-crashes-<target>-<mode>` artifact from the failed run.
2. Reproduce it in the same build configuration that found it. A crash from the nightly workflow names it in the
   artifact. For a local run, it is the `FUZZ_MODE` you used (`dev` unless you set it).

   ```text
   cd fuzz
   FUZZ_MODE=<mode> cargo make reproduce <target> <path to crash-file>
   ```

   The panic message and a stack trace are printed.
3. Fix the bug and add a unit test that fails without the fix. The crashing input is a good starting point, but write
   the test so that it builds only the bytes that matter. The input can be reduced first with `cargo fuzz tmin`.

## Adding a Fuzz Target

1. Add `fuzz/fuzz_targets/<area>_<name>.rs` and a matching `[[bin]]` entry in `fuzz/Cargo.toml`. The name starts with
   the area, such as `pecoff_`, because the area selects the optional files in the next step.
2. If the code under test is private, expose it from its crate only under `#[cfg(fuzzing)]`, as `pecoff` is in
   `patina_dxe_core`.
3. Optionally, add files that help the fuzzer. They are chosen by the area, so every target in the area uses them:
   - `fuzz/dictionaries/<area>.dict`: a [libFuzzer dictionary](https://llvm.org/docs/LibFuzzer.html#dictionaries) of
     the tokens that are hard to find by mutation, such as format magic values.
   - `fuzz/seeds/<area>.txt`: directories of valid inputs to start from, one per line, relative to `fuzz`. They are
     only read, never written. Starting from valid inputs gets the fuzzer past the checks at the start of a parser.
4. Run it locally with `cargo make fuzz <target>` before opening the pull request.

A target should only report real bugs: if a failure can happen on valid use of an API, check for it in the target
instead of letting it crash. A target that reports false positives is quickly ignored.
