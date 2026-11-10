#------------------------------------------------------------------------------
# Copyright 2008 - 2020 ADVANCED MICRO DEVICES, INC.  All Rights Reserved.
# Copyright (c) Microsoft Corporation.
# SPDX-License-Identifier: BSD-2-Clause-Patent
#
# Module Name:
#
#   call_gate_transfer.asm
#
# Abstract:
#
#   Ring 3 demotion through invoke_demoted_routine: programs the syscall MSRs, calls
#   setup_call_gate to install the call gate the demoted routine returns through, and far
#   returns into the CPL3 routine. Also holds the call gate return path, which restores the
#   saved MSRs, segments and nonvolatile registers before returning to the Ring 0 caller.
#
# Notes:
#
#------------------------------------------------------------------------------

.section .data

.global invoke_demoted_routine
.global setup_call_gate
.global syscall_center

.section .text
.align 8

# Segments defined in SmiException.nasm
.equ LONG_CS_R0,                     0x38
.equ LONG_DS_R0,                     0x40
.equ LONG_CS_R3_PH,                  0x4B
.equ LONG_DS_R3,                     0x53
.equ LONG_CS_R3,                     0x5B
# Selector of the call gate at GDT offset 0x60, with RPL 3.
.equ CALL_GATE_SEL,                  0x63

# MSR constants
.equ MSR_IA32_EFER,                 0xC0000080
.equ MSR_IA32_EFER_SCE_MASK,        0x00000001

.equ MSR_IA32_STAR,                 0xC0000081
.equ MSR_IA32_LSTAR,                0xC0000082
.equ MSR_IA32_FMASK,                0xC0000084
.equ MSR_IA32_GS_BASE,              0xC0000101
.equ MSR_IA32_KERNEL_GS_BASE,       0xC0000102

# RFLAGS bits the CPU clears on `syscall` entry (RFLAGS &= ~IA32_FMASK).
#
# `syscall` consults only this MSR, so at its reset value of 0 Ring 3 chooses the flags Ring 0
# runs under: DF reverses the `rep movs` behind every memcpy, and AC disables SMAP for the whole
# handler. Every bit with a defined meaning is masked, which is easier to audit than a minimal
# mask that must be re-derived whenever the Ring 0 code changes. The bits left set are VM, VIF
# and VIP, which have no meaning in long mode, and reserved bit 1, which is always set.
# CF|PF|AF|ZF|SF|TF|IF|DF|OF|IOPL|NT|RF|AC|ID
.equ MSR_IA32_FMASK_VALUE,          0x00257FD5


.macro CHECK_RAX
    cmp     rax, 0
    jz      4f
.endm

#------------------------------------------------------------------------------
# /**
#   Invoke specified routine on specified core in CPL 3.
#
#   @param[in]      CpuIndex            Index of the intended core, less than the CPU
#                                       count. Not used by this routine.
#   @param[in]      Cpl3Routine         Function pointer to demoted routine.
#   @param[in]      Cpl3Stack           Top of the CPL3 stack the demoted routine runs on.
#   @param[in]      ArgCount            Number of arguments needed by Cpl3Routine.
#   @param          ...                 ArgCount arguments for Cpl3Routine, placed in
#                                       registers and on the CPL3 stack per the EFIAPI
#                                       calling convention.
#
#   @retval EFI_SUCCESS                 The demoted routine returned successfully.
#   @retval Others                      An error from the ring transition or the status
#                                       returned by the demoted routine.
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
    # Save the register arguments to their home slots on the caller's stack.
    mov     [rsp + 0x20], r9
    mov     [rsp + 0x18], r8
    mov     [rsp + 0x10], rdx
    mov     [rsp + 0x08], rcx

    # Save nonvolatile registers; the demoted routine may clobber them.
    push    rbp
    mov     rbp, rsp
    # Align RSP down to 16 bytes; RBP keeps the unaligned value.
    and     rsp, -16

    push    rbx
    push    rdi
    push    rsi
    push    r12
    push    r13
    push    r14
    push    r15

    # Save RBP again; the return path skips over this copy.
    push    rbp

    # Reserve the 0x20-byte shadow space the EFIAPI callee may spill RCX, RDX, R8 and R9 into,
    # so it stays inside this CPU's CPL3 stack whatever the argument count.
    mov     r15, r8
    sub     r15, 0x20
    and     r15, -16

    # Save and program STAR, LSTAR, FMASK, EFER, GS_BASE and KERNEL_GS_BASE. The old values
    # stay on the stack and are restored on return.
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

    # Must precede the EFER.SCE write below: SCE is what makes `syscall` reachable, and FMASK
    # is the only thing that stops Ring 3 from picking Ring 0's RFLAGS once it is.
    mov     rcx, MSR_IA32_FMASK
    rdmsr
    push    rdx
    push    rax

    mov     eax, MSR_IA32_FMASK_VALUE
    xor     edx, edx
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

    # SAVED_USER_RSP slot (gs:[8] after swapgs): syscall entry saves the Ring 3 RSP here.
    sub     rsp, 8

    # MM_SUPV_RSP slot (gs:[0] after swapgs): the Ring 0 stack syscall entry switches to, which
    # is the address of this slot. KERNEL_GS_BASE was programmed above to point here.
    mov     rdx, rsp
    sub     rdx, 8
    push    rdx

    # The stack now looks like this:
    # Current RSP           <- MM_SUPV_RSP; syscalls run on the stack below this
    # (uninitialized)       <- SAVED_USER_RSP
    # KERNEL_GS_BASE * 2    <- Will be restored on return
    # GS_BASE * 2           <- Will be restored on return
    # EFER * 2              <- Will be restored on return
    # FMASK * 2             <- Will be restored on return
    # LSTAR * 2             <- Will be restored on return
    # STAR * 2              <- Will be restored on return
    # RBP                   <- Copy saved after the nonvolatile registers
    # r15
    # r14
    # r13
    # r12
    # rsi
    # rdi
    # rbx
    # ?                     <- Padding if the caller's RSP was not 16-byte aligned
    # Original RBP
    # ---------------       <- RSP on entry
    # return address
    # rcx
    # rdx
    # r8
    # r9

    # Install the return call gate: RCX = return address (label 5), RDX = CPL0 stack pointer.
    lea     rcx, [rip + 5f]
    mov     rdx, rsp
    sub     rsp, 0x20
    call    setup_call_gate
    add     rsp, 0x20

    # Same-privilege far return to reload CS after the GDT update.
    mov     rcx, cs
    push    rcx                 #prepare cs on the stack
    lea     rax, [rip + 2f]
    push    rax                 #prepare return rip on the stack
    retfq

