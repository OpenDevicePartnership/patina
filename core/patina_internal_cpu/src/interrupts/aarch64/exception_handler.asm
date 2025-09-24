#
# Copyright (c) 2011 - 2021, Arm Limited. All rights reserved.<BR>
# Portion of Copyright (c) 2014 NVIDIA Corporation. All rights reserved.<BR>
# Copyright (c) 2016 HP Development Company, L.P.
#
# SPDX-License-Identifier: Apache-2.0
#
#------------------------------------------------------------------------------

##
#  This is the stack constructed by the exception handler (low address to high address).
#  X0 to FAR makes up the EFI_SYSTEM_CONTEXT for AArch64.
#
#  UINT64  X0;     0x000
#  UINT64  X1;     0x008
#  UINT64  X2;     0x010
#  UINT64  X3;     0x018
#  UINT64  X4;     0x020
#  UINT64  X5;     0x028
#  UINT64  X6;     0x030
#  UINT64  X7;     0x038
#  UINT64  X8;     0x040
#  UINT64  X9;     0x048
#  UINT64  X10;    0x050
#  UINT64  X11;    0x058
#  UINT64  X12;    0x060
#  UINT64  X13;    0x068
#  UINT64  X14;    0x070
#  UINT64  X15;    0x078
#  UINT64  X16;    0x080
#  UINT64  X17;    0x088
#  UINT64  X18;    0x090
#  UINT64  X19;    0x098
#  UINT64  X20;    0x0a0
#  UINT64  X21;    0x0a8
#  UINT64  X22;    0x0b0
#  UINT64  X23;    0x0b8
#  UINT64  X24;    0x0c0
#  UINT64  X25;    0x0c8
#  UINT64  X26;    0x0d0
#  UINT64  X27;    0x0d8
#  UINT64  X28;    0x0e0
#  UINT64  FP;     0x0e8
#  UINT64  LR;     0x0f0
#  UINT64  SP;     0x0f8
#
#  FP/SIMD Registers. 128bit if used as Q-regs.
#  UINT64  V0[2];  0x100
#  UINT64  V1[2];  0x110
#  UINT64  V2[2];  0x120
#  UINT64  V3[2];  0x130
#  UINT64  V4[2];  0x140
#  UINT64  V5[2];  0x150
#  UINT64  V6[2];  0x160
#  UINT64  V7[2];  0x170
#  UINT64  V8[2];  0x180
#  UINT64  V9[2];  0x190
#  UINT64  V10[2]; 0x1a0
#  UINT64  V11[2]; 0x1b0
#  UINT64  V12[2]; 0x1c0
#  UINT64  V13[2]; 0x1d0
#  UINT64  V14[2]; 0x1e0
#  UINT64  V15[2]; 0x1f0
#  UINT64  V16[2]; 0x200
#  UINT64  V17[2]; 0x210
#  UINT64  V18[2]; 0x220
#  UINT64  V19[2]; 0x230
#  UINT64  V20[2]; 0x240
#  UINT64  V21[2]; 0x250
#  UINT64  V22[2]; 0x260
#  UINT64  V23[2]; 0x270
#  UINT64  V24[2]; 0x280
#  UINT64  V25[2]; 0x290
#  UINT64  V26[2]; 0x2a0
#  UINT64  V27[2]; 0x2b0
#  UINT64  V28[2]; 0x2c0
#  UINT64  V29[2]; 0x2d0
#  UINT64  V30[2]; 0x2e0
#  UINT64  V31[2]; 0x2f0
#
#  System Context
#  UINT64  ELR;    0x300
#  UINT64  SPSR;   0x308
#  UINT64  FPSR;   0x310
#  UINT64  ESR;    0x318
#  UINT64  FAR;    0x320
#  UINT64  Padding;0x328
##

  .section .data

  .global sp_el0_end

# Stack for SP_EL0 of 0x2000 bytes. Also set to 8KB aligned, which corresponds to BIT13.
  .align 13
