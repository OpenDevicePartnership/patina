#
# Real-mode to long-mode AP bootstrap used by INIT-SIPI-SIPI.
#
# This stub lives in a read-only non-executable section since it should
# only be copied out before execution.
#
# Copyright (c) Microsoft Corporation.
# SPDX-License-Identifier: Apache-2.0
#

.pushsection .rodata.ap_bootstrap, "a"
.balign 16
.globl ap_bootstrap_start
.globl ap_bootstrap_rm_page_base
.globl ap_bootstrap_rm_gdtr_offset
.globl ap_bootstrap_rm_pm_entry
.globl ap_bootstrap_protected_mode
.globl ap_bootstrap_long_mode
.globl ap_bootstrap_data
.globl ap_bootstrap_end

#
# 16-bit real-mode code is emitted as bytes because the Rust toolchain does
# not provide a supported 16-bit assembly mode and this was deemed the lesser
# evil compared to a new toolchain dependency for this tiny stub or using
# an opaque binary file.
#
# The ap_bootstrap_rm_* labels below identify operand offsets within this
# template. The rust code patches values at those offsets after copying the template.
#
ap_bootstrap_start:
    .byte 0xFA                         # cli
    .byte 0xFC                         # cld
    .byte 0x8C, 0xC8                   # mov ax, cs
    .byte 0x8E, 0xD8                   # mov ds, ax
    .byte 0x8E, 0xC0                   # mov es, ax
    .byte 0x8E, 0xD0                   # mov ss, ax
    .byte 0x66, 0xBB                   # mov ebx, imm32 (patched operand follows)
ap_bootstrap_rm_page_base:
    .long 0                            # Patched page base. fixed 32-bit operand offset
    .byte 0x0F, 0x01, 0x16             # lgdt [disp16] (patched displacement follows)
ap_bootstrap_rm_gdtr_offset:
    .word 0                            # Patched GDTR offset. fixed 16-bit operand offset
    .byte 0x0F, 0x20, 0xC0             # mov eax, cr0
    .byte 0x66, 0x83, 0xC8, 0x01       # or eax, 1 (Protected Mode Enable)
    .byte 0x0F, 0x22, 0xC0             # mov cr0, eax
    .byte 0x66, 0xEA                   # jmp ptr16:32 (patched offset follows)
ap_bootstrap_rm_pm_entry:
    .long 0                            # Patched entry offset. fixed 32-bit operand offset
    .word {code32_selector}

.code32
.balign 16
ap_bootstrap_protected_mode:
    # Load the 32-bit data selectors
    mov ax, {data32_selector}
    mov ds, ax
    mov es, ax
    mov ss, ax

    # EBX = Address base of the startup page, from ap_bootstrap_rm_page_base above.

    # Load the provided CR4.
    mov eax, dword ptr [ebx + {bootstrap_data_off} + {data_cr4_off}]
    mov cr4, eax

    # Load the provided EFER.
    mov ecx, {ia32_efer}
    mov eax, dword ptr [ebx + {bootstrap_data_off} + {data_efer_off}]
    mov edx, dword ptr [ebx + {bootstrap_data_off} + {data_efer_off} + 4]
    wrmsr

    # Load CR3 and CR0.
    mov eax, dword ptr [ebx + {bootstrap_data_off} + {data_cr3_off}]
    mov cr3, eax
    mov eax, dword ptr [ebx + {bootstrap_data_off} + {data_cr0_off}]
    mov cr0, eax

    # jmp fword ptr [ebx + BootstrapData.long_mode]
    .byte 0xFF, 0xAB
    .long {bootstrap_data_off} + {data_long_mode_off}

.code64
ap_bootstrap_long_mode:
    mov rax, qword ptr [rbx + {bootstrap_data_off} + {data_entry_off}]
    jmp rax

    .fill {bootstrap_data_off} - (. - ap_bootstrap_start), 1, 0
ap_bootstrap_data:
    .zero {data_size}
ap_bootstrap_end:
.popsection
