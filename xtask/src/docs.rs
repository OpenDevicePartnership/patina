use crate::util::project_root;
use colored::Colorize;
use std::{env, error::Error, process::Command};

type DynError = Box<dyn Error>;

pub(crate) fn docs() -> Result<(), DynError> {
    println!("─────────────────────────────────");
    println!("{}", "🚀 Running: cargo doc".bright_green());
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(&cargo)
        .current_dir(project_root())
        .env("RUSTC_BOOTSTRAP", "1")
        .env("RUSTDOCFLAGS", "-D warnings")
        .args(["doc", "--features", "doc", "--workspace", "--exclude", "xtask"])
        .args(env::args().skip(2)) // Pass through any additional arguments
        .status()?;

    if !status.success() {
        Err("❌ Failed: cargo doc failed")?;
    }

    println!("{}", "✔️    Done: cargo doc".bright_green());

    Ok(())
}
