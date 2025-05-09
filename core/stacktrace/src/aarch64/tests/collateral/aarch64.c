//
// When compiled, this file generates an `aarch64.dll`. Once loaded into memory,
// calling `StartCallStack()` sets up the call stack required for validating
// stack-walking code. The call stack can also be controlled externally through
// the exported functions.
//
// Steps to compile: build.cmd
//

#include <stdio.h>
#include <stdint.h>
#include <stdbool.h>
#include <intrin.h>

extern uint64_t GetSp();

volatile uint64_t current_pc = 0;
volatile uint64_t return_pc = 0;
volatile uint64_t current_sp = 0;

// Used to find the runtime function and unwind codes
__declspec(dllexport)
uint64_t GetCurrentPc() {
    return current_pc;
}

// Used to validate the calculated return pc
__declspec(dllexport)
uint64_t GetReturnPc() {
    return return_pc;
}

// Used to calculate the return pc
__declspec(dllexport)
uint64_t GetCurrentSp() {
    return current_sp;
}

volatile int in_frame = 0;
__declspec(dllexport)
int GetCurrentFrameNumber() {
    return in_frame;
}

// Continue when the thread should move to the next frame in the call stack
volatile int next_frame = 1;
__declspec(dllexport)
void ContinueToNextFrame() {
    next_frame++;
}

// This function will effectively return the instruction pointer of the previous
// function. DO NOT INLINE IT!
__declspec(noinline)
uint64_t _GetCurrentPc() {
    return (uintptr_t)_ReturnAddress();
}

__declspec(noinline)
int func1(int a) {
    return_pc = (uintptr_t)_ReturnAddress();
    current_sp = GetSp();
    current_pc = _GetCurrentPc();
    // printf("dll: %I64x %I64x %I64x\n", current_pc, current_sp, return_pc);
    in_frame = next_frame;
    while (next_frame <= 1);
    return a;
}

__declspec(noinline)
int func2(int a, int b) {
    int res = func1(a);
    return_pc = (uintptr_t)_ReturnAddress();
    current_sp = GetSp();
    current_pc = _GetCurrentPc();
    // printf("dll: %I64x %I64x %I64x\n", current_pc, current_sp, return_pc);
    in_frame = next_frame;
    while (next_frame <= 2);
    return res + b;
}

__declspec(noinline)
int func3(int a, int b, int c) {
    int res = func2(a, b);
    return_pc = (uintptr_t)_ReturnAddress();
    current_sp = GetSp();
    current_pc = _GetCurrentPc();
    // printf("dll: %I64x %I64x %I64x\n", current_pc, current_sp, return_pc);
    in_frame = next_frame;
    while (next_frame <= 3);
    return res + c;
}

__declspec(noinline)
int func4(int a, int b, int c, int d) {
    int res = func3(a, b, c);
    return_pc = (uintptr_t)_ReturnAddress();
    current_sp = GetSp();
    current_pc = _GetCurrentPc();
    // printf("dll: %I64x %I64x %I64x\n", current_pc, current_sp, return_pc);
    in_frame = next_frame;
    while (next_frame <= 4);
    return res + d;
}

__declspec(noinline)
__declspec(dllexport)
void StartCallStack() {
    int ret = func4(10, 20, 30, 40);
    // printf("dll: res = %d\n", ret);
}