2:
    # Load the Ring 3 data selector into DS, ES, FS and GS.
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
    mov     r9, [rbp + 0x48]            #Fourth input argument for demoted routine
    dec     rax
    CHECK_RAX
    # Remaining arguments go on the CPL3 stack.
    xor     rbx, rbx                    #rbx=0
    mov     r14, rax
    shl     r14, 3                      #r14=8*rax
    sub     r15, r14                    #r15-=r14, offset the stack for remainder of input arguments
    and     r15, -16                    #align the CPL3 stack to 16 bytes
3:
    mov     r14, [rbp + 0x50 + rbx]     #r14=*(rbp+0x50+rbx), starting at the fifth input argument
    mov     [r15 + 0x20 + rbx], r14     #*(r15+0x20+rbx)=r14
    add     rbx, 0x08                   #rbx+=0x08
    dec     rax
    CHECK_RAX
    jmp     3b

4:
    # Demote to CPL3 with a far return, which loads CS and SS.
    sub     r15, 0x08                   #dummy r15 displacement, to mimic the return pointer on the stack
    push    LONG_DS_R3                  #prepare ss on the stack
    mov     rax, r15                    #grab Cpl3StackPtr from r15
    push    rax                         #prepare CPL3 stack pointer on the stack
    push    LONG_CS_R3                  #prepare cs on the stack
    mov     rax, [rbp + 0x18]           #grab routine pointer from stack
    push    rax                         #prepare routine pointer on the stack

    mov     r15, CALL_GATE_SEL          #call gate selector the demoted routine returns through
    shl     r15, 32                     #selector in bits 63:32 for the far call through the gate

    retfq

5:
    # A call gate, unlike an interrupt gate, leaves RFLAGS unchanged across the privilege
    # change, and FMASK does not apply to it. Ring 3's DF and AC arrive here as-is, so clear
    # them before any Rust runs.
    cld
    clac

    # Drop the four values the call gate pushed (no parameters are copied):
    #PUSH.v old_SS // #SS on this or next pushes use SS.sel as error code
    #PUSH.v old_RSP
    #PUSH.v old_CS
    #PUSH.v next_RIP
    add     rsp, 0x20

    # The demoted routine returns here through the call gate with its status in RAX. Save it
    # before RAX is reused.
    push    rax

    # Reload the Ring 0 data selectors before GS_BASE is restored below. In 64-bit mode a
    # `mov gs` also loads the GS base from the descriptor, so a reload after the MSR write would
    # replace the restored GS_BASE with the flat descriptor's base of 0.
    xor     rcx, rcx
    mov     cx, LONG_DS_R0
    mov     ds, cx
    mov     es, cx
    mov     fs, cx
    mov     gs, cx

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
    mov     rcx, MSR_IA32_FMASK
    wrmsr

    pop     rax
    pop     rdx
    mov     rcx, MSR_IA32_LSTAR
    wrmsr

    pop     rax
    pop     rdx
    mov     rcx, MSR_IA32_STAR
    wrmsr

    # Reload the return status saved by `push rax` above, now below RSP. The 15 slots are the
    # 6 saved MSR pairs, the 2 syscall slots and the status itself; keep this in step with the
    # push sequence at the top of this routine.
    mov     rax, [rsp - 15 * 8]

    add     rsp, 0x08       #skip the saved RBP copy
    # Restore the nonvolatile registers.
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
