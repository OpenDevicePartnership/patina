#------------------------------------------------------------------------------
# Copyright (c) 2020, AMD Incorporated. All rights reserved.<BR>
# Copyright (c) 2017, Intel Corporation. All rights reserved.<BR>
# Copyright (c) Microsoft Corporation.
# SPDX-License-Identifier: BSD-2-Clause-Patent
#
# Module Name:
#
#   WriteTr.nasm
#
# Abstract:
#
#   Write TR register
#
# Notes:
#
#------------------------------------------------------------------------------

.section .data
.global syscall_center
.global syscall_dispatcher

.section .text
.align 8

# Segments defined in SmiException.nasm
.equ LONG_DS_R0,                      0x40
.equ LONG_DS_R3,                      0x53

# This should be OFFSET_OF (MM_SUPV_SYSCALL_CACHE, MmSupvRsp)
.equ MM_SUPV_RSP,                     0x00
# This should be OFFSET_OF (MM_SUPV_SYSCALL_CACHE, SavedUserRsp)
.equ SAVED_USER_RSP,                  0x08

#------------------------------------------------------------------------------
# Caller Interface:
# UINT64
# EFIAPI <SysV calling convention>
# SysCall (
#   UINTN CallIndex,
#   UINTN Arg1,
#   UINTN Arg2,
#   UINTN Arg3
#   );
#
# Backend Interface:
# /// C-compatible syscall dispatcher entry point.
# ///
# /// This function is called from the assembly syscall entry stub (SyscallCenter).
# ///
# /// # Arguments
# ///
# /// * `call_index` - Syscall index (from RAX)
# /// * `arg1` - First argument (from RDX)
# /// * `arg2` - Second argument (from R8)
# /// * `arg3` - Third argument (from R9)
# /// * `caller_addr` - Caller return address (from RCX)
# /// * `ring3_stack_ptr` - Ring 3 stack pointer
# ///
# /// # Returns
# ///
# /// The value to return in RAX.
# #[unsafe(no_mangle)]
# pub extern "efiapi" fn syscall_dispatcher(
#     call_index: u64,
#     arg1: u64,
#     arg2: u64,
#     arg3: u64,
#     caller_addr: u64,
#     ring3_stack_ptr: u64,
# ) -> u64;
#------------------------------------------------------------------------------
syscall_center:
# Calling convention: CallIndex in RAX, Arg1 in RDX, Arg2 in R8, Arg3 in R9 from SysCallLib
# Architectural definition: CallerAddr in RCX, rFLAGs in R11 from x64 syscall instruction
# push CallIndex stored at top of stack

    swapgs  # get kernel pointer, save user GSbase
    mov gs:[SAVED_USER_RSP], rsp # save user's stack pointer
    mov rsp, gs:[MM_SUPV_RSP] # set up kernel stack

    #Preserve all registers in CPL3
    push    rax
    push    rcx
    push    rbp
    push    rdx
    push    r8
    push    r9
    push    rsi
    push    r12
    push    rdi
    push    rbx
    push    r11
    push    r10
    push    r13
    push    r14
    push    r15

    mov     rbp, rsp
    and     rsp, -16

    ## FX_SAVE_STATE_X64 FxSaveState#
    sub rsp, 512
    mov rdi, rsp
    .byte 0x0f, 0xae, 0x07 #fxsave [rdi]

    #Prepare for ds, es, fs, gs
    xor     rbx, rbx
    mov     bx, LONG_DS_R0
    mov     ds, bx
    mov     es, bx
    mov     fs, bx

    mov     rsi, gs:[SAVED_USER_RSP]     # Save Ring 3 stack to RSI
    push    rsi                          # Push Ring 3 stack as Ring3Stack for syscall_dispatcher
    push    rcx                          # Push return address on stack as CallerAddr for syscall_dispatcher
    mov     rcx, rax
    sub     rsp, 0x20

    call    syscall_dispatcher

    add     rsp, 0x20
    pop     rcx                          # Restore SP to avoid stack overflow
    pop     rsi                          # Restore SI to avoid stack overflow

    #Prepare for ds, es, fs, gs
    xor     rbx, rbx
    mov     bx, LONG_DS_R3
    mov     ds, bx
    mov     es, bx
    mov     fs, bx

    mov rsi, rsp
    .byte 0x0f, 0xae, 0x0e # fxrstor [rsi]
    add rsp, 512

    mov     rsp, rbp

    #restore registers from CPL3 stack
    pop     r15
    pop     r14
    pop     r13
    pop     r10
    pop     r11
    pop     rbx
    pop     rdi
    pop     r12
    pop     rsi
    pop     r9
    pop     r8
    pop     rdx
    pop     rbp
    pop     rcx           # return rcx from stack

    add     rsp, 8        # return rsp to original position
    mov     rsp, gs:[SAVED_USER_RSP]  # restore user RSP
    swapgs  # restore user GS, save kernel pointer
    .byte   0x48          # return to the long mode
    sysret                # RAX contains return value
