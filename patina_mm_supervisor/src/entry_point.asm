#
# Image entry point shim for the MM Supervisor Core.
#
# Copyright (c) Microsoft Corporation.
#
# SPDX-License-Identifier: Apache-2.0
#

.section .data

.section .text
.global rust_main
.global efi_main

# Mark efi_main as an external function in the COFF symbol table (.scl 2 == external,
# .type 32 == DT_FUNCTION). Without this the linker emits efi_main as an S_PUB32 record
# with no function flag, and PDB consumers that only accept function publics (the SEA
# gen_aux tool) cannot resolve it.
.def efi_main
    .scl 2
    .type 32
.endef

.align 8
# efi_main jumps to rust_main, which the platform binary provides. The jump leaves the
# argument registers and the stack untouched.
efi_main:

    jmp     rust_main
