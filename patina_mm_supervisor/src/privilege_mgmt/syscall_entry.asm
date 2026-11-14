#------------------------------------------------------------------------------
# Copyright (c) 2020, AMD Incorporated. All rights reserved.<BR>
# Copyright (c) 2017, Intel Corporation. All rights reserved.<BR>
# Copyright (c) Microsoft Corporation.
# SPDX-License-Identifier: BSD-2-Clause-Patent
#
# Module Name:
#
#   syscall_entry.asm
#
# Abstract:
#
#   The `syscall` entry stub, syscall_center. It swaps to the supervisor GS base, switches to
#   the supervisor stack, saves the Ring 3 register state and calls syscall_dispatcher, then
#   restores that state and returns to Ring 3 with `sysret`.
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

# OFFSET_OF (MM_SUPV_SYSCALL_CACHE, MmSupvRsp)
.equ MM_SUPV_RSP,                     0x00
# OFFSET_OF (MM_SUPV_SYSCALL_CACHE, SavedUserRsp)
.equ SAVED_USER_RSP,                  0x08

#------------------------------------------------------------------------------
# Caller Interface:
# UINT64
# Custom convention: CallIndex in RAX, Arg1 in RDX, Arg2 in R8, Arg3 in R9
# SysCall (
#   UINTN CallIndex,
#   UINTN Arg1,
#   UINTN Arg2,
#   UINTN Arg3
#   );
#
# Backend Interface (the return value goes back to Ring 3 in RAX):
#   call_index      - syscall index (from RAX)
#   arg1            - first argument (from RDX)
#   arg2            - second argument (from R8)
#   arg3            - third argument (from R9)
#   caller_addr     - Ring 3 return address (from RCX)
#   ring3_stack_ptr - Ring 3 stack pointer
#
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
# Architectural definition: CallerAddr in RCX, RFLAGS in R11 from the x64 syscall instruction

    # IA32_FMASK already clears DF and AC on entry; these repeat it so the stub does not depend
    # on an MSR programmed elsewhere. Neither touches a register or a flag anything here reads.
    cld
    clac

    swapgs  # load the supervisor GS base, saving the user one
    mov gs:[SAVED_USER_RSP], rsp # save the Ring 3 stack pointer
    mov rsp, gs:[MM_SUPV_RSP] # switch to the supervisor stack

    # Save the Ring 3 registers.
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

    # Save the FPU and SSE state (FX_SAVE_STATE_X64, 512 bytes).
    sub rsp, 512
    mov rdi, rsp
    .byte 0x0f, 0xae, 0x07 #fxsave [rdi]

    # Load the Ring 0 data selector into DS, ES and FS. GS is left alone to keep its base.
    xor     rbx, rbx
    mov     bx, LONG_DS_R0
    mov     ds, bx
    mov     es, bx
    mov     fs, bx

    mov     rsi, gs:[SAVED_USER_RSP]     # Ring 3 stack pointer
    push    rsi                          # 6th argument: ring3_stack_ptr
    push    rcx                          # 5th argument: caller_addr
    mov     rcx, rax
    sub     rsp, 0x20

    call    syscall_dispatcher

    add     rsp, 0x20
    pop     rcx                          # unwind caller_addr
    pop     rsi                          # unwind ring3_stack_ptr

    # Load the Ring 3 data selector into DS, ES and FS.
    xor     rbx, rbx
    mov     bx, LONG_DS_R3
    mov     ds, bx
    mov     es, bx
    mov     fs, bx

    mov rsi, rsp
    .byte 0x0f, 0xae, 0x0e # fxrstor [rsi]
    add rsp, 512

    mov     rsp, rbp

    # Restore the Ring 3 registers saved on the supervisor stack.
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
    pop     rcx           # Ring 3 return address for sysret

    add     rsp, 8        # skip the saved RAX; RAX holds the return value
    mov     rsp, gs:[SAVED_USER_RSP]  # restore the Ring 3 RSP
    swapgs  # restore the user GS base
    .byte   0x48          # REX.W prefix: sysretq returns to 64-bit mode
    sysret                # RAX contains return value
