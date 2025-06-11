use crate::util::project_root;
use colored::Colorize;
use std::{env, error::Error, process::Command};

type DynError = Box<dyn Error>;

pub(crate) fn check() -> Result<(), DynError> {
    println!("─────────────────────────────────");
    println!("{}", "🚀 Running: cargo check".bright_green());
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let arch = "x86_64"; //std::env::consts::ARCH;
    let target = match arch {
        "x86_64" => "x86_64-unknown-uefi",
        "aarch64" => "aarch64-unknown-uefi",
        _ => panic!("Unsupported architecture: {}", arch),
    };

    // cargo check for no std environment
    let status = Command::new(&cargo)
        .current_dir(project_root())
        .env("RUSTC_BOOTSTRAP", "1")
        .args([
            "check",
            "--target",
            target,
            "-Zbuild-std=core,compiler_builtins,alloc",
            "-Zbuild-std-features=compiler-builtins-mem",
            "-Zunstable-options",
            "--timings=html",
            "--workspace",
            "--exclude",
            "xtask",
        ])
        .args(env::args().skip(2)) // Pass through any additional arguments
        .status()?;

    if !status.success() {
        Err("❌ Failed: cargo check")?;
    }

    // cargo check for std environment
    let status = Command::new(&cargo)
        .current_dir(project_root())
        .env("RUSTC_BOOTSTRAP", "1")
        .args(["check", "--features", "std"])
        .args(env::args().skip(2)) // Pass through any additional arguments
        .status()?;

    if !status.success() {
        Err("❌ Failed: cargo check")?;
    }

    // cargo check for xtask std environment
    let status = Command::new(&cargo)
        .current_dir(project_root())
        .env("RUSTC_BOOTSTRAP", "1")
        .args(["check", "-p", "xtask"])
        .args(env::args().skip(2)) // Pass through any additional arguments
        .status()?;

    if !status.success() {
        Err("❌ Failed: cargo check")?;
    }

    println!("{}", "✔️    Done: cargo check".bright_green());

    Ok(())
}
