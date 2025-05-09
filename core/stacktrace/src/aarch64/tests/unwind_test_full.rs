/// This is a comprehensive test to validate the stack-walking logic implemented
/// in `stacktrace.rs`. The test creates a separate thread in the process, loads
/// the `aarch64.dll`, and calls the `StartCallStack()` exported function, which, in
/// turn, creates the following call stack and waits. The test then retrieves
/// the `sp` and `pc` of `func1()` to validate the stack-walking logic and
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
type GetCurrentPc = unsafe extern "system" fn() -> u64;
type GetReturnPc = unsafe extern "system" fn() -> u64;
type GetCurrentSp = unsafe extern "system" fn() -> u64;

#[derive(Clone, Copy)]
struct FfiFunctionPointers {
    start_call_stack: StartCallStack,
    continue_to_next_frame: ContinueToNextFrame,
    get_current_frame_number: GetCurrentFrameNumber,
    get_current_pc: GetCurrentPc,
    get_return_pc: GetReturnPc,
    get_current_sp: GetCurrentSp,
}

#[test]
#[ignore = "This test requires a DLL and custom build steps"]
fn test_unwind_info_full() {
    set_logger();

    unsafe {
        let path = r"src\aarch64\tests\collateral\aarch64.dll";
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
        Box::from_raw(raw_ffi_function_pointers as *mut FfiFunctionPointers);

    (boxed_ffi_function_pointers.start_call_stack)();

    0
}

unsafe fn execute_frame_transitions(ffi_function_pointers: &FfiFunctionPointers) {
    let mut return_pc;
    let mut current_sp;

    // Frame 1 - func1():
    while (ffi_function_pointers.get_current_frame_number)() < 1 {
        // Wait until thread gets in to frame 1 aka func1()
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let current_pc = (ffi_function_pointers.get_current_pc)();
    current_sp = (ffi_function_pointers.get_current_sp)();
    return_pc = (ffi_function_pointers.get_return_pc)();

    log::info!("[+] Required call stack has been setup. Dumping it using Stack unwind implementation...");

    // Dump the call stack using StackTrace lib
    let res = StackTrace::dump_with(current_pc, current_sp);
    assert!(res.is_ok());

    log::info!("\n[+] Unwinding the stack one frame at a time. Querying the actual SP/Return PC...");
    log::info!("[+] # Current  SP  Return  PC");
    log::info!("    0 {:X}   {:X}", current_sp, return_pc);

    // At Frame 2 - func2() - Unwind frame 1:
    // This should trigger completion of func1() and func2() will loop
    (ffi_function_pointers.continue_to_next_frame)(); // Trigger thread to get in to frame 2 aka func2()
    while (ffi_function_pointers.get_current_frame_number)() < 2 {
        // Wait until thread gets in to frame 2 aka func2()
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // current_pc = (ffi_function_pointers.get_current_pc)();
    current_sp = (ffi_function_pointers.get_current_sp)();
    return_pc = (ffi_function_pointers.get_return_pc)();
    log::info!("    1 {:X}   {:X}", current_sp, return_pc);

    // At Frame 3 - func3() - Unwind frame 2:
    // This should trigger completion of func2() and func3() will loop
    (ffi_function_pointers.continue_to_next_frame)(); // Trigger thread to get in to frame 3 aka func3()
    while (ffi_function_pointers.get_current_frame_number)() < 3 {
        // Wait until thread gets in to frame 3 aka func3()
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // current_pc = (ffi_function_pointers.get_current_pc)();
    current_sp = (ffi_function_pointers.get_current_sp)();
    return_pc = (ffi_function_pointers.get_return_pc)();
    log::info!("    2 {:X}   {:X}", current_sp, return_pc);

    // At Frame 4 - func4() - Unwind frame 3:
    // This should trigger completion of func3() and func4() will loop
    (ffi_function_pointers.continue_to_next_frame)(); // Trigger thread to get in to frame 4 aka func4()
    while (ffi_function_pointers.get_current_frame_number)() < 4 {
        // Wait until thread gets in to frame 4 aka func4()
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // current_pc = (ffi_function_pointers.get_current_pc)();
    current_sp = (ffi_function_pointers.get_current_sp)();
    return_pc = (ffi_function_pointers.get_return_pc)();
    log::info!("    3 {:X}   {:X}", current_sp, return_pc);

    // Unwind frame 4:
    // This should trigger completion of func4()
    (ffi_function_pointers.continue_to_next_frame)();
}

unsafe fn load_functions(dll: HINSTANCE) -> Option<FfiFunctionPointers> {
    let name_cstr = CString::new("StartCallStack").unwrap();
    let proc_address = GetProcAddress(dll, name_cstr.as_ptr());
    let start_call_stack: StartCallStack = if proc_address.is_null() {
        log::info!("Failed to get function address for StartCallStack. Error: {}", GetLastError());
        return None;
    } else {
        std::mem::transmute::<*mut winapi::shared::minwindef::__some_function, StartCallStack>(proc_address)
    };

    let name_cstr = CString::new("ContinueToNextFrame").unwrap();
    let proc_address = GetProcAddress(dll, name_cstr.as_ptr());
    let continue_to_next_frame: ContinueToNextFrame = if proc_address.is_null() {
        log::info!("Failed to get function address for ContinueToNextFrame. Error: {}", GetLastError());
        return None;
    } else {
        std::mem::transmute::<*mut winapi::shared::minwindef::__some_function, ContinueToNextFrame>(proc_address)
    };

    let name_cstr = CString::new("GetCurrentFrameNumber").unwrap();
    let proc_address = GetProcAddress(dll, name_cstr.as_ptr());
    let get_current_frame_number: GetCurrentFrameNumber = if proc_address.is_null() {
        log::info!("Failed to get function address for GetCurrentFrameNumber. Error: {}", GetLastError());
        return None;
    } else {
        std::mem::transmute::<*mut winapi::shared::minwindef::__some_function, GetCurrentFrameNumber>(proc_address)
    };

    let name_cstr = CString::new("GetCurrentPc").unwrap();
    let proc_address = GetProcAddress(dll, name_cstr.as_ptr());
    let get_current_pc: GetCurrentPc = if proc_address.is_null() {
        log::info!("Failed to get function address for GetCurrentPc. Error: {}", GetLastError());
        return None;
    } else {
        std::mem::transmute::<*mut winapi::shared::minwindef::__some_function, GetCurrentPc>(proc_address)
    };

    let name_cstr = CString::new("GetReturnPc").unwrap();
    let proc_address = GetProcAddress(dll, name_cstr.as_ptr());
    let get_return_pc: GetReturnPc = if proc_address.is_null() {
        log::info!("Failed to get function address for GetReturnPc. Error: {}", GetLastError());
        return None;
    } else {
        std::mem::transmute::<*mut winapi::shared::minwindef::__some_function, GetReturnPc>(proc_address)
    };

    let name_cstr = CString::new("GetCurrentSp").unwrap();
    let proc_address = GetProcAddress(dll, name_cstr.as_ptr());
    let get_current_sp: GetCurrentSp = if proc_address.is_null() {
        log::info!("Failed to get function address for GetCurrentSp. Error: {}", GetLastError());
        return None;
    } else {
        std::mem::transmute::<*mut winapi::shared::minwindef::__some_function, GetCurrentSp>(proc_address)
    };

    Some(FfiFunctionPointers {
        start_call_stack,
        continue_to_next_frame,
        get_current_frame_number,
        get_current_pc,
        get_return_pc,
        get_current_sp,
    })
}
