use super::unwind::UnwindInfo;
use crate::byte_reader::ByteReader;
use crate::error::{Error, StResult};
use crate::pe::PE;
use core::fmt;

/// `RuntimeFunction`
/// Source: <https://learn.microsoft.com/en-us/cpp/build/arm64-exception-handling>
#[derive(Debug, Clone)]
pub struct RuntimeFunction<'a> {
    /// loaded image memory as a byte slice
    image_base: &'a [u8],

    /// image name extracted from the loaded pe image
    image_name: Option<&'static str>,

    /// start of the function rva
    pub start_rva: u32,

    /// end of the function rva
    pub end_rva: u32,

    /// unwind info in AArch64. Second word of .pdata section.
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

    /// Parse the Unwind Info data pointed by RuntimeFunction
    pub fn get_unwind_info(&self) -> StResult<UnwindInfo<'_>> {
        UnwindInfo::parse(self.image_base, self.unwind_info, self.image_name)
            .map_err(|_| Error::UnwindInfoNotFound(self.image_name, self.image_base.as_ptr() as u64, self.unwind_info))
    }

    /// Function to return the Runtime Function corresponding to the given
    /// relative pc.
    pub unsafe fn find_function(pe: &PE<'a>, pc_rva: u32) -> StResult<RuntimeFunction<'a>> {
        let (exception_table_rva, exception_table_size) = unsafe { pe.get_exception_table()? };

        // Jump to .pdata section and parse the Runtime Function records.
        // - Break the section in to 8 byte chunks(2 u32)
        // - Map the 8 bytes in to 2 u32
        // - Filter the chunk which fall within the given rva range
        // - Map the chunk in to RuntimeFunction
        let runtime_function = pe.bytes
            [exception_table_rva as usize..(exception_table_rva + exception_table_size) as usize]
            .chunks(core::mem::size_of::<u32>() * 2) // 2 u32
            .map(|ele| {
                let start_rva = ele.read32(0).unwrap();
                let unwind_info = ele.read32(4).unwrap();

                let flag = unwind_info & 0x3;

                let function_length = match flag {
                    // packed unwind data not used; remaining bits point to an
                    // .xdata record. The length of the function can only be
                    // calculated by parsing the [0..=17] bits of .xdata record.
                    // It indicates the total length of the function in bytes,
                    // divided by 4
                    0 => {
                        let xdata_rva = unwind_info as usize;
                        let xdata_header = &pe.bytes[xdata_rva..xdata_rva + 4];
                        let xdata_header = xdata_header.read32(0).unwrap();
                        (xdata_header & 0x3FFFF) * 4
                    }
                    // packed unwind data used with a single prolog and epilog
                    // at the beginning and end of the scope. The length of the
                    // function is specified directly in .pdata[2] [2..=12]. It
                    // indicates the total length of the function in bytes,
                    // divided by 4
                    1 => ((unwind_info >> 2) & 0x7FF) * 4,
                    // packed unwind data used for code without any prolog and
                    // epilog. Useful for describing separated function
                    // segments. The length of the function is specified
                    // directly in .pdata[2] [2..=12]. It indicates the total
                    // length of the function in bytes, divided by 4
                    2 => ((unwind_info >> 2) & 0x7FF) * 4,
                    // reserved
                    _ => 0,
                };

                let end_rva = start_rva + function_length;

                (start_rva, end_rva, unwind_info)
            })
            .find(|ele| ele.0 <= pc_rva && pc_rva <= ele.1)
            .map(|ele| RuntimeFunction::new(pe.bytes, pe.image_name, ele.0, ele.1, ele.2));

        runtime_function.ok_or(Error::RuntimeFunctionNotFound(pe.image_name, pc_rva))
    }

    /// Windows only test function to return all Runtime Functions
    #[cfg(all(target_os = "windows", target_arch = "aarch64", test))]
    pub unsafe fn find_all_functions(pe: &PE<'a>) -> StResult<Vec<RuntimeFunction<'a>>> {
        let (exception_table_rva, exception_table_size) = pe.get_exception_table()?;

        // Jump to .pdata section and parse the Runtime Function records.
        // - Break the section in to 8 byte chunks(2 u32)
        // - Map the 8 bytes in to 2 u32
        // - Map each chunk in to RuntimeFunction
        let runtime_functions = pe.bytes
            [exception_table_rva as usize..(exception_table_rva + exception_table_size) as usize]
            .chunks(4)
            .map(|ele| ele.read32(0).unwrap())
            .collect::<Vec<u32>>()
            .chunks(2) // 2 u32
            .map(|ele| {
                let start_rva = ele[0];
                let unwind_info = ele[1];

                let flag = unwind_info & 0x3;
                let function_length = match flag {
                    // packed unwind data not used; remaining bits point to an
                    // .xdata record. The length of the function can only be
                    // calculated by parsing the [0..=17] bits of .xdata record.
                    // It indicates the total length of the function in bytes,
                    // divided by 4
                    0 => {
                        let xdata_rva = unwind_info as usize;
                        let xdata_header = &pe.bytes[xdata_rva..xdata_rva + 4];
                        let xdata_header = xdata_header.read32(0).unwrap();
                        (xdata_header & 0x3FFFF) * 4
                    }
                    // packed unwind data used with a single prolog and epilog
                    // at the beginning and end of the scope. The length of the
                    // function is specified directly in .pdata[2] [2..=12]. It
                    // indicates the total length of the function in bytes,
                    // divided by 4
                    1 => ((unwind_info >> 2) & 0x7FF) * 4,
                    // packed unwind data used for code without any prolog and
                    // epilog. Useful for describing separated function
                    // segments. The length of the function is specified
                    // directly in .pdata[2] [2..=12]. It indicates the total
                    // length of the function in bytes, divided by 4
                    2 => ((unwind_info >> 2) & 0x7FF) * 4,
                    // reserved
                    _ => 0,
                };

                let end_rva = start_rva + function_length;

                (start_rva, end_rva, unwind_info)
            })
            .map(|ele| RuntimeFunction::new(pe.bytes, pe.image_name, ele.0, ele.1, ele.2))
            .collect::<Vec<RuntimeFunction>>();

        Ok(runtime_functions)
    }
}
