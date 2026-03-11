#------------------------------------------------------------------------------
# Copyright 2008 - 2020 ADVANCED MICRO DEVICES, INC.  All Rights Reserved.
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

.global invoke_demoted_routine
.global setup_call_gate
.global syscall_center

# .global SetupCpl0MsrStar
# .global RestoreCpl0MsrStar

.section .text
.align 8

# Segments defined in SmiException.nasm
.equ PROTECTED_DS,                   0x20
.equ LONG_CS_R0,                     0x38
.equ LONG_DS_R0,                     0x40
.equ LONG_CS_R3_PH,                  0x4B
.equ LONG_DS_R3,                     0x53
.equ LONG_CS_R3,                     0x5B
.equ CALL_GATE_OFFSET,               0x63

# MSR constants
.equ MSR_IA32_EFER,                 0xC0000080
.equ MSR_IA32_EFER_SCE_MASK,        0x00000001

.equ MSR_IA32_STAR,                 0xC0000081
.equ MSR_IA32_LSTAR,                0xC0000082
.equ MSR_IA32_GS_BASE,              0xC0000101
.equ MSR_IA32_KERNEL_GS_BASE,       0xC0000102


.macro CHECK_RAX
    cmp     rax, 0
    jz      4f
.endm

#------------------------------------------------------------------------------
# /**
#   Invoke specified routine on specified core in CPL 3.
#
#   @param[in]      CpuIndex            CpuIndex value of intended core, cannot be
#                                       greater than mNumberOfCpus.
#   @param[in]      Cpl3Routine         Function pointer to demoted routine.
#   @param[in]      ArgCount            Number of arguments needed by Cpl3Routine.
#   @param          ...                 The variable argument list whose count is defined by
#                                       ArgCount. Its contented will be accessed and populated
#                                       to the registers and/or CPL3 stack areas per EFIAPI
#                                       calling convention.
#
#   @retval EFI_SUCCESS                 The demoted routine returns successfully.
#   @retval Others                      Errors caught by subroutines during ring transitioning
#                                       or error code returned from demoted routine.
# **/
# EFI_STATUS
# EFIAPI
# InvokeDemotedRoutine (
#   IN UINTN                 CpuIndex,
#   IN EFI_PHYSICAL_ADDRESS  Cpl3Routine,
#   IN EFI_PHYSICAL_ADDRESS  Cpl3Stack,
#   IN UINTN                 ArgCount,
#   ...
#   );
# Calling convention: Arg0 in RCX, Arg1 in RDX, Arg2 in R8, Arg3 in R9, more on the stack
#------------------------------------------------------------------------------
invoke_demoted_routine:
    #Preserve input parameters onto reg parameter stack area for later usage
    mov     [rsp + 0x20], r9
    mov     [rsp + 0x18], r8
    mov     [rsp + 0x10], rdx
    mov     [rsp + 0x08], rcx

    #Preserve nonvolatile registers, in case demoted routines mess with them
    push    rbp
    mov     rbp, rsp
    #Clear the lowest 16 bit after saving rsp, to make sure the stack pointer 16byte aligned
    and     rsp, -16

    push    rbx
    push    rdi
    push    rsi
    push    r12
    push    r13
    push    r14
    push    r15

    #Preserve the updated rbp as we need them on return
    push    rbp

    mov     r15, r8
    and     r15, -16

    # Set up the MSR STAR, LSTAR, EFER, GS_BASE and KERNEL_GS_BASE, in situ
    mov     rcx, MSR_IA32_STAR
    rdmsr
    push    rdx
    push    rax

    mov     edx, LONG_CS_R3_PH
    shl     edx, 16
    add     edx, LONG_CS_R0
    wrmsr

    mov     rcx, MSR_IA32_LSTAR
    rdmsr
    push    rdx
    push    rax

    lea     rax, syscall_center
    lea     rdx, syscall_center
    shr     rdx, 32
    wrmsr

    mov     rcx, MSR_IA32_EFER
    rdmsr
    push    rdx
    push    rax

    or      rax, MSR_IA32_EFER_SCE_MASK
    wrmsr

    mov     rcx, MSR_IA32_GS_BASE
    rdmsr
    push    rdx
    push    rax

    xor     rdx, rdx
    xor     rax, rax
    wrmsr

    mov     rcx, MSR_IA32_KERNEL_GS_BASE
    rdmsr
    push    rdx
    push    rax

    mov     eax, esp
    sub     eax, 16
    mov     rdx, rsp
    sub     rdx, 16
    shr     rdx, 32
    wrmsr

    # This is to do the GS trick upon syscall entry
    sub     rsp, 8

    # This is to do the GS trick upon syscall entry
    mov     rdx, rsp
    sub     rdx, 8
    push    rdx

    # Now the stack will look like
    # Current RSP           <- Incoming calls will operate on top of this
    # 0                     <- Will be used for user stack saving
    # KERNEL_GS_BASE * 2    <- Will be restored on return
    # GS_BASE * 2           <- Will be restored on return
    # EFER                  <- Will be restored on return
    # LSTAR * 2             <- Will be restored on return
    # STAR * 2              <- Will be restored on return
    # One version of RBP    <- Value after we pushed NV registers
    # r15
    # r14
    # r13
    # r12
    # rsi
    # rdi
    # rbx
    # ?                     <- Potential buffer for unaligned incoming caller
    # Original RBP
    # ---------------       <- RSP When the caller invokes this
    # rcx
    # rdx
    # r8
    # r9

    #Setup call gate for return
    lea     rcx, [rip + 5f]
    mov     rdx, rsp
    sub     rsp, 0x20
    call    setup_call_gate
    add     rsp, 0x20

    #Same level far return to apply GDT change
    xor     rcx, rcx
    mov     rcx, cs
    push    rcx                 #prepare cs on the stack
    lea     rax, [rip + 2f]
    push    rax                 #prepare return rip on the stack
    retfq

