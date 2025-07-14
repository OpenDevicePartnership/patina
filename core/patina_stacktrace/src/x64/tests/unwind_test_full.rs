/// This is a comprehensive test to validate the stack-walking logic implemented
/// in `stacktrace.rs`. The test creates a separate thread in the process, loads
/// the `x64.dll`, and calls the `StartCallStack()` exported function, which, in
/// turn, creates the following call stack and waits. The test then retrieves
/// the `rsp` and `rip` of `func1()` to validate the stack-walking logic and
/// repeats this process for the other stack frames.
///
///  func1()
///  func2()
///  func3()
///  func4()
///  StartCallStack()
use std::ffi::CString;
use std::ptr;
use winapi::ctypes::c_void;
use winapi::shared::minwindef::{DWORD, HINSTANCE, HMODULE};
use winapi::um::errhandlingapi::GetLastError;
use winapi::um::libloaderapi::{FreeLibrary, GetProcAddress, LoadLibraryA};
use winapi::um::processthreadsapi::CreateThread;
use winapi::um::synchapi::WaitForSingleObject;

use crate::stacktrace::StackTrace;

use super::set_logger;

type StartCallStack = unsafe extern "system" fn();
type ContinueToNextFrame = unsafe extern "system" fn();
type GetCurrentFrameNumber = unsafe extern "system" fn() -> i32;
type GetCurrentRip = unsafe extern "system" fn() -> u64;
type GetReturnRip = unsafe extern "system" fn() -> u64;
type GetCurrentRsp = unsafe extern "system" fn() -> u64;

#[derive(Clone, Copy)]
struct FfiFunctionPointers {
    start_call_stack: StartCallStack,
    continue_to_next_frame: ContinueToNextFrame,
    get_current_frame_number: GetCurrentFrameNumber,
    get_current_rip: GetCurrentRip,
    get_return_rip: GetReturnRip,
    get_current_rsp: GetCurrentRsp,
}

#[test]
#[ignore = "This test requires a DLL and custom build steps"]
fn test_unwind_info_full() {
    set_logger();

    unsafe {
        let path = r"src\x64\tests\collateral\x64.dll";
        let dll_path = CString::new(path).unwrap();
        let dll: HMODULE = LoadLibraryA(dll_path.as_ptr());
        if dll.is_null() {
            log::info!("Failed to load DLL. Error: {}", GetLastError());
            return;
        }

        let Some(ffi_function_pointers) = load_functions(dll) else {
            log::info!("Failed to get address of all the required functions.");
            FreeLibrary(dll);
            return;
        };

        let boxed_ffi_function_pointers = Box::new(ffi_function_pointers);
        let raw_ffi_function_pointers = Box::into_raw(boxed_ffi_function_pointers) as *mut c_void;

        let mut thread_id = 0;
        let thread_handle = CreateThread(
            ptr::null_mut(),
            0,
            Some(call_stack_thread), // Trigger the call stack
            raw_ffi_function_pointers,
            0,
            &mut thread_id,
        );

        if thread_handle.is_null() {
            log::info!("Thread creation failed. Error: {}", GetLastError());
            FreeLibrary(dll);
            return;
        }

        log::info!("Thread created successfully! Thread ID: {}", thread_id);

        execute_frame_transitions(&ffi_function_pointers);
        WaitForSingleObject(thread_handle, winapi::um::winbase::INFINITE);
        FreeLibrary(dll);
    }
}

unsafe extern "system" fn call_stack_thread(raw_ffi_function_pointers: *mut c_void) -> DWORD {
    if raw_ffi_function_pointers.is_null() {
        return 1;
    }

    let boxed_ffi_function_pointers: Box<FfiFunctionPointers> =
        unsafe { Box::from_raw(raw_ffi_function_pointers as *mut FfiFunctionPointers) };

    unsafe { (boxed_ffi_function_pointers.start_call_stack)() };

    0
}

