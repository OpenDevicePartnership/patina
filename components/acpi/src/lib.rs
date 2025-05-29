#![no_std]
extern crate alloc;

pub mod component;
pub mod config;
pub mod error;
pub mod service;

mod acpi;
mod acpi_protocol;
mod acpi_table;
mod integration_test;
mod signature;
