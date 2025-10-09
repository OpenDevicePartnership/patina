extern crate patina_pi;
use alloc_no_stdlib::{self, SliceWrapper, SliceWrapperMut, define_index_ops_mut};
use brotli_decompressor::{BrotliDecompressStream, BrotliResult, BrotliState, HuffmanCode};
use patina_pi::fw_fs::{FirmwareVolume, SectionExtractor, SectionMetaData, guid};
use r_efi::efi;
use std::{env, error::Error, fmt::Debug, fs, path::Path};

//Rebox and HeapAllocator satisfy BrotliDecompress custom allocation requirement.
struct Rebox<T>(Box<[T]>);

impl<T> core::default::Default for Rebox<T> {
    fn default() -> Self {
        Rebox(Vec::new().into_boxed_slice())
    }
}
define_index_ops_mut!(T, Rebox<T>);

impl<T> alloc_no_stdlib::SliceWrapper<T> for Rebox<T> {
    fn slice(&self) -> &[T] {
        &self.0
    }
}

impl<T> alloc_no_stdlib::SliceWrapperMut<T> for Rebox<T> {
    fn slice_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

struct HeapAllocator<T: Clone> {
    pub default_value: T,
}

impl<T: Clone> alloc_no_stdlib::Allocator<T> for HeapAllocator<T> {
    type AllocatedMemory = Rebox<T>;
    fn alloc_cell(self: &mut HeapAllocator<T>, len: usize) -> Rebox<T> {
        Rebox(vec![self.default_value.clone(); len].into_boxed_slice())
    }
    fn free_cell(self: &mut HeapAllocator<T>, _data: Rebox<T>) {}
}

#[derive(Debug, Clone, Copy)]
struct BrotliSectionExtractor {}

impl SectionExtractor for BrotliSectionExtractor {
    fn extract(&self, section: &patina_pi::fw_fs::Section) -> Result<Box<[u8]>, efi::Status> {
        if let SectionMetaData::GuidDefined(guid_header, _) = section.meta_data()
            && guid_header.section_definition_guid == guid::BROTLI_SECTION
        {
            let data = section.section_data();
            let out_size = u64::from_le_bytes(data[0..8].try_into().unwrap());
            let _scratch_size = u64::from_le_bytes(data[8..16].try_into().unwrap());

            let mut brotli_state = BrotliState::new(
                HeapAllocator::<u8> { default_value: 0 },
                HeapAllocator::<u32> { default_value: 0 },
                HeapAllocator::<HuffmanCode> { default_value: Default::default() },
            );
            let in_data = &data[16..];
            let mut out_data = vec![0u8; out_size as usize];
            let mut out_data_size = 0;
            let result = BrotliDecompressStream(
                &mut in_data.len(),
                &mut 0,
                &data[16..],
                &mut out_data.len(),
                &mut 0,
                out_data.as_mut_slice(),
                &mut out_data_size,
                &mut brotli_state,
            );

            if matches!(result, BrotliResult::ResultSuccess) {
                return Ok(out_data.into_boxed_slice());
            } else {
                return Err(efi::Status::VOLUME_CORRUPTED);
            }
        }
        Ok(Box::new([0u8; 0]))
    }
}

struct PrettyMetaData<'a>(&'a SectionMetaData);

impl Debug for PrettyMetaData<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            SectionMetaData::GuidDefined(guid_header, _) => f
                .debug_struct("GuidDefined")
                .field(
                    "section_definition_guid",
                    &uuid::Uuid::from_bytes(*guid_header.section_definition_guid.as_bytes()),
                )
                .finish_non_exhaustive(),
            section => f.write_fmt(format_args!("{:?}", section)),
        }
    }
}

fn print_fv(fv: FirmwareVolume) -> Result<(), efi::Status> {
    println!("FV: {:x?}", fv.fv_name().map(|x| uuid::Uuid::from_bytes(*x.as_bytes())));
    println!("  BlockMap: {:x?}", fv.block_map());
    println!("  Files: ");
    for (file_idx, file) in fv.file_iter().enumerate() {
        let file = file?;
        println!(
            "    ({:?}, name: {:x?}, type: {:?}, size: {:x?})",
            file_idx,
            uuid::Uuid::from_bytes(*file.name().as_bytes()),
            file.file_type(),
            file.size()
        );
        println!("    Sections:");
        for (section_idx, section) in file.section_iter_with_extractor(&BrotliSectionExtractor {}).enumerate() {
            let section = section?;
            println!(
                "      ({:?}, type: {:?}, metadata: {:x?}",
                section_idx,
                section.section_type(),
                PrettyMetaData(section.meta_data())
            );
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");

    let fv_bytes = fs::read(root.join("FVMAIN_COMPACT.Fv"))?;
    let fv = FirmwareVolume::new(&fv_bytes).unwrap();

    print_fv(fv).unwrap();

    Ok(())
}
