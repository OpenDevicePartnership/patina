//! Build script for patina_dxe_core to verify toolchain configuration.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use std::{env, process};

// This must be kept in sync with .cargo/config.toml's PATINA_CONFIG_VERSION
const PATINA_CONFIG_VERSION: &str = "1";

fn main() {
    let version = env::var_os("PATINA_CONFIG_VERSION").unwrap_or_default();

    let rustflags = env::var_os("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
    eprintln!("CARGO_ENCODED_RUSTFLAGS={rustflags:?}");

    if version != PATINA_CONFIG_VERSION {
        eprintln!(
            "error: Incorrect PATINA_CONFIG_VERSION, expected version \"{}\", got version {:?}",
            PATINA_CONFIG_VERSION, version
        );
        eprintln!(
            "Use Patina's latest config.toml. See https://opendevicepartnership.github.io/patina/dev/toolchain_configuration.html"
        );
        process::exit(1);
    }

    // Only rerun this when the rustflags or the config version changes
    println!("cargo:rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");
    println!("cargo:rerun-if-env-changed=PATINA_CONFIG_VERSION");
}
