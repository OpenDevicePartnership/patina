use crate::util::project_root;
use colored::Colorize;
use std::{env, error::Error, process::Command};

type DynError = Box<dyn Error>;

pub(crate) fn clippy() -> Result<(), DynError> {
    println!("─────────────────────────────────");
    println!("{}", "🚀 Running: cargo clippy".bright_green());
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(&cargo)
        .current_dir(project_root())
        .env("RUSTC_BOOTSTRAP", "1")
        .args(["clippy", "--all-targets", "--all-features", "--", "-D", "warnings"])
        .args(env::args().skip(2)) // Pass through any additional arguments
        .status()?;

    if !status.success() {
        Err("❌ Failed: cargo clippy")?;
    }

    println!("{}", "✔️    Done: cargo clippy".bright_green());

    Ok(())
}
