#
# Entry point shim for the MM User Core.
#
# Copyright (c) Microsoft Corporation.
# SPDX-License-Identifier: BSD-2-Clause-Patent
#

.section .data

.section .text
.global user_core_main
.global efi_main


.align 8
# Image entry point. The supervisor enters here in Ring 3. Calls user_core_main and returns
# its status to Ring 0 through the call gate.
efi_main:

    sub     rsp, 0x28

    call    user_core_main

    # Restore the stack pointer.
    add     rsp, 0x28

    # RAX holds the returned status and must be preserved.
    # The supervisor put the call gate far pointer in R15 (selector in bits 32-47).
    push    r15                         # Far pointer to the call gate
    .byte   0xff, 0x1c, 0x24            # call far [rsp] (m16:32): return to Ring 0 through the call gate
1:
    jmp     1b                           # Not reached
