use crate::{build_aarch64, build_x64, clippy, coverage, cspell, deny, docs, format, test};
use colored::Colorize;
use std::error::Error;

type DynError = Box<dyn Error>;

pub(crate) fn all() -> Result<(), DynError> {
    println!("\n{}", "🚀 Running: all tasks".bright_green());

    deny()?;
    clippy()?;
    cspell()?;
    build_x64()?;
    build_aarch64()?;
    test()?;
    coverage()?;
    format()?;
    docs()?;

    Ok(())
}
