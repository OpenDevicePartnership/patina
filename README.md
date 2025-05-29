<!-- markdownlint-disable MD013 --> <!-- MD013: Line length -->
<!-- markdownlint-disable MD033 --> <!-- MD033: Inline HTML -->

# Patina

This repository hosts the Patina project - a Rust implementation of UEFI firmware.

The goal of this project is to serve as a replacement for core UEFI firmware components so they are written in Pure
Rust as opposed to Rust wrappers around core implementation still written in C.

## Background

There have been various [instances of advocacy](https://msrc-blog.microsoft.com/2019/11/07/using-rust-in-windows/) for
building system level software in [Rust](https://www.rust-lang.org/).

This repository contains a Rust [UEFI](https://uefi.org/) firmware implementation called Patina. We plan to enable an
incremental migration of today's firmware components largely written in C to Rust starting with the core. The primary
objective for this effort is to improve the security and stability of system firmware by leveraging the memory safety
offered by Rust while retaining similar boot performance.

## First-Time Patina Setup Instructions for QEMU

This section will guide you through setting up `Patina` for QEMU across different platforms and architectures.

### Supported Platforms and Architectures

| Host Platform | Target Architectures Supported |
| ------------- | ------------------------------ |
| Windows       | x64, AArch64                   |
| WSL           | x64, AArch64                   |
| Linux         | x64, AArch64                   |

### Prerequisites

One-time tools and packages required to set up Patina development.

<details>
<summary><b> 🪟 Windows 11 - 24H2 </b></summary>

| Tool                                                                                                                                  | Install Command                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| ------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [Chocolatey](https://chocolatey.org/)                                                                                                 | `winget install --id Chocolatey.Chocolatey -e`                                                                                                                                                                                                                                                                                                                                                                                                      |
| [Python 3](https://www.python.org/)                                                                                                   | `winget install Python3` <br> **Note:** Disable any app execution alias defined for `python.exe` and `python3.exe` from Windows settings(Apps > Advanced app settings > App execution alias)                                                                                                                                                                                                                                                        |
| [Git](https://git-scm.com/)                                                                                                           | `winget install --id Git.Git -e`                                                                                                                                                                                                                                                                                                                                                                                                                    |
| [Rust](https://rustup.rs/)                                                                                                            | `winget install --id Rustlang.Rustup -e` <ol><li> **Add x86_64 uefi target:** `rustup target add x86_64-unknown-uefi` </li><li> **Add aarch64 uefi target:** `rustup target add aarch64-unknown-uefi`</li><li>**Install cargo make:** `cargo install cargo-make`</li><li>**Install cargo tarpaulin:** `cargo install cargo-tarpaulin`</li></ol>                                                                                                     |
| [LLVM](https://llvm.org/)                                                                                                             | `winget install --id LLVM.LLVM -e --override "/S /D=C:\LLVM"` <ul><li>**Note:** `/D=C:\LLVM` override(with no spaces) is needed for AArch64 build of `patina-qemu` repo on Windows.</li></ul>                                                                                                                                                                                                                                                       |
| [GNU Make](https://community.chocolatey.org/packages/make)                                                                            | `choco  install make` <ul><li>**Note:** Needed for AArch64 build of `patina-qemu` repo on Windows.</li></ul>                                                                                                                                                                                                                                                                                                                                        |
| [MSVC BuildTools](https://rust-lang.github.io/rustup/installation/windows-msvc.html#installing-only-the-required-components-optional) | `winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --norestart --add Microsoft.VisualStudio.Component.VC.CoreBuildTools --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64  --add Microsoft.VisualStudio.Component.Windows11SDK.22621 --add Microsoft.VisualStudio.Component.VC.Tools.ARM  --add Microsoft.VisualStudio.Component.VC.Tools.ARM64"` <br> **Note:** Only required when building for `std` |
| [Node](https://nodejs.org/en)                                                                                                         | `winget install --id OpenJS.NodeJS.LTS -e` <ol><li> **Add cspell:** `npm install -g cspell@latest` </li><li> **Add markdown lint cli:** `npm install -g markdownlint-cli` </li></ol>                                                                                                                                                                                                                                                                |
| [QEMU](https://www.qemu.org/)                                                                                                         | `winget install --id SoftwareFreedomConservancy.QEMU -e`                                                                                                                                                                                                                                                                                                                                                                                            |
| [WinDBG](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/)                                                        | `winget install --id Microsoft.WinDbg -e`                                                                                                                                                                                                                                                                                                                                                                                                           |
| [VSCode](https://code.visualstudio.com/)                                                                                              | `winget install --id Microsoft.VisualStudioCode -e`                                                                                                                                                                                                                                                                                                                                                                                                 |
| [Rust Analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)                                          | `code --install-extension rust-lang.rust-analyzer`                                                                                                                                                                                                                                                                                                                                                                                                  |

**Note:** Add the LLVM bin directory (`C:\LLVM\bin`) and the QEMU bin directory
(`C:\Program Files\qemu`) to the `PATH` environment variable.
</details>

<details>
<summary><b> 🐧 Linux/WSL - Ubuntu 24.04 LTS - Bash </b></summary>

| Tool                                                                                            | Install Command                                                                                                                                                                                                                                                                                                                                                                                                          |
| ----------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Build Essentials                                                                                | `sudo apt update && sudo apt install -y build-essential git nasm m4 bison flex curl wget uuid-dev python3 python3-venv python-is-python3 unzip acpica-tools gcc-multilib mono-complete pkg-config libssl-dev mtools`                                                                                                                                                                                                     |
| [Rust](https://rustup.rs/)                                                                      | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` <br>**Note:** Might have to reopen the terminal <ol><li> **Add x86_64 uefi target:** `rustup target add x86_64-unknown-uefi` </li><li> **Add aarch64 uefi target:** `rustup target add aarch64-unknown-uefi`</li><li>**Install cargo make:** `cargo install cargo-make`</li><li>**Install cargo tarpaulin:** `cargo install cargo-tarpaulin`</li></ol> |
| [Node](https://nodejs.org/en)                                                                   | `curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.7/install.sh \| bash` <br>`source ~/.bashrc`<br>`nvm install --lts` <ol><li> **Add cspell:** `npm install -g cspell@latest` </li><li> **Add markdown lint cli:** `npm install -g markdownlint-cli` </li></ol>                                                                                                                                               |
| [QEMU](https://www.qemu.org/)                                                                   | `sudo apt install -y qemu-system`                                                                                                                                                                                                                                                                                                                                                                                        |
| [LLVM](https://llvm.org/)                                                                       | `sudo apt install -y clang llvm lld`                                                                                                                                                                                                                                                                                                                                                                                     |
| [VSCode](https://code.visualstudio.com/docs/setup/linux#_debian-and-ubuntu-based-distributions) | `wget https://go.microsoft.com/fwlink/?LinkID=760868 -O code.deb` <br> `sudo apt install ./code.deb`                                                                                                                                                                                                                                                                                                                     |
| [Rust Analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)    | `code --install-extension rust-lang.rust-analyzer`                                                                                                                                                                                                                                                                                                                                                                       |

</details>

### Code

| Repo                                                                                   | Clone                                                                     | About                                                                            |
| -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| [patina](https://github.com/OpenDevicePartnership/patina/)                             | `git clone https://github.com/OpenDevicePartnership/patina`               | Patina Firmware. Contains all crates published to [crates.io](https://crates.io) |
| [patina-qemu](https://github.com/OpenDevicePartnership/patina-qemu/)                   | `git clone https://github.com/OpenDevicePartnership/patina-qemu`          | Repository to produce Patina firmware image for QEMU                             |
| [patina-dxe-core-qemu](https://github.com/OpenDevicePartnership/patina-dxe-core-qemu/) | `git clone https://github.com/OpenDevicePartnership/patina-dxe-core-qemu` | Repository to produce Patina DXE Core Binary for QEMU                            |

**Note:** Prefer short paths on Windows(`C:\r\`) or Linux(`/home/<username>/r/`)

### Build and Run

<details>
<summary><b> 🖥️ X64 Target </b></summary>

| Repo                                                                                   | Build Instructions                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| [patina-dxe-core-qemu](https://github.com/OpenDevicePartnership/patina-dxe-core-qemu/) | `cd <patina-dxe-core-qemu>` <br><br> **Build dxe core efi binary:** <br>`cargo make q35`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| [patina-qemu](https://github.com/OpenDevicePartnership/patina-qemu/)                   | `cd <patina-qemu>` <br><br> **Setup and Activate Virtual Env:** <br> `python -m venv q35env` <br> 🪟 `q35env\Scripts\activate.bat` <br> 🐧 `source q35env/bin/activate` <br><br> **Build Perquisites:** <br>`pip install --upgrade -r pip-requirements.txt` <br><br> **Stuart Setup:** <br>`stuart_setup  -c Platforms/QemuQ35Pkg/PlatformBuild.py` <br> <br>**Stuart Update:** <br>`stuart_update -c Platforms/QemuQ35Pkg/PlatformBuild.py` <br>**Note:** Retry the command if failed with `Filename too long` error <br><br> **Stuart Build and Launch Uefi Shell:** <br>🪟 `stuart_build  -c Platforms/QemuQ35Pkg/PlatformBuild.py --flashrom BLD_*_DXE_CORE_BINARY_PATH="C:\r\patina-dxe-core-qemu\target\x86_64-unknown-uefi"` <br>🐧 `stuart_build  -c Platforms/QemuQ35Pkg/PlatformBuild.py TOOL_CHAIN_TAG=CLANGPDB --flashrom BLD_*_DXE_CORE_BINARY_PATH="$HOME/r/patina-dxe-core-qemu/target/x86_64-unknown-uefi"` <br><br> **Stuart Build and Launch OS(Optional):** <br>🪟 `stuart_build  -c Platforms/QemuQ35Pkg/PlatformBuild.py --flashrom BLD_*_DXE_CORE_BINARY_PATH="C:\r\patina-dxe-core-qemu\target\x86_64-unknown-uefi" PATH_TO_OS="C:\OS\Windows11.qcow2"` <br>🐧 `stuart_build  -c Platforms/QemuQ35Pkg/PlatformBuild.py TOOL_CHAIN_TAG=CLANGPDB --flashrom BLD_*_DXE_CORE_BINARY_PATH="$HOME/r/patina-dxe-core-qemu/target/x86_64-unknown-uefi" PATH_TO_OS="$HOME/OS/Windows11.qcow2"` |
| [patina](https://github.com/OpenDevicePartnership/patina/)                             | No need to build this(except for local development). Crates from this repo are consumed directly from [crates.io](https://crates.io) by `patina-dxe-core-qemu` repo                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |

</details>

<details>
<summary><b> 📱 AArch64 Target </b></summary>

| Repo                                                                                   | Build Instructions                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| [patina-dxe-core-qemu](https://github.com/OpenDevicePartnership/patina-dxe-core-qemu/) | `cd <patina-dxe-core-qemu>` <br><br> **Build dxe core efi binary:** <br>`cargo make sbsa`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| [patina-qemu](https://github.com/OpenDevicePartnership/patina-qemu/)                   | `cd <patina-qemu>` <br><br> **Setup and Activate Virtual Env:** <br> `python -m venv sbsaenv` <br> 🪟 `sbsaenv\Scripts\activate.bat` <br> 🐧 `source sbsaenv/bin/activate` <br><br> **Build Perquisites:** <br>`pip install --upgrade -r pip-requirements.txt` <br><br> **Stuart Setup:** <br>`stuart_setup  -c Platforms/QemuSbsaPkg/PlatformBuild.py` <br> <br>**Stuart Update:** <br>`stuart_update -c Platforms/QemuSbsaPkg/PlatformBuild.py` <br><br> **Stuart Build and Launch Uefi Shell:** <br>🪟 `stuart_build  -c Platforms/QemuSbsaPkg/PlatformBuild.py TOOL_CHAIN_TAG=CLANGPDB --flashrom BLD_*_DXE_CORE_BINARY_PATH="C:\r\patina-dxe-core-qemu\target\aarch64-unknown-uefi"` <br>🐧 `stuart_build  -c Platforms/QemuSbsaPkg/PlatformBuild.py TOOL_CHAIN_TAG=CLANGPDB --flashrom BLD_*_DXE_CORE_BINARY_PATH="$HOME/r/patina-dxe-core-qemu/target/aarch64-unknown-uefi"` <br><br> **Stuart Build and Launch OS(Optional):** <br>🪟 `stuart_build  -c Platforms/QemuSbsaPkg/PlatformBuild.py TOOL_CHAIN_TAG=CLANGPDB --flashrom BLD_*_DXE_CORE_BINARY_PATH="C:\r\patina-dxe-core-qemu\target\aarch64-unknown-uefi" PATH_TO_OS="C:\OS\Windows11.qcow2"` <br>🐧 `stuart_build  -c Platforms/QemuSbsaPkg/PlatformBuild.py TOOL_CHAIN_TAG=CLANGPDB --flashrom BLD_*_DXE_CORE_BINARY_PATH="$HOME/r/patina-dxe-core-qemu/target/aarch64-unknown-uefi" PATH_TO_OS="$HOME/OS/Windows11.qcow2"` |
| [patina](https://github.com/OpenDevicePartnership/patina/)                             | No need to build this(except for local development). Crates from this repo are consumed directly from [crates.io](https://crates.io) by `patina-dxe-core-qemu` repo                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |

</details>

### Local Development

The above steps will help you build and test the vanilla code, with dependencies
fetched from crates.io. For local development, you should modify the relevant
crates within the patina repository and update the dependencies using
appropriate path patches.

### Debugging

- [WinDbg + QEMU + Patina UEFI - Debugging Guide](https://github.com/OpenDevicePartnership/patina-qemu/blob/main/Platforms/Docs/Common/windbg-qemu-uefi-debugging.md)
- [WinDbg + QEMU + Patina UEFI + Windows OS - Debugging Guide](https://github.com/OpenDevicePartnership/patina-qemu/blob/main/Platforms/Docs/Common/windbg-qemu-windows-debugging.md)

## Miscellaneous

### Build

All of the patina crates can be compiled in one of 3 supported targets; aarch64, x64, or native.

```shell
cargo make build-aarch64
   - or -
cargo make build-x64
   - or -
cargo make build
```

By default, the make compiles a developer build, but development or release can be indicated by using the "-p" flag

```shell
cargo make -p development build-aarch64
   - or -
cargo make -p release build-aarch64
```

### Test

Use the test command to invoke a test build and execute all unit tests.

```shell
cargo make test
```

### Coverage

The coverage command will generate test coverage data for all crates in the project.  To target a single crate, the
name can be added to the command line.

```shell
cargo make coverage
   - or -
cargo make coverage dxe_core
```

### Documentation

We have "Getting Started" documentation located in this repository at `docs/*`. The latest documentation can be found at
<https://OpenDevicePartnership.github.io/patina/>, however this documentation can also be self-hosted via
([mdbook](https://github.com/rust-lang/mdBook)). Once all dependencies are installed, you can run `mdbook serve docs` to
self host the getting started book.

You can also generate API documentation for the project using `cargo make doc`. This will eventually be hosted on
docs.rs once we begin uploading to crates.io. You can have the documentation opened in your browser by running
`cargo make doc-open`.

## Performing a Release

Below is the information required to perform a release that publishes to the registry feed:

1. Review the current draft release on the github repo: [Releases](https://github.com/OpenDevicePartnership/patina/releases)
   1. If something is incorrect, update it in the draft release
   2. If you need to manually change the version, make sure you update the associated git tag value in the draft release
2. Publish the release
3. Monitor the publish release workflow that is automatically triggered on the release being published:
   [Publish Release Workflow](https://github.com/OpenDevicePartnership/patina/actions/workflows/publish-release.yml)
4. Once completed successfully, click on the  "Notify Branch Creation Step" and click the provided link to create the
   PR to update all versions in all Cargo.toml files across the repository.

## Contributing

- Review Rust Documentation in the [/docs](https://github.com/OpenDevicePartnership/patina/blob/HEAD/docs/src/introduction.md)
directory.
- Run unit tests and ensure all pass.

Before making pull requests at a minimum, run:

```shell
cargo make all
```

## Important Notes

This repository is still considered to be in a "beta" stage at this time. Platform testing and integration feedback
is very welcome.