sp_el0_start:
  .space 0x2000
sp_el0_end:

  .section .text

  .global exception_handlers_start
  .global exception_handler

  .set GP_CONTEXT_SIZE,    (32 *  8)
  .set FP_CONTEXT_SIZE,    (32 * 16)
  .set SYS_CONTEXT_SIZE,   ( 6 *  8)

  .set EXCEPT_AARCH64_SYNCHRONOUS_EXCEPTIONS,  0
  .set EXCEPT_AARCH64_IRQ,                     1
  .set EXCEPT_AARCH64_FIQ,                     2
  .set EXCEPT_AARCH64_SERROR,                  3

# Vector table offset definitions
  .set ARM_VECTOR_CUR_SP0_SYNC,  0x000
  .set ARM_VECTOR_CUR_SP0_IRQ,   0x080
  .set ARM_VECTOR_CUR_SP0_FIQ,   0x100
  .set ARM_VECTOR_CUR_SP0_SERR,  0x180

  .set ARM_VECTOR_CUR_SPX_SYNC,  0x200
  .set ARM_VECTOR_CUR_SPX_IRQ,   0x280
  .set ARM_VECTOR_CUR_SPX_FIQ,   0x300
  .set ARM_VECTOR_CUR_SPX_SERR,  0x380

  .set ARM_VECTOR_LOW_A64_SYNC,  0x400
  .set ARM_VECTOR_LOW_A64_IRQ,   0x480
  .set ARM_VECTOR_LOW_A64_FIQ,   0x500
  .set ARM_VECTOR_LOW_A64_SERR,  0x580

  .set ARM_VECTOR_LOW_A32_SYNC,  0x600
  .set ARM_VECTOR_LOW_A32_IRQ,   0x680
  .set ARM_VECTOR_LOW_A32_FIQ,   0x700
  .set ARM_VECTOR_LOW_A32_SERR,  0x780

#
# There are two methods for installing AArch64 exception vectors:
#  1. Install a copy of the vectors to a location specified by a PCD
#  2. Write VBAR directly, requiring that vectors have proper alignment (2K)
# The conditional below adjusts the alignment requirement based on which
# exception vector initialization method is used.
#

  .section .text.exception_handlers_start,"ax";
  .align 11;
  .org 0x0;

