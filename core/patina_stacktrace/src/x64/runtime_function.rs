use super::unwind::UnwindInfo;

use crate::{
    byte_reader::ByteReader,
    error::{Error, StResult},
    pe::PE,
    stacktrace::StackFrame,
};
use core::fmt;

/// `RuntimeFunction`
/// Source: <https://learn.microsoft.com/en-us/cpp/build/exception-handling-x64?view=msvc-170#struct-runtime_function>
#[derive(Debug, Clone)]
pub struct RuntimeFunction<'a> {
    /// Loaded image memory as a byte slice.
    image_base: &'a [u8],

    /// Image name extracted from the loaded PE image.
    image_name: Option<&'static str>,

    /// Start of the function RVA.
    pub start_rva: u32,

    /// End of the function RVA.
    pub end_rva: u32,

    /// RVA for the unwind info.
    pub unwind_info: u32,
}

impl fmt::Display for RuntimeFunction<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RuntimeFunction {{ image_base:0x{:p}, start_rva: 0x{:08X}, end_rva: 0x{:08X}, unwind_info: 0x{:08X} }}",
            self.image_base.as_ptr(),
            self.start_rva,
            self.end_rva,
            self.unwind_info
        )
    }
}

impl<'a> RuntimeFunction<'a> {
    pub fn new(
        image_base: &'a [u8],
        image_name: Option<&'static str>,
        start_rva: u32,
        end_rva: u32,
        unwind_info: u32,
    ) -> Self {
        Self { image_base, image_name, start_rva, end_rva, unwind_info }
    }

