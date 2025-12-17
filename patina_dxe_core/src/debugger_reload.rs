//! Module for adding a debugger reload command.
//!
//! This subsystem provides the ability to reload the DXE core image via the debugger.
//! This is intended for faster development of the Patina core without requiring
//! changing the image in flash.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;

use alloc::vec::Vec;
use mu_rust_helpers::uefi_decompress::{DecompressionAlgorithm, decompress_into_with_algo};
use patina::{
    component::service::memory::{AllocationOptions, MemoryManager, PageAllocationStrategy},
    guids,
    pi::hob::{self},
    uefi_size_to_pages,
};

use crate::{memory_manager::CoreMemoryManager, pecoff};

static HOB_LIST_ADDRESS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static TEAR_DOWN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[cfg(target_arch = "x86_64")]
// On x86_64 we need to allocate below 4GB because it uses statics for the GDT/IDT which can be shared with MP cores while
// they operate in protected mode.
const ARCH_ALLOCATION_STRATEGY: PageAllocationStrategy = PageAllocationStrategy::MaxAddress(0xFFFF_FFFF);
#[cfg(target_arch = "aarch64")]
const ARCH_ALLOCATION_STRATEGY: PageAllocationStrategy = PageAllocationStrategy::Any;

/// Initializes the debugger reload command subsystem.
pub fn initialize_debugger_reload(hob_list: *const c_void) {
    HOB_LIST_ADDRESS.store(hob_list as usize, core::sync::atomic::Ordering::Relaxed);
    patina_debugger::add_monitor_command("reload", "Used to reload the core", reload_monitor);
}

/// Tears down the debugger reload command subsystem.
pub fn tear_down_debugger_reload() {
    TEAR_DOWN.store(true, core::sync::atomic::Ordering::Relaxed);
}

/// The monitor command handler for the "reload" command. This is intended to be called by the
/// extension and so is not intended to be human friendly.
fn reload_monitor(args: &mut core::str::SplitWhitespace<'_>, out: &mut dyn core::fmt::Write) {
    if TEAR_DOWN.load(core::sync::atomic::Ordering::Relaxed) {
        let _ = writeln!(out, "Only supported at initial breakpoint");
        return;
    }

    match args.next() {
        Some("alloc_buffer") => {
            allocate_buffer_command(args, out);
        }
        Some("load") => {
            load_command(args, out);
        }
        _ => {
            let _ = writeln!(out, "Unknown command");
        }
    }
}

/// Implements the "reload alloc_buffer" command. This command allocates a buffer of the specified size and
/// returns the address of the buffer.
fn allocate_buffer_command(args: &mut core::str::SplitWhitespace<'_>, out: &mut dyn core::fmt::Write) {
    // get the requested length of the prep buffer.
    let buffer_size = match args.next() {
        Some(size_str) => match size_str.parse::<usize>() {
            Ok(size) => size,
            Err(_) => {
                let _ = writeln!(out, "Invalid buffer size");
                return;
            }
        },
        None => {
            let _ = writeln!(out, "Usage: reload alloc_buffer <size>");
            return;
        }
    };

    if buffer_size == 0 {
        let _ = writeln!(out, "Buffer size must be greater than 0");
        return;
    }

    // allocate a page-aligned buffer of the requested size.
    let pages = match CoreMemoryManager.allocate_pages(uefi_size_to_pages!(buffer_size), AllocationOptions::new()) {
        Ok(pages) => pages,
        Err(err) => {
            let _ = writeln!(out, "Failed to allocate reload buffer: {:?}", err);
            return;
        }
    };

    let address = pages.into_raw_ptr::<u8>().expect("Page allocation size check failed for u8") as usize;
    let _ = write!(out, "{:x}", address);
}

/// Implements the "reload load" command. This command loads the core image from the specified address and size.
fn load_command(args: &mut core::str::SplitWhitespace<'_>, out: &mut dyn core::fmt::Write) {
    // get the address prep buffer.
    let address = match args.next() {
        Some(addr_str) => match addr_str.parse::<usize>() {
            Ok(addr) => addr,
            Err(_) => {
                let _ = writeln!(out, "Invalid address");
                return;
            }
        },
        None => {
            let _ = writeln!(out, "No address provided");
            return;
        }
    };

    let size = match args.next() {
        Some(size_str) => match size_str.parse::<usize>() {
            Ok(size) => size,
            Err(_) => {
                let _ = writeln!(out, "Invalid size");
                return;
            }
        },
        None => {
            let _ = writeln!(out, "No size provided");
            return;
        }
    };

    if address == 0 || size == 0 {
        let _ = writeln!(out, "Address and size must be greater than 0");
        return;
    }

    // SAFETY: The debugger is responsible for ensuring the address and size are valid.
    let initial_image = unsafe { core::slice::from_raw_parts(address as *const u8, size) };
    let image = if let Some("compressed") = args.next() {
        match decompress_image(initial_image, out) {
            Some(image_slice) => image_slice,
            None => {
                return;
            }
        }
    } else {
        initial_image
    };

    core_reload(image, out);
}