unsafe fn execute_frame_transitions(ffi_function_pointers: &FfiFunctionPointers) {
    let mut return_rip;
    let mut current_rsp;

    // Frame 1 - func1():
    while unsafe { (ffi_function_pointers.get_current_frame_number)() } < 1 {
        // Wait until thread gets in to frame 1 aka func1()
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let current_rip = unsafe { (ffi_function_pointers.get_current_rip)() };
    current_rsp = unsafe { (ffi_function_pointers.get_current_rsp)() };
    return_rip = unsafe { (ffi_function_pointers.get_return_rip)() };

    log::info!("[+] Required call stack has been setup. Dumping it using Stack unwind implementation...");

    // Dump the call stack using StackTrace lib
    let res = unsafe { StackTrace::dump_with(current_rip, current_rsp) };
    assert!(res.is_ok());

    log::info!("\n[+] Unwinding the stack one frame at a time. Querying the actual Rsp/Return Rip...");
    log::info!("[+] # Current RSP  Return RIP");
    log::info!("    0 {:X}   {:X}", current_rsp, return_rip);

    // At Frame 2 - func2() - Unwind frame 1:
    // This should trigger completion of func1() and func2() will loop
    unsafe { (ffi_function_pointers.continue_to_next_frame)() }; // Trigger thread to get in to frame 2 aka func2()
    while unsafe { (ffi_function_pointers.get_current_frame_number)() } < 2 {
        // Wait until thread gets in to frame 2 aka func2()
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // current_rip = (ffi_function_pointers.get_current_rip)();
    current_rsp = unsafe { (ffi_function_pointers.get_current_rsp)() };
    return_rip = unsafe { (ffi_function_pointers.get_return_rip)() };
    log::info!("    1 {:X}   {:X}", current_rsp, return_rip);

    // At Frame 3 - func3() - Unwind frame 2:
    // This should trigger completion of func2() and func3() will loop
    unsafe { (ffi_function_pointers.continue_to_next_frame)() }; // Trigger thread to get in to frame 3 aka func3()
    while unsafe { (ffi_function_pointers.get_current_frame_number)() } < 3 {
        // Wait until thread gets in to frame 3 aka func3()
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // current_rip = (ffi_function_pointers.get_current_rip)();
    current_rsp = unsafe { (ffi_function_pointers.get_current_rsp)() };
    return_rip = unsafe { (ffi_function_pointers.get_return_rip)() };
    log::info!("    2 {:X}   {:X}", current_rsp, return_rip);

    // At Frame 4 - func4() - Unwind frame 3:
    // This should trigger completion of func3() and func4() will loop
    unsafe { (ffi_function_pointers.continue_to_next_frame)() }; // Trigger thread to get in to frame 4 aka func4()
    while unsafe { (ffi_function_pointers.get_current_frame_number)() } < 4 {
        // Wait until thread gets in to frame 4 aka func4()
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // current_rip = (ffi_function_pointers.get_current_rip)();
    current_rsp = unsafe { (ffi_function_pointers.get_current_rsp)() };
    return_rip = unsafe { (ffi_function_pointers.get_return_rip)() };
    log::info!("    3 {:X}   {:X}", current_rsp, return_rip);

    // Unwind frame 4:
    // This should trigger completion of func4()
    unsafe { (ffi_function_pointers.continue_to_next_frame)() };
}

unsafe fn load_functions(dll: HINSTANCE) -> Option<FfiFunctionPointers> {
    let name_cstr = CString::new("StartCallStack").unwrap();
    let proc_address = unsafe { GetProcAddress(dll, name_cstr.as_ptr()) };
    let start_call_stack: StartCallStack = if proc_address.is_null() {
        log::info!("Failed to get function address for StartCallStack. Error: {}", unsafe { GetLastError() });
        return None;
    } else {
        unsafe { std::mem::transmute::<*mut winapi::shared::minwindef::__some_function, StartCallStack>(proc_address) }
    };

    let name_cstr = CString::new("ContinueToNextFrame").unwrap();
    let proc_address = unsafe { GetProcAddress(dll, name_cstr.as_ptr()) };
    let continue_to_next_frame: ContinueToNextFrame = if proc_address.is_null() {
        log::info!("Failed to get function address for ContinueToNextFrame. Error: {}", unsafe { GetLastError() });
        return None;
    } else {
        unsafe {
            std::mem::transmute::<*mut winapi::shared::minwindef::__some_function, ContinueToNextFrame>(proc_address)
        }
    };

    let name_cstr = CString::new("GetCurrentFrameNumber").unwrap();
    let proc_address = unsafe { GetProcAddress(dll, name_cstr.as_ptr()) };
    let get_current_frame_number: GetCurrentFrameNumber = if proc_address.is_null() {
        log::info!("Failed to get function address for GetCurrentFrameNumber. Error: {}", unsafe { GetLastError() });
        return None;
    } else {
        unsafe {
            std::mem::transmute::<*mut winapi::shared::minwindef::__some_function, GetCurrentFrameNumber>(proc_address)
        }
    };

    let name_cstr = CString::new("GetCurrentRip").unwrap();
    let proc_address = unsafe { GetProcAddress(dll, name_cstr.as_ptr()) };
    let get_current_rip: GetCurrentRip = if proc_address.is_null() {
        log::info!("Failed to get function address for GetCurrentRip. Error: {}", unsafe { GetLastError() });
        return None;
    } else {
        unsafe { std::mem::transmute::<*mut winapi::shared::minwindef::__some_function, GetCurrentRip>(proc_address) }
    };

    let name_cstr = CString::new("GetReturnRip").unwrap();
    let proc_address = unsafe { GetProcAddress(dll, name_cstr.as_ptr()) };
    let get_return_rip: GetReturnRip = if proc_address.is_null() {
        log::info!("Failed to get function address for GetReturnRip. Error: {}", unsafe { GetLastError() });
        return None;
    } else {
        unsafe { std::mem::transmute::<*mut winapi::shared::minwindef::__some_function, GetReturnRip>(proc_address) }
    };

    let name_cstr = CString::new("GetCurrentRsp").unwrap();
    let proc_address = unsafe { GetProcAddress(dll, name_cstr.as_ptr()) };
    let get_current_rsp: GetCurrentRsp = if proc_address.is_null() {
        log::info!("Failed to get function address for GetCurrentRsp. Error: {}", unsafe { GetLastError() });
        return None;
    } else {
        unsafe { std::mem::transmute::<*mut winapi::shared::minwindef::__some_function, GetCurrentRsp>(proc_address) }
    };

    Some(FfiFunctionPointers {
        start_call_stack,
        continue_to_next_frame,
        get_current_frame_number,
        get_current_rip,
        get_return_rip,
        get_current_rsp,
    })
}
