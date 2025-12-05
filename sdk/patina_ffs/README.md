# Patina FIrmware File System (FFS) Crate

Rust support for the UEFI Platform Initialization (PI) Specification defined Firmware File System (FFS). Offers
zero-copy inspection helpers for firmware volumes, files, and sections together with serialization to spec-compliant
byte streams. It targets firmware environments that run without the Rust standard library.

## Key Capabilities

- Parse firmware volumes, firmware files, and sections directly from immutable byte slices using `VolumeRef`,
  `FileRef`, and `Section`.
- Compose new firmware content with builders (`Volume`, `File`, `Section`) that enforce header layout, checksum
  calculation, erase polarity, and inter-section padding.
- Provide trait-based extension points (`SectionExtractor`, `SectionComposer`) so platforms can plug in custom
  decompression or guided-section handling.
- Return consistent errors through `FirmwareFileSystemError`, which maps to Patina’s `EfiError` and `efi::Status`.

## Core Data Types

- `VolumeRef<'a>` – A view over a firmware volume that validates headers, block maps, and extended headers, and exposes
  iterators over contained files. It also supports creation from a physical address (`unsafe fn new_from_address`).
- `Volume` – A firmware volume builder that accepts a block map and a collection of files and emits byte streams.
- `FileRef<'a>` and `File` – Read and compose firmware files, including large-file headers, checksum enforcement, and
  section traversal.
- `Section` and `SectionHeader` – Represent leaf and encapsulation sections, track dirty state, and serialize with the
  correct header variant (standard or extended). `SectionIterator` walks a serialized section list with 4-byte
  alignment handling.
- `SectionExtractor` – Trait that defines interfaces to expand encapsulated sections (for example Brotli or UEFI
  Compress), with implementations available in the `patina_ffs_extractors` companion crate.
- `SectionComposer` – Trait for turning higher-level inputs into serialized section payloads before they are inserted
  into a `File`.

## Example: Scanning FIrmware Volumes (FVs)

```rust
use core::ffi::c_void;
use patina_dxe_core::Core;
use patina_ffs::volume::VolumeRef;
use patina_ffs_extractors::BrotliSectionExtractor;

# fn get_fv_bytes() -> &'static [u8] { unimplemented!() }

fn inspect_firmware(fv_bytes: &'static [u8]) {
    let fv = VolumeRef::new(fv_bytes).expect("valid firmware volume");
    for file in fv.files() {
        let file = file.expect("valid file");
        for section in file.sections_with_extractor(&BrotliSectionExtractor::default()).unwrap() {
            // Analyze payload, locate PE32 images, or feed data to the dispatcher.
        }
    }
}

#[cfg_attr(target_os = "uefi", export_name = "efi_main")]
pub extern "efiapi" fn _start(physical_hob_list: *const c_void) -> ! {
    Core::default()
        .init_memory(physical_hob_list)
        .with_service(BrotliSectionExtractor::default())
        .start()
        .unwrap();
    loop {}
}
```

## Example: Compsosing a Driver File

```rust
use alloc::vec::Vec;
use patina::pi::fw_fs::ffs;
use patina_ffs::file::File;
use patina_ffs::section::{Section, SectionHeader};
use r_efi::efi;

fn build_driver_section(pe_image: Vec<u8>) -> File {
    let mut file = File::new(efi::Guid::from_bytes(&[0u8; 16]), ffs::file::raw::r#type::DRIVER);
    let section = Section::new_from_header_with_data(
        SectionHeader::Standard(ffs::section::raw_type::PE32, pe_image.len() as u32),
        pe_image,
    )
    .expect("section build");
    file.sections_mut().push(section);
    file.set_data_checksum(true);
    file
}
```
