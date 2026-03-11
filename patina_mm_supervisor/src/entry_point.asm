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

.align 8
# Shim layer that redefines the contract between runtime module and init.
efi_main:

    jmp     rust_main
