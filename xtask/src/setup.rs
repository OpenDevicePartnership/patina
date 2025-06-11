use crate::util::{cargo_bin_dir, project_root};
use colored::Colorize;
use std::{error::Error, path::Path, process::Command};

type DynError = Box<dyn Error>;

pub(crate) fn setup() -> Result<(), DynError> {
    println!("\n{}", "🚀 Running: Setup".bright_green());

    // Install platform specific pre-requisites
    install_platform_prerequisites()?;

    // Install rust x64 and aarch64 uefi toolchain targets
    install_rust_targets()?;

    // Install binstall
    install_binstall()?;

    // Install cargo tools
    install_cargo_tools()?;

    Ok(())
}

fn install_platform_prerequisites() -> Result<(), DynError> {
    if cfg!(target_os = "windows") {
        install_windows_prerequisites()
    } else if cfg!(target_os = "linux") {
        install_linux_prerequisites()
    } else {
        Err("Unsupported platform".into())
    }
}

fn install_windows_prerequisites() -> Result<(), DynError> {
    println!("Installing: Windows prerequisites");

    // Install Chocolatey
    let status = Command::new("winget").args(["install", "--id", "Chocolatey.Chocolatey", "-e"]).status()?;
    if !status.success() {
        return Err("Failed: winget install --id Chocolatey.Chocolatey -e".into());
    }

    // Install Python 3.12
    let status = Command::new("winget").args(["install", "--id", "Python.Python.3.12", "-e"]).status()?;
    if !status.success() {
        return Err("Failed: winget install --id Python.Python.3.12 -e".into());
    }

    // Install LLVM required for AArch64 build
    let status = Command::new("winget")
        .args(["install", "--id", "LLVM.LLVM", "-e", "--override", "\"/S /D=C:\\LLVM\""])
        .status()?;
    if !status.success() {
        return Err("Failed: winget install --id LLVM.LLVM -e --override \"/S /D=C:\\LLVM\"".into());
    }

    // choco install make
    let status = Command::new("choco").args(["install", "make"]).status()?;
    if !status.success() {
        return Err("Failed: choco install make".into());
    }

    // Install MSVC build tools
    let status = Command::new("winget").args(["install", "--id", "Microsoft.VisualStudio.2022.BuildTools", "-e", "--override", "\"--quiet --wait --norestart --add Microsoft.VisualStudio.Component.VC.CoreBuildTools --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64  --add Microsoft.VisualStudio.Component.Windows11SDK.22621 --add Microsoft.VisualStudio.Component.VC.Tools.ARM  --add Microsoft.VisualStudio.Component.VC.Tools.ARM64\""]).status()?;
    if !status.success() {
        return Err("Failed: Installing MSVC build tools".into());
    }

    // Install NodeJS
    let status = Command::new("winget").args(["install", "--id", "OpenJS.NodeJS.LTS", "-e"]).status()?;
    if !status.success() {
        return Err("Failed: winget install --id OpenJS.NodeJS.LTS -e".into());
    }

    // Install npm cspell
    let status = Command::new("npm.cmd").args(["install", "-g", "cspell@latest"]).status()?;
    if !status.success() {
        return Err("Failed: npm install -g cspell@latest".into());
    }

    // Install npm markdownlint-cli
    let status = Command::new("npm.cmd").args(["install", "-g", "markdownlint-cli@latest"]).status()?;
    if !status.success() {
        return Err("Failed: npm install -g markdownlint-cli@latest".into());
    }

    Ok(())
}

fn install_linux_prerequisites() -> Result<(), DynError> {
    println!("Installing: Linux prerequisites");

    // The following sudo apt dependencies must be installed manually, as Cargo
    // cannot run with sudo.
    //
    // sudo apt update && sudo apt install -y build-essential git nasm m4 bison
    // flex curl wget uuid-dev python3 python3-venv python-is-python3 unzip
    // acpica-tools gcc-multilib mono-complete pkg-config libssl-dev mtools
    // qemu-system clang llvm lld

    let status = Command::new("bash")
        .arg("-c")
        .arg(
            r#"
            set -e
            curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.7/install.sh | bash
            source "$HOME/.bashrc"
            nvm install --lts
            npm install -g cspell@latest
            npm install -g markdownlint-cli
        "#,
        )
        .status()
        .expect("failed to run setup");

    if !status.success() {
        return Err("Failed: nvm install ".into());
    }

    Ok(())
}

