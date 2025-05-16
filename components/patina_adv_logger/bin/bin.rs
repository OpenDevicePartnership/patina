//! Executable for parsing advanced logger buffers.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use clap::Parser;
use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Parser, Debug)]
struct Args {
    /// Path for the input file containing the raw advanced logger buffer.
    input_path: PathBuf,
    /// Optional path for the output file. If not specified, the output will be printed to stdout.
    #[arg(short, long)]
    output_path: Option<PathBuf>,
    /// Flag to include entry metadata (level, phase, timestamp) in the output.
    #[arg(short, long, default_value_t = false)]
    entry_metadata: bool,
    /// Flag to include the header in the output.
    #[arg(long, default_value_t = false)]
    header: bool,
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    // Open the input file
    let mut file = File::open(Path::new(&args.input_path))?;

    // Read the file contents into a buffer
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let parser = patina_adv_logger::parser::Parser::new(&buffer).with_entry_metadata(args.entry_metadata);
    // Write to standard if no output file is specified.
    match args.output_path {
        Some(path) => {
            let mut out = File::create(path)?;
            parse_log(args.header, &parser, &mut out)?;
        }
        None => parse_log(args.header, &parser, &mut io::stdout())?,
    };

    Ok(())
}

fn parse_log<W: std::io::Write>(
    header: bool,
    parser: &patina_adv_logger::parser::Parser,
    out: &mut W,
) -> io::Result<()> {
    if header {
        parser.write_header(out).map_err(|e| {
            eprintln!("Error writing log: {}", e);
            io::Error::new(io::ErrorKind::Other, e)
        })?;
    }

    parser.write_log(out).map_err(|e| {
        eprintln!("Error writing log: {}", e);
        io::Error::new(io::ErrorKind::Other, e)
    })?;

    Ok(())
}
