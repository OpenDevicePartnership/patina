use crate::util::project_root;
use colored::Colorize;
use std::{env, error::Error, process::Command};

type DynError = Box<dyn Error>;

pub(crate) fn build_aarch64() -> Result<(), DynError> {
    println!("─────────────────────────────────");
    println!("{}", "🚀 Running: aarch64 - cargo build".bright_green());
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(&cargo)
        .current_dir(project_root())
        .env("RUSTC_BOOTSTRAP", "1")
        .args([
            "build",
            "--target",
            "aarch64-unknown-uefi",
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
        Err("❌ Failed: AArch64 cargo build")?;
    }

    println!("{}", "✔️    Done: AArch64 cargo build".bright_green());

    Ok(())
}
