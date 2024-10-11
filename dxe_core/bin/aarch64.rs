//! DXE Core Sample AARCH64 Binary
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg(all(target_os = "uefi", feature = "aarch64"))]
#![no_std]
#![no_main]

extern crate alloc;

use adv_logger::{component::AdvancedLoggerComponent, logger::AdvancedLogger};
use core::{ffi::c_void, panic::PanicInfo};
use dxe_core::Core;
use sample_components::HelloWorldComponent;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("{}", info);
    loop {}
}

static LOGGER: AdvancedLogger<serial_writer::UartPl011> = AdvancedLogger::new(
    uefi_logger::Format::Standard,
    &[
        ("goblin", log::LevelFilter::Off),
        ("uefi_depex_lib", log::LevelFilter::Off),
        ("gcd_measure", log::LevelFilter::Off),
    ],
    log::LevelFilter::Trace,
    serial_writer::UartPl011::new(0x6000_0000),
);

type DxeCore = Core<uefi_cpu_init::NullCpuInitializer, section_extractor::CompositeSectionExtractor>;

#[cfg_attr(target_os = "uefi", export_name = "efi_main")]
pub extern "efiapi" fn _start(physical_hob_list: *const c_void) -> ! {
    log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Trace)).unwrap();

    let hello_world_component = HelloWorldComponent::default();
    let adv_logger_component = AdvancedLoggerComponent::new(&LOGGER);
    adv_logger.init_advanced_logger(physical_hob_list).unwrap();

    let mut dxe_core = DxeCore { ..Default::default() };
    dxe_core.start(physical_hob_list, &[&hello_world_component, &adv_logger_component]).unwrap();
    log::info!("Dead Loop Time");
    loop {}
}
