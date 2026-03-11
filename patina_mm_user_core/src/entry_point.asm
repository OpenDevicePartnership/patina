#
# Entry point to a Standalone MM driver.
#
# Copyright (c), Microsoft Corporation.
# SPDX-License-Identifier: BSD-2-Clause-Patent
#

.section .data

.section .text
.global user_core_main
.global efi_main


.align 8
# Shim layer that redefines the contract between runtime module and init.
efi_main:

    #By the time we are here, it should be everything CPL3 already
    sub     rsp, 0x28

    #To boot strap this driver, we directly call the entry point worker
    call    user_core_main

    #Restore the stack pointer
    add     rsp, 0x28

    # Once returned, we will get returned status in rax, don't touch it, if you can help
    # r15 contains call gate selector that was planned ahead
    push    r15                         # New selector to be used, which is set to call gate by the supervisor
    .byte   0xff, 0x1c, 0x24            # call    far qword [rsp]# return to ring 0 via call gate m16:32
1:
    jmp     1b                           # Code should not reach here
