#
# Exception entry point logic for X64.
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
# Shim layer that redefines the contract between runtime module and init.
efi_main:

    jmp     rust_main
