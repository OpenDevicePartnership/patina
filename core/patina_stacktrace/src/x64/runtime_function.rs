use super::unwind::UnwindInfo;
use crate::byte_reader::ByteReader;
use crate::error::{Error, StResult};
use crate::pe::PE;
use core::fmt;

/// `RuntimeFunction`
/// Source: <https://learn.microsoft.com/en-us/cpp/build/exception-handling-x64?view=msvc-170#struct-runtime_function>
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

    /// rva for unwind info
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
    pub fn get_unwind_info(&self) -> StResult<UnwindInfo> {
        UnwindInfo::parse(&self.image_base[self.unwind_info as usize..], self.image_name)
            .map_err(|_| Error::UnwindInfoNotFound(self.image_name, self.image_base.as_ptr() as u64, self.unwind_info))
    }

    /// Function to return the Runtime Function corresponding to the given
    /// relative rip.
    pub unsafe fn find_function(pe: &PE<'a>, rip_rva: u32) -> StResult<RuntimeFunction<'a>> {
        let (exception_table_rva, exception_table_size) = pe.get_exception_table()?;

        // Jump to .pdata section and parse the Runtime Function records.
        // - Break the section in to 12 byte chunks
        // - Map the 12 bytes in to 3 u32
        // - Filter the chunk which fall with in the given rva range
        // - Map the chunk in to RuntimeFunction
        let runtime_function = pe.bytes
            [exception_table_rva as usize..(exception_table_rva + exception_table_size) as usize]
            .chunks(core::mem::size_of::<u32>() * 3) // 3 u32
            .map(|ele| {
                (
                    ele.read32(0).unwrap(), // start_rva
                    ele.read32(4).unwrap(), // end_rva
                    ele.read32(8).unwrap(), // unwindinfo_rva
                )
            })
            .find(|ele| ele.0 <= rip_rva && rip_rva <= ele.1)
            .map(|ele| RuntimeFunction::new(pe.bytes, pe.image_name, ele.0, ele.1, ele.2));

        runtime_function.ok_or(Error::RuntimeFunctionNotFound(pe.image_name, rip_rva))
    }

    /// Windows only test function to return all Runtime Functions
    #[cfg(all(target_os = "windows", target_arch = "x86_64", test))]
    pub(crate) unsafe fn find_all_functions(pe: &PE<'a>) -> StResult<Vec<RuntimeFunction<'a>>> {
        let (exception_table_rva, exception_table_size) = pe.get_exception_table()?;

        // Jump to .pdata section and parse the Runtime Function records.
        // - Break the section in to 12 byte chunks
        // - Map the 12 bytes in to 3 u32
        // - Map each chunk in to RuntimeFunction
        let runtime_functions = pe.bytes
            [exception_table_rva as usize..(exception_table_rva + exception_table_size) as usize]
            .chunks(4)
            .map(|ele| ele.read32(0).unwrap())
            .collect::<Vec<u32>>()
            .chunks(3)
            .map(|ele| RuntimeFunction::new(pe.bytes, pe.image_name, ele[0], ele[1], ele[2]))
            .collect::<Vec<RuntimeFunction>>();

        Ok(runtime_functions)
    }
}