    /// Parses the unwind info referenced by this runtime function entry.
    pub fn get_unwind_info(&self) -> StResult<UnwindInfo<'_>> {
        UnwindInfo::parse(&self.image_base[self.unwind_info as usize..], self.image_name).map_err(|_| {
            Error::UnwindInfoNotFound {
                module: self.image_name,
                image_base: self.image_base.as_ptr() as u64,
                unwind_info: self.unwind_info,
            }
        })
    }

    /// Finds the runtime function corresponding to the given relative RIP.
    ///
    /// # Safety
    /// `stack_frame` must reflect a live frame within `pe`; the PC and SP values are
    /// trusted to fall inside the mapped PE image and its stack allocation. Passing
    /// arbitrary register snapshots risks indexing the exception table with invalid
    /// offsets.
    pub fn find_function(pe: &PE<'a>, stack_frame: &mut StackFrame) -> StResult<RuntimeFunction<'a>> {
        let rip_rva = (stack_frame.pc - pe.base_address) as u32;

        // SAFETY: `pe` owns the underlying PE image bytes and guarantees the header
        // ranges referenced by `get_exception_table()` remain readable here.
        let (exception_table_rva, exception_table_size) = unsafe { pe.get_exception_table()? };

        if (exception_table_size as usize) < (core::mem::size_of::<u32>() * 3) {
            return Err(Error::Malformed { module: pe.image_name, reason: "Invalid exception table size < 12 bytes" });
        }

        // Jump to the `.pdata` section and parse the runtime function records
        // by breaking the section into 12-byte chunks, mapping each chunk to
        // three u32 values, filtering the chunks that fall within the given RVA
        // range, and constructing `RuntimeFunction` instances from the results.
        let runtime_function = pe.bytes
            [exception_table_rva as usize..(exception_table_rva + exception_table_size) as usize]
            .chunks(core::mem::size_of::<u32>() * 3) // 3 u32
            .map(|ele| {
                (
                    ele.read32(0).unwrap(), // start_rva - unwrap() will work validated above
                    ele.read32(4).unwrap(), // end_rva - unwrap() will work validated above
                    ele.read32(8).unwrap(), // unwindinfo_rva - unwrap() will work validated above
                )
            })
            .find(|ele| ele.0 <= rip_rva && rip_rva < ele.1)
            .map(|ele| RuntimeFunction::new(pe.bytes, pe.image_name, ele.0, ele.1, ele.2));

        runtime_function.ok_or(Error::RuntimeFunctionNotFound { module: pe.image_name, rip_rva })
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::{error::Error, pe::PE, stacktrace::StackFrame};

    const IMAGE_SIZE: usize = 0x800;
    const PE_POINTER_OFFSET: usize = 0x3C;
    const PE_HEADER_OFFSET: u32 = 0x80;
    const PE_SIGNATURE: u32 = 0x0000_4550;
    const PE64_MAGIC: u16 = 0x20B;
    const SIZE_OF_IMAGE_OFFSET: usize = 0x50;
    const EXCEPTION_TABLE_POINTER_OFFSET: usize = 0xA0;

    fn build_pe_bytes(entries: &[(u32, u32, u32)], unwind_sections: &[(u32, &[u8])]) -> Vec<u8> {
        let mut bytes = vec![0u8; IMAGE_SIZE];

        bytes[0..2].copy_from_slice(&0x5A4Du16.to_le_bytes());
        bytes[PE_POINTER_OFFSET..PE_POINTER_OFFSET + 4].copy_from_slice(&PE_HEADER_OFFSET.to_le_bytes());

        let header = PE_HEADER_OFFSET as usize;
        bytes[header..header + 4].copy_from_slice(&PE_SIGNATURE.to_le_bytes());
        bytes[header + 0x18..header + 0x1A].copy_from_slice(&PE64_MAGIC.to_le_bytes());
        bytes[header + SIZE_OF_IMAGE_OFFSET..header + SIZE_OF_IMAGE_OFFSET + 4]
            .copy_from_slice(&(IMAGE_SIZE as u32).to_le_bytes());

        let exception_rva = 0x300u32;
        let exception_size = (entries.len() * core::mem::size_of::<u32>() * 3) as u32;
        bytes[header + EXCEPTION_TABLE_POINTER_OFFSET..header + EXCEPTION_TABLE_POINTER_OFFSET + 4]
            .copy_from_slice(&exception_rva.to_le_bytes());
        bytes[header + EXCEPTION_TABLE_POINTER_OFFSET + 4..header + EXCEPTION_TABLE_POINTER_OFFSET + 8]
            .copy_from_slice(&exception_size.to_le_bytes());

        for (index, &(start, end, unwind)) in entries.iter().enumerate() {
            let offset = exception_rva as usize + index * 12;
            bytes[offset..offset + 4].copy_from_slice(&start.to_le_bytes());
            bytes[offset + 4..offset + 8].copy_from_slice(&end.to_le_bytes());
            bytes[offset + 8..offset + 12].copy_from_slice(&unwind.to_le_bytes());
        }

        for &(rva, data) in unwind_sections {
            let offset = rva as usize;
            let end = offset + data.len();
            assert!(end <= bytes.len(), "unwind section exceeds image bounds");
            bytes[offset..end].copy_from_slice(data);
        }

        bytes
    }

    #[test]
    fn get_unwind_info_parses_minimal_header() {
        let mut image = vec![0u8; 0x80];
        let unwind_blob = [0x01, 0x00, 0x00, 0x00];
        image[0x40..0x44].copy_from_slice(&unwind_blob);
        let runtime = RuntimeFunction::new(&image, Some("image"), 0x10, 0x20, 0x40);
        let unwind = runtime.get_unwind_info().expect("expected valid unwind info");
        assert_eq!(unwind.get_stack_pointer_offset().unwrap(), 0);
        let summary = format!("{unwind}");
        assert!(summary.contains("version: 0x01"));
    }

    #[test]
    fn get_unwind_info_translates_parse_errors() {
        let mut image = vec![0u8; 0x40];
        image[0x10] = 0x03; // Unsupported version -> parse error.
        let runtime = RuntimeFunction::new(&image, Some("image"), 0x10, 0x20, 0x10);
        let error = runtime.get_unwind_info().unwrap_err();
        assert!(matches!(error, Error::UnwindInfoNotFound { module: Some("image"), unwind_info: 0x10, .. }));
    }

    #[test]
    fn find_function_returns_matching_range() {
        let entries = [(0x100, 0x180, 0x500), (0x200, 0x280, 0x520)];
        let unwind_data = [(0x500u32, &[0x01u8, 0x00, 0x00, 0x00][..])];
        let image = build_pe_bytes(&entries, &unwind_data);
        let pe = PE { base_address: 0, _size_of_image: image.len() as u32, image_name: Some("image"), bytes: &image };
        let mut frame = StackFrame { pc: 0x120, ..StackFrame::default() };

        let runtime = RuntimeFunction::find_function(&pe, &mut frame).expect("expected runtime function");
        assert_eq!(runtime.start_rva, 0x100);
        assert_eq!(runtime.end_rva, 0x180);
        assert_eq!(runtime.unwind_info, 0x500);
    }

    #[test]
    fn find_function_skips_exclusive_end_boundary() {
        let entries = [(0x100, 0x150, 0x500), (0x150, 0x1A0, 0x520)];
        let unwind_data = [(0x500u32, &[0x01u8, 0x00, 0x00, 0x00][..]), (0x520u32, &[0x01u8, 0x00, 0x00, 0x00][..])];
        let image = build_pe_bytes(&entries, &unwind_data);
        let pe = PE { base_address: 0, _size_of_image: image.len() as u32, image_name: Some("image"), bytes: &image };
        let mut frame = StackFrame { pc: 0x150, ..StackFrame::default() };

        let runtime = RuntimeFunction::find_function(&pe, &mut frame).expect("expected boundary runtime function");
        assert_eq!(runtime.start_rva, 0x150);
        assert_eq!(runtime.end_rva, 0x1A0);
    }

    #[test]
    fn find_function_reports_missing_ranges() {
        let entries = [(0x100, 0x180, 0x500)];
        let unwind_data = [(0x500u32, &[0x01u8, 0x00, 0x00, 0x00][..])];
        let image = build_pe_bytes(&entries, &unwind_data);
        let pe = PE { base_address: 0, _size_of_image: image.len() as u32, image_name: Some("image"), bytes: &image };
        let mut frame = StackFrame { pc: 0x1C0, ..StackFrame::default() };

        let error = RuntimeFunction::find_function(&pe, &mut frame).unwrap_err();
        assert!(matches!(error, Error::RuntimeFunctionNotFound { module: Some("image"), rip_rva: 0x1C0 }));
    }
}
