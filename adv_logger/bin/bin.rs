//! Executable for parsing advanced logger buffers.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use std::env;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use adv_logger::parser::Parser;

fn main() -> io::Result<()> {
    // Collect the command line arguments
    let args: Vec<String> = env::args().collect();

    // Check if the correct number of arguments are provided
    if args.len() != 2 {
        eprintln!("Usage: {} <input binary file path>", args[0]);
        std::process::exit(1);
    }

    // Get the file path from the arguments
    let input_path = &args[1];

    // Open the input file
    let mut file = File::open(Path::new(input_path))?;

    // Read the file contents into a buffer
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    // Write to standard if no output file is specified.
    let mut out: io::Stdout = io::stdout();

    let parser = Parser::new(&buffer);

    parser.write_header(&mut out).map_err(|e| {
        eprintln!("Error writing log: {}", e);
        io::Error::new(io::ErrorKind::Other, e)
    })?;

    parser.write_log(&mut out).map_err(|e| {
        eprintln!("Error writing log: {}", e);
        io::Error::new(io::ErrorKind::Other, e)
    })?;

    Ok(())
}
