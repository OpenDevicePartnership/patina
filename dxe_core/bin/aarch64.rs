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
use alloc::boxed::Box;
use core::{ffi::c_void, panic::PanicInfo};
use dxe_core::Core;
use sample_components::HelloComponent;

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

type DxeCore = Core<uefi_cpu_init::AArch64CpuInitializer, section_extractor::CompositeSectionExtractor>;

static ADV_LOGGER: AdvancedLoggerComponent<serial_writer::UartPl011> = AdvancedLoggerComponent::new(&LOGGER);

#[cfg_attr(target_os = "uefi", export_name = "efi_main")]
pub extern "efiapi" fn _start(physical_hob_list: *const c_void) -> ! {
    log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Trace)).unwrap();
    let adv_logger_component = AdvancedLoggerComponent::<serial_writer::UartPl011>::new(&LOGGER);
    adv_logger_component.init_advanced_logger(physical_hob_list).unwrap();

    Core::default()
        .with_cpu_initializer(uefi_cpu_init::NullCpuInitializer::default())
        .with_section_extractor(section_extractor::CompositeSectionExtractor::default())
        // Add any config knob functions for pre-gcd-init Core
        // .with_some_config(true)
        .initialize(physical_hob_list) // We can make allocations now!
        // Add any config knob functions for post-gcd-init Core
        // .with_some_config(true)
        .with_driver(Box::new(adv_logger_component))
        .with_driver(Box::new(HelloComponent::default()))
        .with_driver(Box::new(HelloComponent::default().with_name("Dxe Core")))
        .with_driver(Box::new(HelloComponent::default().with_name("World")))
        .start()
        .unwrap();

    log::info!("Dead Loop Time");
    loop {}
}
