#
# Exception entry point logic for X64.
#
# Copyright (C) Microsoft Corporation. All rights reserved.
#
# SPDX-License-Identifier: Apache-2.0
#

.section .data

.section .text
.global exception_handler
.global common_interrupt_entry
.global AsmIdtVectorBegin

.align 8
# These need to be of a fixed length so that they can be indexed into. For this
# reason, avoid variable length jumps or pushes.
AsmIdtVectorBegin:
    .set vector, 0
    .rept 256
        sub     rsp, 8
        mov     qword ptr [rsp], vector
        push    rax
        mov     rax, offset common_interrupt_entry
        jmp     rax
        .set vector, vector+1
    .endr
AsmIdtVectorEnd:

# Bit field of the x64 interrupt vectors that will push an error code to the stack.
.set VECTORS_WITH_ERROR_CODES, 0x60227D00

common_interrupt_entry:

    # The stack is inconsistent at this point. Some exceptions will have an error
    # code and some won't. Check if this vector has an error code, and if not, inject
    # a zero to keep a consistent stack. The Vector is at the top of the stack as
    # pushed in the vector above.

    # temporarily swap the RCX and Vector, rax is already pushed from above.
    xchg    rcx, qword ptr [rsp + 8]

    # The vector is in RCX, use it to as a bit shift vector index and compare with
    # indices with the errorcodes.

    mov     rax, 1
    shl     rax, cl
    and     rax, VECTORS_WITH_ERROR_CODES

    # Neither xchg or pop will affect the ZF, so go ahead and restore them.
    pop     rax
    xchg    qword ptr [rsp], rcx

    jnz     stack_normalized

    # No error code, inject 0 before the vector.

    push    qword ptr [rsp]
    mov     qword ptr [rsp + 8], 0

