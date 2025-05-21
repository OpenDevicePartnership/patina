//
// C test code to load the x64.dll in to memory and orchestrate the call stack.
//

#include <stdio.h>
#include <stdint.h>
#include <windows.h>

typedef void (*StartCallStack)();
typedef void (*ContinueToNextFrame)();
typedef int (*GetCurrentFrameNumber)();
typedef uint64_t(*GetCurrentRip)();
typedef uint64_t(*GetReturnRip)();
typedef uint64_t(*GetCurrentRsp)();

StartCallStack start_call_stack = NULL;
ContinueToNextFrame continue_to_next_frame = NULL;
GetCurrentFrameNumber get_current_frame_number = NULL;
GetCurrentRip get_current_rip = NULL;
GetReturnRip get_return_rip = NULL;
GetCurrentRsp get_current_rsp = NULL;

DWORD WINAPI CallStackThread(LPVOID lpParam) {
    // starts the call stack func4() -> func3() -> func2() -> func1() and loops
    start_call_stack();
    return 0;
}

int main() {
    HANDLE thread_handle = NULL;
    DWORD thread_id = 0;
    HMODULE dll = NULL;

    uint64_t current_rip = 0;
    uint64_t return_rip = 0;
    uint64_t current_rsp = 0;

    dll = LoadLibraryW(L".\\dll.dll");
    if (!dll) {
        printf("Failed to load DLL. Error: %ld\n", GetLastError());
        return 1;
    }

    // Get the address of the `start_call_stack` function
    start_call_stack = (StartCallStack)GetProcAddress(dll, "StartCallStack");
    if (!start_call_stack) {
        printf("Failed to get function address for StartCallStack(). Error: %ld\n", GetLastError());
        FreeLibrary(dll);
        return 1;
    }

    continue_to_next_frame = (ContinueToNextFrame)GetProcAddress(dll, "ContinueToNextFrame");
    if (!continue_to_next_frame) {
        printf("Failed to get function address for ContinueToNextFrame(). Error: %ld\n", GetLastError());
        FreeLibrary(dll);
        return 1;
    }

    get_current_frame_number = (GetCurrentFrameNumber)GetProcAddress(dll, "GetCurrentFrameNumber");
    if (!get_current_frame_number) {
        printf("Failed to get function address for GetCurrentFrameNumber(). Error: %ld\n", GetLastError());
        FreeLibrary(dll);
        return 1;
    }

    get_current_rip = (GetCurrentRip)GetProcAddress(dll, "GetCurrentRip");
    if (!get_current_rip) {
        printf("Failed to get function address for GetCurrentRip(). Error: %ld\n", GetLastError());
        FreeLibrary(dll);
        return 1;
    }

    get_return_rip = (GetReturnRip)GetProcAddress(dll, "GetReturnRip");
    if (!get_return_rip) {
        printf("Failed to get function address for GetReturnRip(). Error: %ld\n", GetLastError());
        FreeLibrary(dll);
        return 1;
    }

    get_current_rsp = (GetCurrentRsp)GetProcAddress(dll, "GetCurrentRsp");
    if (!get_current_rsp) {
        printf("Failed to get function address for GetCurrentRsp(). Error: %ld\n", GetLastError());
        FreeLibrary(dll);
        return 1;
    }


    thread_handle = CreateThread(
        NULL,               // Default security attributes
        0,                  // Default stack size
        CallStackThread,
        NULL,       // Parameter to thread function
        0,
        &thread_id
    );

    if (thread_handle == NULL) {
        printf("Thread creation failed. Error: %ld\n", GetLastError());
        return 1;
    }

    printf("Thread created successfully! Thread ID: %ld\n", thread_id);

    while (get_current_frame_number() < 1); // Wait until thread gets in to frame 1 aka func1()
    current_rip = get_current_rip();
    current_rsp = get_current_rsp();
    return_rip = get_return_rip();

    // This should trigger completion of func1() and func2() will loop
    continue_to_next_frame();               // Trigger thread to get in to frame 2 aka func2()
    while (get_current_frame_number() < 2); // Wait until thread gets in to frame 2 aka func2()
    current_rip = get_current_rip();
    current_rsp = get_current_rsp();
    return_rip = get_return_rip();

    // This should trigger completion of func2() and func3() will loop
    continue_to_next_frame();               // Trigger thread to get in to frame 3 aka func3()
    while (get_current_frame_number() < 3); // Wait until thread gets in to frame 3 aka func3()
    current_rip = get_current_rip();
    current_rsp = get_current_rsp();
    return_rip = get_return_rip();

    // This should trigger completion of func3() and func4() will loop
    continue_to_next_frame();               // Trigger thread to get in to frame 4 aka func4()
    while (get_current_frame_number() < 4); // Wait until thread gets in to frame 4 aka func4()
    current_rip = get_current_rip();
    current_rsp = get_current_rsp();
    return_rip = get_return_rip();

    // This should trigger completion of func4()
    continue_to_next_frame();
    current_rip = get_current_rip();
    current_rsp = get_current_rsp();
    return_rip = get_return_rip();

    WaitForSingleObject(thread_handle, INFINITE);
    CloseHandle(thread_handle);
    FreeLibrary(dll);
    return 0;
}
