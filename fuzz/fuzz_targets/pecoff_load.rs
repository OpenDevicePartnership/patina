//! Fuzz target for PE/COFF image loading and relocation.
//!
//! This follows the order in which the Patina DXE Core processes an image that parsed successfully: it reads the HII
//! resource section from the source image, copies the headers and sections into a zeroed buffer of `SizeOfImage`
//! bytes, and then applies relocation fixups for the address the image was allocated at. The relocation step is
//! repeated for a second address, passing the blocks from the first, because that is how the DXE Core moves an
//! image again (for example when runtime images are relocated for `SetVirtualAddressMap()`).
//!
//! None of these steps may panic for any input. All of them index and slice attacker-controlled offsets and sizes,
//! so an out-of-bounds access shows up here as a panic.
//!
//! The loaded image is placed between guard bytes, and the target checks after every step that they are unchanged. It
//! also checks that `load_image` writes every byte of the image, by starting from a non-zero fill.
//!
//! `flatten_runtime_relocation_data` is intentionally not called: it allocates from the DXE Core's runtime allocator,
//! which only exists when the DXE Core is running.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![no_main]

use libfuzzer_sys::fuzz_target;
use patina_dxe_core::fuzz::pecoff::{self, UefiPeInfo};

/// The largest `SizeOfImage` the target will allocate a buffer for.
///
/// `SizeOfImage` comes from the image header and can be up to 4 GiB. The DXE Core allocates from the GCD, which
/// fails cleanly for a size that does not exist, but the fuzz target allocates from the process heap. Real DXE
/// images are far smaller than this limit.
const MAX_IMAGE_SIZE: usize = 16 * 1024 * 1024;

/// The names of the sections that DXE Core reads directly from the source image.
const SECTION_NAMES: [&str; 3] = [".text", ".rsrc", ".reloc"];

/// The number of guard bytes on each side of the image buffer, and their value.
const GUARD_LEN: usize = 0x1000;
const GUARD: u8 = 0xCC;

/// What the image buffer holds before it is loaded. It is not zero, because `load_image` is expected to write every
/// byte of the image and a zero fill would hide a byte that it skipped.
const IMAGE_FILL: u8 = 0xFF;

/// An image buffer with guard bytes before and after it. The loader is only given the image buffer, so the guards
/// must never change.
struct GuardedImage {
    arena: Vec<u8>,
}

impl GuardedImage {
    fn new(size: usize) -> Self {
        Self { arena: vec![GUARD; GUARD_LEN + size + GUARD_LEN] }
    }

    fn image(&mut self) -> &mut [u8] {
        let end = self.arena.len() - GUARD_LEN;
        &mut self.arena[GUARD_LEN..end]
    }

    fn assert_guards_intact(&self, step: &str) {
        let (before, rest) = self.arena.split_at(GUARD_LEN);
        let after = &rest[rest.len() - GUARD_LEN..];
        assert!(before.iter().all(|&byte| byte == GUARD), "{step} wrote before the start of the image buffer");
        assert!(after.iter().all(|&byte| byte == GUARD), "{step} wrote after the end of the image buffer");
    }
}

/// Derives a page-aligned address from the input so that a finding can be reproduced from the input file alone.
///
/// The addresses are chosen from the same range an image can be loaded at and from the extremes of the address
/// space, because the relocation adjustment is the difference between the load address and the image base.
fn load_address(data: &[u8], salt: u64) -> usize {
    // FNV-1a.
    let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ salt;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }

    let address = match hash % 8 {
        0 => 0,
        1 => 0xFFFF_FFFF_FFFF_F000,
        // Below 4 GiB, which covers images with 32-bit image bases.
        2 | 3 => hash & 0xFFFF_F000,
        _ => hash & 0x0000_7FFF_FFFF_F000,
    };
    address as usize
}

fuzz_target!(|data: &[u8]| {
    let Ok(pe_info) = UefiPeInfo::parse(data) else {
        return;
    };

    // These read from the source image, using offsets and sizes from the headers, and do not need a loaded image.
    let _ = pecoff::load_resource_section(&pe_info, data);
    for name in SECTION_NAMES {
        let _ = pecoff::get_section(name, &pe_info, data);
    }

    // The DXE Core rejects an image with a section outside of the image before it allocates memory for it.
    if pe_info.validate_sections().is_err() {
        return;
    }

    let size = pe_info.size_of_image as usize;
    if size == 0 || size > MAX_IMAGE_SIZE {
        return;
    }

    let mut arena = GuardedImage::new(size);
    arena.image().fill(IMAGE_FILL);
    let loaded = pecoff::load_image(&pe_info, data, arena.image());
    arena.assert_guards_intact("load_image");
    if loaded.is_err() {
        return;
    }

    // The loaded image must not depend on what the buffer held before: a byte that load_image does not write would
    // show up as a difference from a load into a buffer with a different fill.
    let mut zeroed = vec![0u8; size];
    pecoff::load_image(&pe_info, data, &mut zeroed).expect("load_image succeeded once, so it must succeed again");
    assert!(arena.image() == &zeroed[..], "load_image left bytes of the buffer that it did not write");
    drop(zeroed);

    let first = load_address(data, 1);
    let relocated = pecoff::relocate_image(&pe_info, first, arena.image(), &[]);
    arena.assert_guards_intact("relocate_image");
    let Ok(blocks) = relocated else {
        return;
    };

    let second = load_address(data, 2);
    let relocated = pecoff::relocate_image(&pe_info, second, arena.image(), &blocks);
    arena.assert_guards_intact("relocate_image (second address)");
    let Ok(blocks) = relocated else {
        return;
    };

    let _ = pecoff::relocate_image(&pe_info, first, arena.image(), &blocks);
    arena.assert_guards_intact("relocate_image (back to first address)");
});