stack_normalized:

    push    0

    #
    # At This point, the stack is as follows. The offset is relative to the address
    # stored in R15 during this routine.
    #
    # --------------------------------------------------------------------------
    # Offset -8 |   Unused (alignent)
    # Size   8  |
    # --------------------------------------------------------------------------
    # Offset 0  |   Vector Index
    # Size   8  |
    # --------------------------------------------------------------------------
    # Offset 8  |   Error Code (or injected 0)
    # Size   8  |
    # --------------------------------------------------------------------------
    # Offset 16 |   VA of Instruction Pointer (Start of stack frame)
    # Size   8  |
    # --------------------------------------------------------------------------
    # Offset 24 |   Code Segment Selector
    # Size   2  |
    # --------------------------------------------------------------------------
    # Offset 26 |   Unused (alignment)
    # Size   6  |
    # --------------------------------------------------------------------------
    # Offset 32 |   RFlags
    # Size   8  |
    # --------------------------------------------------------------------------
    # Offset 40 |   VA of Stack Pointer
    # Size   8  |
    # --------------------------------------------------------------------------
    # Offset 48 |   Stack Segment Selector
    # Size   2  |
    # --------------------------------------------------------------------------
    # Offset 50 |   Unused (alignment)
    # Size   6  |
    # --------------------------------------------------------------------------
    #

    # Push the save state to the stack. This is building the system context structure
    # on the stack to pass to the rust code.
    push    r15

    # Now that R15 is saved, use it to store the start of the above structure.
    mov     r15, rsp
    add     r15, 16     # account for R15 and the error code hint.

    # Continue pushing the state structure.
    push    r14
    push    r13
    push    r12
    push    r11
    push    r10
    push    r9
    push    r8
    push    rax
    push    rcx
    push    rdx
    push    rbx
    push    qword ptr [r15 + 40] # Pull the stack pointer from the stack frame.
    push    rbp
    push    rsi
    push    rdi

    # Push selectors. Use rax as the selectors can't be directly pushed.
    movzx   rax, word ptr [r15 + 48] # Pull the SS from the stack frame.
    push    rax
    movzx   rax, word ptr [r15 + 24] # Pull the CS from the stack frame.
    push    rax
    mov     rax, ds
    push    rax
    mov     rax, es
    push    rax
    mov     rax, fs
    push    rax
    mov     rax, gs
    push    rax

    # Pull the instruction pointer from the stack frame
    push    qword ptr [r15 + 16]

    # IDTR and GDTR cannot be directly pushed, allocate the space and store.
    sub     rsp, 16
    sidt    [rsp]
    sub     rsp, 16
    sgdt    [rsp]

    # push the TR & LDTR, these are 16 bits so zero RAX to not push garbage.
    xor     rax, rax
    str     rax
    push    rax
    sldt    ax
    push    rax

    # Push RFlags from the stack frame.
    push    qword ptr [r15 + 32]

    # Control registers, must be moved to RAX first.
    mov     rax, cr8
    push    rax
    mov     rax, cr4
    or      rax, 0x208 # Enable FXSAVE & FXRSTOR. This is needed later.
    mov     cr4, rax
    push    rax
    mov     rax, cr3
    push    rax
    mov     rax, cr2
    push    rax
    xor     rax, rax
    push    rax
    mov     rax, cr0
    push    rax

    # Debug registers.
    mov     rax, dr7
    push    rax
    mov     rax, dr6
    push    rax
    mov     rax, dr3
    push    rax
    mov     rax, dr2
    push    rax
    mov     rax, dr1
    push    rax
    mov     rax, dr0
    push    rax

    # FX_SAVE_STATE
    sub     rsp, 512
    mov     rax, rsp
    fxsave  [rax]

    # Exception data
    push    qword ptr [r15 + 8]

    # Call into the rust code. Use the UEFI calling convention for consistency.
    cld     # EFLAGs must be cleared.

    # Arg 0 - Vector index
    mov     rcx, qword ptr [r15]

    # Arg 1 - Pointer to the system context structure.
    mov     rdx, rsp

    sub     rsp, 4 * 8 + 8 # max parameter space + 8 for 16 bytes alignment.
    call    exception_handler
    add     rsp, 4 * 8 + 8

    #
    # Return from the exception. Begin by unwinding the context.
    #

    # TODO: Control Flow Guard?

    # Skip the exception data.
    add     rsp, 8

    # Restore fx_save_state
    mov     rsi, rsp
    fxrstor [rsi]
    add     rsp, 512

    # Skip DR0-DR7 to support in-circuit emulators or debugger set breakpoints
    # in the current exception state.
    add     rsp, 8 * 6

    # Control registers
    pop     rax
    mov     cr0, rax
    add     rsp, 8   # Skip CR1
    pop     rax
    mov     cr2, rax
    pop     rax
    mov     cr3, rax
    pop     rax
    mov     cr4, rax
    pop     rax
    mov     cr8, rax

    # RFlags, pop to the stack frame.
    pop     qword ptr [r15 + 32]

    # LDTR & TR, these ar architectural and shouldn't be altered. Keep stack frame
    # version
    add     rsp, 48

    # Pop the instruction pointer (RIP) back to the stack frame.
    pop     qword ptr [r15 + 16]

    # Segment selectors, intentionally drop Gs & Fs as they are not used in X64.
    # Es & Ds shouldn't be changed.
    pop     rax # gs
    pop     rax # fs
    pop     rax # es
    pop     rax # ds
    # CS & SS go to the stack frame
    pop     qword ptr [r15 + 24]  # CS
    pop     qword ptr [r15 + 48]  # SS

    # General purpose registers
    pop     rdi
    pop     rsi
    pop     rbp
    pop     qword ptr [r15 + 40]  # RSP
    pop     rbx
    pop     rdx
    pop     rcx
    pop     rax
    pop     r8
    pop     r9
    pop     r10
    pop     r11
    pop     r12
    pop     r13
    pop     r14
    pop     r15

    # Pop alignment qword, the error code (real or fake), and the vector index
    add     rsp, 24

    # Return from the interrupt
    iretq

#
# Gets the address of a vector in the generated vectors. It does this by finding
# the length of each vector and then returning base + (size * index).
#
# RCX - The index of the assembly routine
#
# This routine only uses volatile registers for the EFI calling convention.
#
AsmGetVectorAddress:
    lea     r10, AsmIdtVectorBegin
    lea     rax, AsmIdtVectorEnd
    sub     rax, r10
    shr     rax, 8 # >> 8 == / 256

    # Get the offset to the provided index
    mul     rcx

    # Get the address of the vector
    add     rax, r10
    ret
