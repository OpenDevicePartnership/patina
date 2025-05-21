use std::os::windows::ffi::OsStrExt;
/// `Module` struct is used for tests to manually load a binary to memory for
/// test execution. This module is not used by the actual stack trace lib.
use std::path::PathBuf;
use std::ptr;
use winapi::shared::minwindef::HINSTANCE;
use winapi::um::libloaderapi::{FreeLibrary, LoadLibraryW};
use winapi::um::processthreadsapi::GetCurrentProcess;
use winapi::um::psapi::{GetModuleInformation, MODULEINFO};

pub struct Module {
    pub base_address: u64,
    pub module: HINSTANCE,
    pub _path: String,
}

impl Module {
    /// Use Win32 API to load the given binary in to memory
    pub fn load(path: &str) -> Result<Module, String> {
        // Convert the DLL path to a wide string (UTF-16)
        let path_buf = PathBuf::from(path);
        let path_wide: Vec<u16> = path_buf
            .as_os_str()
            .encode_wide()
            .chain(Some(0)) // Null terminator
            .collect();

        unsafe {
            // Load the DLL
            let module_handle = LoadLibraryW(path_wide.as_ptr());
            if module_handle.is_null() {
                return Err("Failed to load library".to_string());
            }

            // Get the current process handle
            let current_process = GetCurrentProcess();

            // Retrieve module information
            let mut module_info =
                MODULEINFO { lpBaseOfDll: ptr::null_mut(), SizeOfImage: 0, EntryPoint: ptr::null_mut() };

            if GetModuleInformation(
                current_process,
                module_handle,
                &mut module_info,
                std::mem::size_of::<MODULEINFO>() as u32,
            ) == 0
            {
                FreeLibrary(module_handle); // Unload the library
                return Err("Failed to get module information".to_string());
            }

            Ok(Module { base_address: module_info.lpBaseOfDll as u64, module: module_handle, _path: path.to_string() })
        }
    }

    /// Use Win32 API to unload the binary from memory
    pub fn unload(&self) {
        unsafe {
            FreeLibrary(self.module);
        }
    }
}