2:
    #Prepare for ds, es, fs, gs
    xor     rax, rax
    mov     ax, LONG_DS_R3
    mov     ds, ax
    mov     es, ax
    mov     fs, ax
    mov     gs, ax

    #Prepare input arguments
    mov     rax, [rbp + 0x28]           #Get ArgCount from stack
    CHECK_RAX
    mov     rcx, [rbp + 0x30]           #First input argument for demoted routine
    dec     rax
    CHECK_RAX
    mov     rdx, [rbp + 0x38]           #Second input argument for demoted routine
    dec     rax
    CHECK_RAX
    mov     r8, [rbp + 0x40]            #Third input argument for demoted routine
    dec     rax
    CHECK_RAX
    mov     r9, [rbp + 0x48]            #Forth input argument for demoted routine
    dec     rax
    CHECK_RAX
    #For further input arguments, they will be put on the stack
    xor     rbx, rbx                    #rbx=0
    mov     r14, rax
    shl     r14, 3                      #r14=8*rax
    sub     r15, r14                    #r15-=r14, offset the stack for remainder of input arguments
    sub     r15, 0x20                   #r15-=0x20, 4 stack parameters
    and     r15, -16                    #finally we worry about the stack alignment in CPL3
3:
    mov     r14, [rbp + 0x48 + rbx]     #r14=*(rbp+0x48+rbx)
    mov     [r15 + 0x20 + rbx], r14     #*(r15+0x20+rbx)=r14
    add     rbx, 0x08                   #rbx+=0x08
    dec     rax
    CHECK_RAX
    jmp     3b

4:
    #Demote to CPL3 by far return, it will take care of cs and ss
    #Note: we did more pushes on the way, so need to compensate the calculation when grabbing earlier pushed values
    sub     r15, 0x08                   #dummy r15 displacement, to mimic the return pointer on the stack
    push    LONG_DS_R3                  #prepare ss on the stack
    mov     rax, r15                    #grab Cpl3StackPtr from r15
    push    rax                         #prepare CPL3 stack pointer on the stack
    push    LONG_CS_R3                  #prepare cs on the stack
    mov     rax, [rbp + 0x18]           #grab routine pointer from stack
    push    rax                         #prepare routine pointer on the stack

    mov     r15, CALL_GATE_OFFSET       #This is our way to come back, do not mess it up
    shl     r15, 32                     #Call gate on call far stack should be CS:rIP

    retfq

    #2000 years later...

5:
    #First offset the return far related 4 pushes (we have 0 count of arguments):
    #PUSH.v old_SS // #SS on this or next pushes use SS.sel as error code
    #PUSH.v old_RSP
    #PUSH.v old_CS
    #PUSH.v next_RIP
    add     rsp, 0x20

    #Demoted routine is responsible for returning to this point by invoking call gate
    #Return status should still be in rax, save it before calling other functions
    push    rax

    add     rsp, 24

    pop     rax
    pop     rdx
    mov     rcx, MSR_IA32_KERNEL_GS_BASE
    wrmsr

    pop     rax
    pop     rdx
    mov     rcx, MSR_IA32_GS_BASE
    wrmsr

    pop     rax
    pop     rdx
    mov     rcx, MSR_IA32_EFER
    wrmsr

    pop     rax
    pop     rdx
    mov     rcx, MSR_IA32_LSTAR
    wrmsr

    pop     rax
    pop     rdx
    mov     rcx, MSR_IA32_STAR
    wrmsr

    mov     rax, [rsp - 13 * 8]

    xor     rcx, rcx
    mov     cx, LONG_DS_R0
    mov     ds, cx
    mov     es, cx
    mov     fs, cx
    mov     gs, cx

    add     rsp, 0x08       #Unwind the rbp from the last net-push
    #Unwind the rest of the pushes
    pop     r15
    pop     r14
    pop     r13
    pop     r12
    pop     rsi
    pop     rdi
    pop     rbx
    mov     rsp, rbp
    pop     rbp

    ret
