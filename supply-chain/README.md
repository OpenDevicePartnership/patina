# Working with cargo vet

## Introduction

`cargo vet` is a tool to help ensure that third-party Rust dependencies have been audited by a trusted entity.

It matches all dependencies against a set of audits conducted by the authors of the project or entities they trust.

To learn more, visit [mozilla/cargo-vet](https://github.com/mozilla/cargo-vet)

---

## Trusted Publishers Summary

We trust the following entities to audit crates. If new publishers are trusted in the future, they should be added to
this table to provide easy-to-view context on why they are trusted.

| **Publisher** | **GitHub Handle** | **Key Crates** | **Justification** | **Criteria** |
|---------------|-------------------|-----------------|-------------------|--------------|
| **Alex Crichton** | `alexcrichton` | `wasip2`, plus co-maintains `wasm-bindgen`, `js-sys`, `web-sys`, `libc`, `libm`, `cfg-if` | One of the original Rust developers. Maintains critical WebAssembly and systems infrastructure. | `safe-to-run` |
| **Andrew Gallant** | `BurntSushi` | `regex`, `regex-automata`, `regex-syntax`, `ucd-trie`, `winapi-util` | Well-known Rust developer, author of `ripgrep` and `regex`. Develops performance-focused crates that are fundamental infrastructure. | `safe-to-run` |
| **Ashley Mannix** | `KodrAus` | `bitflags`, `log`, `uuid`, `auto_impl` | Long-time Rust contributor, maintains foundational crates like `log` and `bitflags` that are widely used. | `safe-to-deploy` |
| **David Tolnay** | `dtolnay` | `syn`, `quote`, `proc-macro2`, `serde`, `thiserror`, `unicode-ident`, `serde_derive`, `serde_core`, `serde_json`, `thiserror-impl`, `unsafe-libyaml`, `serde_yaml` | Former Rust core team member, maintains critical infrastructure crates used by virtually every Rust project. | `safe-to-deploy` |
| **Ed Page** | `epage` | `clap`, `anstream`, `anstyle`, `anstyle-parse`, `anstyle-query`, `anstyle-wincon`, `clap_builder`, `clap_derive`, `clap_lex`, `colorchoice`, `is_terminal_polyfill`, `once_cell_polyfill`, `toml_datetime`, `toml_parser`, `winnow` | Very active in Rust CLI work, maintainer of the most popular CLI parsing library. | `safe-to-deploy` |
| **Jacob Pratt** | `jhpratt` | `time`, `time-core`, `time-macros`, `deranged` | Maintainer of the most popular time handling crate ecosystem in Rust. | `safe-to-deploy` |
| **Josh Stone** | `cuviper` | `autocfg`, `num-bigint` | Active Rust contributor, maintains important numeric ecosystem crates. | `safe-to-deploy` |
| **Joshua Liebow-Feeser** | `joshlf` | `zerocopy`, `zerocopy-derive` | Maintainer of zero-copy memory operation crates. Widely used and trusted. | `safe-to-deploy` |
| **m4b** | `m4b` | `goblin`, `scroll`, `scroll_derive` | Maintainer of popular and important system programming crates. | `safe-to-deploy` |