exception_handlers_start:
  .macro  ExceptionEntry, val, sp=SPx
  #
  # Our backtrace and register dump code is written in C and so it requires
  # a stack. This makes it difficult to produce meaningful diagnostics when
  # the stack pointer has been corrupted. So in such cases (i.e., when taking
  # synchronous exceptions), this macro is expanded with \sp set to SP0, in
  # which case we switch to the SP_EL0 stack pointer, which has been
  # initialized to point to a buffer that has been set aside for this purpose.
  #
  # Since 'sp' may no longer refer to the stack frame that was active when
  # the exception was taken, we may have to switch back and forth between
  # SP_EL0 and SP_ELx to record the correct value for SP in the context struct.
  #
  .ifnc   \sp, SPx
  msr     SPsel, xzr
  .endif

  # Move the stackpointer so we can reach our structure with the str instruction.
  sub sp, sp, #(FP_CONTEXT_SIZE + SYS_CONTEXT_SIZE)

  # Push the GP registers so we can record the exception context
  stp      x0, x1, [sp, #(-GP_CONTEXT_SIZE)]!
  stp      x2, x3, [sp, #0x10]
  stp      x4, x5, [sp, #0x20]
  stp      x6, x7, [sp, #0x30]
  stp      x8,  x9,  [sp, #0x40]
  stp      x10, x11, [sp, #0x50]
  stp      x12, x13, [sp, #0x60]
  stp      x14, x15, [sp, #0x70]
  stp      x16, x17, [sp, #0x80]
  stp      x18, x19, [sp, #0x90]
  stp      x20, x21, [sp, #0xa0]
  stp      x22, x23, [sp, #0xb0]
  stp      x24, x25, [sp, #0xc0]
  stp      x26, x27, [sp, #0xd0]
  stp      x28, x29, [sp, #0xe0]
  add      x28, sp, #(GP_CONTEXT_SIZE + FP_CONTEXT_SIZE + SYS_CONTEXT_SIZE)

  .ifnc    \sp, SPx
  msr      SPsel, #1
  mov      x7, sp
  msr      SPsel, xzr
  .else
  mov      x7, x28
  .endif

  stp      x30,  x7, [sp, #0xf0]

  # Record the type of exception that occurred.
  mov       x0, #\val

  # Jump to our general handler to deal with all the common parts and process the exception.
  b         common_exception_routine

  .endm

#
# Current EL with SP0 : 0x0 - 0x180
#
  .org ARM_VECTOR_CUR_SP0_SYNC
SynchronousExceptionSP0:
  ExceptionEntry  EXCEPT_AARCH64_SYNCHRONOUS_EXCEPTIONS

  .org ARM_VECTOR_CUR_SP0_IRQ
IrqSP0:
  ExceptionEntry  EXCEPT_AARCH64_IRQ

  .org ARM_VECTOR_CUR_SP0_FIQ
FiqSP0:
  ExceptionEntry  EXCEPT_AARCH64_FIQ

  .org ARM_VECTOR_CUR_SP0_SERR
SErrorSP0:
  ExceptionEntry  EXCEPT_AARCH64_SERROR

#
# Current EL with SPx: 0x200 - 0x380
#
  .org ARM_VECTOR_CUR_SPX_SYNC
SynchronousExceptionSPx:
  ExceptionEntry  EXCEPT_AARCH64_SYNCHRONOUS_EXCEPTIONS, SP0

  .org ARM_VECTOR_CUR_SPX_IRQ
IrqSPx:
  ExceptionEntry  EXCEPT_AARCH64_IRQ

  .org ARM_VECTOR_CUR_SPX_FIQ
FiqSPx:
  ExceptionEntry  EXCEPT_AARCH64_FIQ

  .org ARM_VECTOR_CUR_SPX_SERR
SErrorSPx:
  ExceptionEntry  EXCEPT_AARCH64_SERROR

#
# Lower EL using AArch64 : 0x400 - 0x580
#
  .org ARM_VECTOR_LOW_A64_SYNC
SynchronousExceptionA64:
  ExceptionEntry  EXCEPT_AARCH64_SYNCHRONOUS_EXCEPTIONS

  .org ARM_VECTOR_LOW_A64_IRQ
IrqA64:
  ExceptionEntry  EXCEPT_AARCH64_IRQ

  .org ARM_VECTOR_LOW_A64_FIQ
FiqA64:
  ExceptionEntry  EXCEPT_AARCH64_FIQ

  .org ARM_VECTOR_LOW_A64_SERR
SErrorA64:
  ExceptionEntry  EXCEPT_AARCH64_SERROR

#
# Lower EL using AArch32 : 0x600 - 0x780
#
  .org ARM_VECTOR_LOW_A32_SYNC
SynchronousExceptionA32:
  ExceptionEntry  EXCEPT_AARCH64_SYNCHRONOUS_EXCEPTIONS

  .org ARM_VECTOR_LOW_A32_IRQ
IrqA32:
  ExceptionEntry  EXCEPT_AARCH64_IRQ

  .org ARM_VECTOR_LOW_A32_FIQ
FiqA32:
  ExceptionEntry  EXCEPT_AARCH64_FIQ

  .org ARM_VECTOR_LOW_A32_SERR
SErrorA32:
  ExceptionEntry  EXCEPT_AARCH64_SERROR

  .org 0x800;
  .section .text.exception_handlers_start,"ax";
  .align 3

common_exception_routine:

  mrs      x2, elr_el2
  mrs      x3, spsr_el2
  mrs      x5, esr_el2
  mrs      x6, far_el2
  mrs      x4, fpsr

  # Save the SYS regs
  stp      x2,  x3,  [x28, #(-SYS_CONTEXT_SIZE)]!
  stp      x4,  x5,  [x28, #0x10]
  str      x6,  [x28, #0x20]

  # Push FP regs to Stack.
  stp      q0,  q1,  [x28, #(-FP_CONTEXT_SIZE)]!
  stp      q2,  q3,  [x28, #0x20]
  stp      q4,  q5,  [x28, #0x40]
  stp      q6,  q7,  [x28, #0x60]
  stp      q8,  q9,  [x28, #0x80]
  stp      q10, q11, [x28, #0xa0]
  stp      q12, q13, [x28, #0xc0]
  stp      q14, q15, [x28, #0xe0]
  stp      q16, q17, [x28, #0x100]
  stp      q18, q19, [x28, #0x120]
  stp      q20, q21, [x28, #0x140]
  stp      q22, q23, [x28, #0x160]
  stp      q24, q25, [x28, #0x180]
  stp      q26, q27, [x28, #0x1a0]
  stp      q28, q29, [x28, #0x1c0]
  stp      q30, q31, [x28, #0x1e0]

  # x0 still holds the exception type.
  # Set x1 to point to the top of our struct on the Stack
  mov      x1, sp

  # Call into rust routine.
  bl       exception_handler

  # Pop as many GP regs as we can before entering the critical section below
  ldp      x2,  x3,  [sp, #0x10]
  ldp      x4,  x5,  [sp, #0x20]
  ldp      x6,  x7,  [sp, #0x30]
  ldp      x8,  x9,  [sp, #0x40]
  ldp      x10, x11, [sp, #0x50]
  ldp      x12, x13, [sp, #0x60]
  ldp      x14, x15, [sp, #0x70]
  ldp      x16, x17, [sp, #0x80]
  ldp      x18, x19, [sp, #0x90]
  ldp      x20, x21, [sp, #0xa0]
  ldp      x22, x23, [sp, #0xb0]
  ldp      x24, x25, [sp, #0xc0]
  ldp      x26, x27, [sp, #0xd0]
  ldp      x0,  x1,  [sp], #0xe0

  # Pop FP regs from Stack.
  ldp      q2,  q3,  [x28, #0x20]
  ldp      q4,  q5,  [x28, #0x40]
  ldp      q6,  q7,  [x28, #0x60]
  ldp      q8,  q9,  [x28, #0x80]
  ldp      q10, q11, [x28, #0xa0]
  ldp      q12, q13, [x28, #0xc0]
  ldp      q14, q15, [x28, #0xe0]
  ldp      q16, q17, [x28, #0x100]
  ldp      q18, q19, [x28, #0x120]
  ldp      q20, q21, [x28, #0x140]
  ldp      q22, q23, [x28, #0x160]
  ldp      q24, q25, [x28, #0x180]
  ldp      q26, q27, [x28, #0x1a0]
  ldp      q28, q29, [x28, #0x1c0]
  ldp      q30, q31, [x28, #0x1e0]
  ldp      q0,  q1,  [x28], #FP_CONTEXT_SIZE

  # Pop the SYS regs we need
  ldp      x29, x30, [x28]
  ldr      x28, [x28, #0x10]
  msr      fpsr, x28

  #
  # Disable interrupt(IRQ and FIQ) before restoring context,
  # or else the context will be corrupted by interrupt reentrance.
  # Interrupt mask will be restored from spsr by hardware when we call eret
  #
  msr   daifset, #3
  isb

  msr      elr_el2, x29
  msr      spsr_el2, x30

  # pop remaining GP regs and return from exception.
  ldr      x30, [sp, #0xf0 - 0xe0]
  ldp      x28, x29, [sp], #GP_CONTEXT_SIZE - 0xe0

  # Adjust SP to be where we started from when we came into the handler.
  # The handler can not change the SP.
  add      sp, sp, #FP_CONTEXT_SIZE + SYS_CONTEXT_SIZE

  eret
