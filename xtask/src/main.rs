mod all;
mod build_aarch64;
mod build_x64;
mod check;
mod clippy;
mod coverage;
mod cspell;
mod deny;
mod docs;
mod format;
mod help;
mod setup;
mod test;
mod util;

use all::all;
use build_aarch64::build_aarch64;
use build_x64::build_x64;
use check::check;
use clippy::clippy;
use colored::Colorize;
use coverage::coverage;
use cspell::cspell;
use deny::deny;
use docs::docs;
use format::format;
use help::print_help;
use setup::setup;
use std::{env, error::Error};
use test::test;

type DynError = Box<dyn Error>;

fn main() {
    if let Err(e) = try_main() {
        eprintln!("{}", e.to_string().bright_red());
        std::process::exit(-1);
    }
}

fn try_main() -> Result<(), DynError> {
    let task = env::args().nth(1);
    match task.as_deref() {
        Some("all") => all()?,
        Some("build-aarch64") => build_aarch64()?,
        Some("build-x64") => build_x64()?,
        Some("check") => check()?,
        Some("clippy") => clippy()?,
        Some("coverage") => coverage()?,
        Some("cspell") => cspell()?,
        Some("deny") => deny()?,
        Some("docs") => docs()?,
        Some("fmt") => format()?,
        Some("help") => print_help(),
        Some("test") => test()?,
        Some("setup") => setup()?,
        _ => print_help(),
    }
    Ok(())
}
