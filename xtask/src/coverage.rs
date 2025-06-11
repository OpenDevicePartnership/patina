use crate::util::{project_root, project_target_dir};
use colored::Colorize;
use std::{env, error::Error, process::Command};

type DynError = Box<dyn Error>;

pub(crate) fn coverage() -> Result<(), DynError> {
    println!("─────────────────────────────────");
    println!("{}", "🚀 Running: cargo coverage".bright_green());

    let target_dir = project_target_dir();
    let target_dir = target_dir.to_str().unwrap_or("./target");

    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(&cargo)
        .current_dir(project_root())
        .env("RUSTC_BOOTSTRAP", "1")
        .args([
            "tarpaulin",
            "--workspace",
            "--out",
            "html",
            "--out",
            "xml",
            "--exclude-files",
            "**/tests/*",
            "--exclude-files",
            "**/benches/*",
            "--exclude",
            "patina_test_macro",
            "--exclude",
            "xtask",
            "--output-dir",
            target_dir,
        ])
        .args(env::args().skip(2)) // Pass through any additional arguments
        .status()?;

    if !status.success() {
        Err("❌ Failed: cargo coverage")?;
    }

    println!("{}", "✔️    Done: cargo coverage".bright_green());

    Ok(())
}
