use crate::util::project_root;
use colored::Colorize;
use std::{env, error::Error, process::Command};

type DynError = Box<dyn Error>;

pub(crate) fn cspell() -> Result<(), DynError> {
    println!("─────────────────────────────────");
    println!("{}", "🚀 Running: cspell".bright_green());
    let cmd = if cfg!(target_os = "windows") { "cspell.cmd" } else { "cspell" };
    let status = Command::new(cmd)
        .current_dir(project_root())
        .args([
            "--quiet",
            "--no-progress",
            "--no-summary",
            "--dot",
            "--gitignore",
            "-e",
            "{.git/**,.github/**,.vscode/**}",
            ".",
        ])
        .args(env::args().skip(2)) // Pass through any additional arguments
        .status()?;

    if !status.success() {
        Err("❌ Failed: cspell")?;
    }

    println!("{}", "✔️    Done: cspell".bright_green());

    Ok(())
}