fn install_rust_targets() -> Result<(), DynError> {
    println!("Installing: Rust targets for UEFI");

    // Install x86_64-unknown-uefi target
    let status = Command::new("rustup").args(["target", "add", "x86_64-unknown-uefi"]).status()?;
    if !status.success() {
        return Err("Failed to install x86_64-unknown-uefi target".into());
    }

    // Install aarch64-unknown-uefi target
    let status = Command::new("rustup").args(["target", "add", "aarch64-unknown-uefi"]).status()?;
    if !status.success() {
        return Err("Failed to install aarch64-unknown-uefi target".into());
    }

    Ok(())
}

fn install_binstall() -> Result<(), DynError> {
    println!("Installing: binstall");
    let cargo_bin_dir = cargo_bin_dir();

    if cfg!(target_os = "windows") {
        install_binstall_on_windows(&cargo_bin_dir)
    } else if cfg!(target_os = "linux") {
        install_binstall_on_linux(&cargo_bin_dir)
    } else {
        Err("Unsupported platform".into())
    }
}

fn install_binstall_on_windows(cargo_bin_dir: &Path) -> Result<(), DynError> {
    let url = "https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-x86_64-pc-windows-msvc.zip";

    // Use powershell to download and extract
    let status = Command::new("powershell")
        .args([
            "-Command",
            &format!(
                r#"
Invoke-WebRequest '{}' -OutFile 'cargo-binstall.zip';
Expand-Archive 'cargo-binstall.zip' -DestinationPath '{}';
Move-Item -Force './cargo-binstall.exe' '{}';
Remove-Item 'cargo-binstall.zip'"#,
                url,
                cargo_bin_dir.display(),
                cargo_bin_dir.display(),
            ),
        ])
        .status()?;

    if !status.success() {
        return Err("Failed to run PowerShell command".into());
    }

    Ok(())
}

fn install_binstall_on_linux(cargo_bin_dir: &Path) -> Result<(), DynError> {
    let url = "https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-x86_64-unknown-linux-musl.tgz";

    let status = Command::new("bash")
        .arg("-c")
        .arg(format!(
            r#"
curl -L '{}' -o cargo-binstall.tgz &&
tar -xzf cargo-binstall.tgz &&
mv -f cargo-binstall '{}' &&
rm -f cargo-binstall.tgz
"#,
            url,
            cargo_bin_dir.display(),
        ))
        .status()?;

    if !status.success() {
        return Err("Failed to run curl/tar shell command".into());
    }

    Ok(())
}

fn install_cargo_tools() -> Result<(), DynError> {
    println!("Installing: cargo binstall tools");
    let tools = [
        ("cargo-deny", "^0.17"),
        ("cargo-nextest", "0.9.97"),
        ("cargo-release", "0.25.12"),
        ("cargo-tarpaulin", "0.31.5"),
        ("mdbook", "0.4.40"),
        ("mdbook-admonish", "1.18.0"),
        ("mdbook-mermaid", "0.14.0"),
    ];

    for tool in tools {
        println!("Installing(binstalling): {}@{}", tool.0, tool.1);
        let status = Command::new("cargo")
            .current_dir(project_root())
            .args(["binstall", tool.0, "-y", "--version", tool.1])
            .status()?;
        if !status.success() {
            // If binary installing(binstall) a tool failed, try installing from sources
            println!("Failed to binstall {}@{}, trying with cargo install", tool.0, tool.1);
            let status = Command::new("cargo")
                .current_dir(project_root())
                .args(["install", tool.0, "--version", tool.1])
                .status()?;
            if !status.success() {
                Err(format!("Failed: cargo install {}@{}", tool.0, tool.1))?;
            }
        }
    }

    Ok(())
}