/// Decompresses the provided image and returns a static slice to the decompressed image. Return None if decompression
/// fails.
fn decompress_image(compressed_image: &[u8], out: &mut dyn core::fmt::Write) -> Option<&'static [u8]> {
    if compressed_image.len() < 8 {
        let _ = writeln!(out, "Compressed image size too small");
        return None;
    }

    // Get the decompressed size. By spec it will be the second u32 in the compressed image.
    let decompressed_size = {
        u32::from_le_bytes([compressed_image[4], compressed_image[5], compressed_image[6], compressed_image[7]])
            as usize
    };

    let decompressed_image = alloc::boxed::Box::leak(alloc::vec![0u8; decompressed_size].into_boxed_slice());
    if let Err(error) =
        decompress_into_with_algo(compressed_image, decompressed_image, DecompressionAlgorithm::UefiDecompress)
    {
        let _ = writeln!(out, "Failed to decompress image: {:?}", error);
        return None;
    }

    Some(decompressed_image)
}

/// Reloads the core image from the provided image slice, and prints out the necessary information for the debugger to
/// start the new image.
fn core_reload(image: &[u8], out: &mut dyn core::fmt::Write) {
    let pe_info = match pecoff::UefiPeInfo::parse(image) {
        Ok(info) => info,
        Err(err) => {
            let _ = writeln!(out, "Failed to parse PE image: {:?}", err);
            return;
        }
    };

    // Step 1: allocate the image memory.
    let image_size = pe_info.size_of_image as usize;
    let alloc = match CoreMemoryManager.allocate_pages(
        uefi_size_to_pages!(image_size),
        AllocationOptions::new().with_strategy(ARCH_ALLOCATION_STRATEGY),
    ) {
        Ok(pages) => pages,
        Err(err) => {
            let _ = writeln!(out, "Failed to allocate load buffer: {:?}", err);
            return;
        }
    };

    // Step 2: load the image.
    let loaded_image = alloc.leak_as_slice::<u8>();
    if let Err(error) = pecoff::load_image(&pe_info, image, loaded_image) {
        let _ = writeln!(out, "Failed to load image. {:?}", error);
        return;
    }

    // Step 3: relocate the image.
    let loaded_image_addr = loaded_image.as_ptr() as usize;
    if let Err(error) = pecoff::relocate_image(&pe_info, loaded_image_addr, loaded_image, &Vec::new()) {
        let _ = writeln!(out, "Failed to relocate image. {:?}", error);
        return;
    }

    // Step 4: fixup the hob list.
    let entry_point = loaded_image_addr + pe_info.entry_point_offset;
    let (hob_list, stack_ptr) = match fixup_hob_list(loaded_image_addr, image_size, entry_point) {
        Ok(hob_list) => hob_list,
        Err(error) => {
            let _ = writeln!(out, "Failed to fixup HOB list: {}", error);
            return;
        }
    };

    // Step 5: Provide the debugger with the context to start the new image.
    let _ =
        write!(out, "success:{:x}\nip:{:x}\nsp:{:x}\narg0:{:x}\n", loaded_image_addr, entry_point, stack_ptr, hob_list);
}

/// Fixes up the HOB list to reflect the new core image. This involves updating the memory allocation hob for the DXE
/// core image and getting stack information.
fn fixup_hob_list(
    core_address: usize,
    core_buffer_size: usize,
    entry_point: usize,
) -> Result<(usize, usize), &'static str> {
    let physical_hob_list = HOB_LIST_ADDRESS.load(core::sync::atomic::Ordering::Relaxed);
    if physical_hob_list == 0 {
        return Err("Original HOB list address is zero");
    }

    let mut next_hob = physical_hob_list as *mut hob::header::Hob;
    let mut stack_ptr = 0usize;
    let mut fixed_up_core = false;
    loop {
        // SAFETY: The hob list should be valid as provided at the launch of the core. Otherwise this is an exceedingly
        //         bad idea, but this is a debugger operation so buckle up.
        let hob_header = unsafe { &*next_hob };
        if hob_header.r#type == hob::END_OF_HOB_LIST {
            break;
        }

        // Look for the DXE core memory allocation hob and modify it.
        if hob_header.r#type == hob::MEMORY_ALLOCATION {
            // SAFETY: The hob type has been verified, switching to the actual type.
            let alloc_hob = unsafe { (next_hob as *mut hob::MemoryAllocationModule).as_mut() }
                .ok_or("Failed to read memory allocation HOB")?;

            if alloc_hob.module_name == guids::DXE_CORE {
                alloc_hob.alloc_descriptor.memory_base_address = core_address as u64;
                alloc_hob.alloc_descriptor.memory_length = core_buffer_size as u64;
                alloc_hob.entry_point = entry_point as u64;
                fixed_up_core = true;
            } else if alloc_hob.alloc_descriptor.name == guids::HOB_MEMORY_ALLOC_STACK {
                // Get the top of the stack. The pointer used needs to be offset down 0x18 bytes for
                // alignment to ensure we match calling convention requirements. Failure to do this will cause
                // crashes due to mis-aligned stack accesses.
                stack_ptr = (alloc_hob.alloc_descriptor.memory_base_address + alloc_hob.alloc_descriptor.memory_length)
                    as usize
                    - 0x18;
            }
        }

        let hob_size = hob_header.length as usize;
        next_hob = unsafe { next_hob.byte_add(hob_size) };
    }
    if !fixed_up_core {
        return Err("Failed to find DXE core memory allocation HOB");
    }

    if stack_ptr == 0 {
        return Err("Failed to find stack memory allocation HOB");
    }

    Ok((physical_hob_list, stack_ptr))
}
